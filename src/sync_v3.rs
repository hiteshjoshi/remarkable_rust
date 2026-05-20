//! reMarkable cloud sync v3 — direct native-bundle upload.
//!
//! This is the protocol every reMarkable device speaks to
//! `internal.cloud.remarkable.com`. It's free with any reMarkable account
//! — Connect is a separate paid tier that adds storage and templates, but
//! the core sync API is open to everyone. Uploading a notebook this way
//! ships the real binary `.rm` files we built in Phase 1-3 directly, with
//! no SSH, no USB cable, and no cloud-side EPUB conversion.
//!
//! ## Protocol overview
//!
//! Files are stored as content-addressed blobs keyed by SHA-256. To add a
//! new document:
//!
//! 1. `PUT /sync/v3/files/<sha256>` for every file in the bundle.
//! 2. Build a *doc-index* blob: a text listing of `<file-hash>:0:<name>:0:<size>`
//!    lines, sorted by name. `PUT` it under its own hash.
//! 3. `GET /sync/v3/root` → `{ hash, generation }`. Fetch and parse the
//!    *root index* blob at `hash` — it's the same line format but lists
//!    every document.
//! 4. Replace-or-append our doc's entry, serialize, hash, `PUT` the new
//!    root blob.
//! 5. `PUT /sync/v3/root` with `{ hash, generation, broadcast: true }`.
//!    Server compares `generation` to its stored value; on a race it
//!    returns 412 and we retry from step 3.
//!
//! ## Schema versions
//!
//! Indexes carry their schema on the first line (`3` or `4`).
//! - v4 hashes are `sha256(blob_bytes)`.
//! - v3 hashes are `sha256(concat(binary_child_hashes))` sorted by id.
//! - v4 indexes include a `0:<label>:<count>:<totalSize>` totals row right
//!   after the schema line; v3 does not.
//!
//! We always honour the schema the server is currently using.

use base64::Engine;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::notebook::Bundle;

const SYNC_HOST: &str = "https://internal.cloud.remarkable.com";

fn root_url() -> String {
    format!("{SYNC_HOST}/sync/v3/root")
}

fn files_url(hash: &str) -> String {
    format!("{SYNC_HOST}/sync/v3/files/{hash}")
}

/// Index schema identifiers used by the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    V3,
    V4,
}

impl Schema {
    fn as_str(self) -> &'static str {
        match self {
            Schema::V3 => "3",
            Schema::V4 => "4",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s.trim() {
            "3" => Ok(Schema::V3),
            "4" => Ok(Schema::V4),
            other => Err(Error::InvalidResponse(format!(
                "unknown index schema {other:?}"
            ))),
        }
    }
}

/// One line of an index blob: a child hash plus the friendly name it's
/// stored under. For doc-indexes `id` is the filename
/// (e.g. `<doc>.metadata`); for the root index it's the docID (UUID).
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub hash: String,
    pub id: String,
    pub subfiles: u32,
    pub size: u64,
}

#[derive(Debug)]
pub struct RootState {
    pub schema: Schema,
    pub root_hash: String,
    pub generation: i64,
    pub entries: Vec<IndexEntry>,
}

/// Async client for the sync v3 endpoints.
pub struct SyncClient {
    http: reqwest::Client,
    token: String,
}

impl SyncClient {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("rr/", env!("CARGO_PKG_VERSION")))
            // The sync API doesn't 30x in practice, but keep manual redirect
            // handling so a GCS signed-URL bounce wouldn't carry our custom
            // `rm-*` headers along — those would confuse GCS.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Error::Network)?;
        Ok(Self {
            http,
            token: token.into(),
        })
    }

    fn auth_headers(&self) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|e| Error::Config(format!("bad bearer header: {e}")))?,
        );
        Ok(h)
    }

    /// `GET /sync/v3/root` and fetch the root index blob it points at.
    pub async fn load_root(&self) -> Result<RootState> {
        let url = root_url();
        tracing::debug!(method = "GET", url = %url, "sync_v3 load_root");
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(Error::Network)?;
        let status = resp.status();
        tracing::debug!(status = %status, "sync_v3 load_root response");
        let body = resp.bytes().await.map_err(Error::Network)?;
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        #[derive(Deserialize)]
        struct RootPointer {
            hash: String,
            generation: i64,
        }
        let p: RootPointer = serde_json::from_slice(&body)?;
        tracing::debug!(
            root_hash = %p.hash,
            generation = p.generation,
            "sync_v3 root pointer"
        );

        let index_body = self.get_blob(&p.hash, "root.docSchema").await?;
        let (schema, entries) = parse_index(&index_body)?;
        Ok(RootState {
            schema,
            root_hash: p.hash,
            generation: p.generation,
            entries,
        })
    }

    /// Fetch a blob by its content hash.
    ///
    /// The server REQUIRES an `rm-filename` header on GETs to
    /// `/sync/v3/files/<hash>`, even though the blob is content-addressed
    /// and the name is just a routing hint. The 400 response when the
    /// header is missing says "unexpected 'rm-filename' http header" —
    /// confusingly, this is the API's way of saying it expected the
    /// header and didn't get it. Pass the friendly name the parent index
    /// stored this hash under (e.g. `root.docSchema`, `<docID>.docSchema`,
    /// `<docID>.metadata`).
    pub async fn get_blob(&self, hash: &str, rm_filename: &str) -> Result<Vec<u8>> {
        let url = files_url(hash);
        tracing::debug!(method = "GET", url = %url, rm_filename = rm_filename, "sync_v3 blob fetch");
        let mut headers = self.auth_headers()?;
        headers.insert(
            HeaderName::from_static("rm-filename"),
            HeaderValue::from_str(rm_filename)
                .map_err(|e| Error::Config(format!("bad rm-filename: {e}")))?,
        );
        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(Error::Network)?;
        let status = resp.status();
        tracing::debug!(status = %status, "sync_v3 blob fetch response");
        let body = resp.bytes().await.map_err(Error::Network)?;
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Ok(body.to_vec())
    }

    /// `PUT /sync/v3/files/<hash>` — upload a single content-addressed
    /// blob. Carries the friendly filename, byte count, and a CRC32C hash
    /// in lowercase headers the server explicitly checks for.
    pub async fn put_blob(&self, hash: &str, data: &[u8], rm_filename: &str) -> Result<()> {
        let mut headers = self.auth_headers()?;
        // These three headers must be lowercase on the wire — the server
        // canonicalises lookups against the literal byte sequence. reqwest's
        // HeaderName::from_static stores lowercase by construction so we're
        // safe using it directly.
        headers.insert(
            HeaderName::from_static("rm-filename"),
            HeaderValue::from_str(rm_filename)
                .map_err(|e| Error::Config(format!("bad rm-filename: {e}")))?,
        );
        headers.insert(
            HeaderName::from_static("rm-filesize"),
            HeaderValue::from_str(&data.len().to_string())
                .map_err(|e| Error::Config(format!("bad rm-filesize: {e}")))?,
        );
        headers.insert(
            HeaderName::from_static("x-goog-hash"),
            HeaderValue::from_str(&format!("crc32c={}", crc32c_base64(data)))
                .map_err(|e| Error::Config(format!("bad x-goog-hash: {e}")))?,
        );

        let url = files_url(hash);
        tracing::debug!(
            method = "PUT",
            url = %url,
            rm_filename = rm_filename,
            rm_filesize = data.len(),
            "sync_v3 blob upload"
        );
        let resp = self
            .http
            .put(&url)
            .headers(headers)
            .body(data.to_vec())
            .send()
            .await
            .map_err(Error::Network)?;
        let status = resp.status();
        tracing::debug!(status = %status, "sync_v3 blob upload response");

        // 3xx — the sync server is handing us a signed URL (typically GCS).
        // PUT the bytes to that URL with only the headers it accepts.
        if status.is_redirection() {
            if let Some(location) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
            {
                tracing::debug!(redirect = %location, "following signed-URL redirect");
                return self.put_to_signed_url(&location, data).await;
            }
            let body = resp.bytes().await.map_err(Error::Network)?;
            return Err(Error::Api {
                status: status.as_u16(),
                body: format!(
                    "redirect without Location header: {}",
                    String::from_utf8_lossy(&body).trim()
                ),
            });
        }

        if status.is_success() {
            return Ok(());
        }
        let body = resp.bytes().await.map_err(Error::Network)?;
        Err(Error::Api {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    /// Upload `data` to a server-supplied signed URL (GCS / S3 style).
    /// These URLs reject our `rm-*` headers, so we send only `x-goog-hash`
    /// for CRC32C verification when the URL is GCS.
    async fn put_to_signed_url(&self, url: &str, data: &[u8]) -> Result<()> {
        let mut req = self.http.put(url).body(data.to_vec());
        // GCS uses x-goog-hash and respects nothing else from our custom
        // header set. S3-style signed URLs (if reMarkable ever switches)
        // tolerate it being absent.
        if url.contains("googleapis.com") || url.contains("storage.googleapis") {
            req = req.header("x-goog-hash", format!("crc32c={}", crc32c_base64(data)));
        }
        let resp = req.send().await.map_err(Error::Network)?;
        let status = resp.status();
        tracing::debug!(status = %status, "signed-URL upload response");
        if status.is_success() {
            return Ok(());
        }
        let body = resp.bytes().await.map_err(Error::Network)?;
        Err(Error::Api {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    /// `PUT /sync/v3/root` with the current generation. On 412 the caller
    /// is expected to re-fetch root and retry.
    pub async fn update_root(
        &self,
        new_hash: &str,
        current_generation: i64,
    ) -> Result<UpdateRootOutcome> {
        let mut headers = self.auth_headers()?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("rm-filename"),
            HeaderValue::from_static("roothash"),
        );

        #[derive(Serialize)]
        struct RootUpdateReq<'a> {
            broadcast: bool,
            hash: &'a str,
            generation: i64,
        }
        let body = serde_json::to_vec(&RootUpdateReq {
            broadcast: true,
            hash: new_hash,
            generation: current_generation,
        })?;

        let resp = self
            .http
            .put(root_url())
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(Error::Network)?;
        let status = resp.status();
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Ok(UpdateRootOutcome::GenerationRace);
        }
        let body = resp.bytes().await.map_err(Error::Network)?;
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            generation: i64,
        }
        let r: Resp = serde_json::from_slice(&body).unwrap_or(Resp {
            generation: current_generation + 1,
        });
        Ok(UpdateRootOutcome::Updated {
            new_generation: r.generation,
        })
    }

    /// Upload an entire notebook bundle to the cloud as a new document.
    /// Atomically attaches it to the root index via optimistic-concurrency
    /// retries.
    pub async fn upload_bundle(&self, bundle: &Bundle) -> Result<UploadResult> {
        // 1. Snapshot files we need to push.
        let doc_uuid = bundle.doc_uuid.to_string();
        let files = bundle_files(bundle);

        // 2. PUT every blob with its rm-filename.
        let mut doc_entries: Vec<IndexEntry> = Vec::with_capacity(files.len());
        for f in &files {
            let hash = sha256_hex(&f.bytes);
            self.put_blob(&hash, &f.bytes, &f.cloud_name).await?;
            doc_entries.push(IndexEntry {
                hash,
                id: f.cloud_name.clone(),
                subfiles: 0,
                size: f.bytes.len() as u64,
            });
        }

        // 3. Up to a few retries on the root-generation race.
        const MAX_ATTEMPTS: usize = 3;
        for attempt in 0..MAX_ATTEMPTS {
            let root = self.load_root().await?;

            // Build the doc-index for our doc and PUT it.
            let doc_body = serialize_index(root.schema, &doc_uuid, &doc_entries, false);
            let doc_index_hash = hash_index(root.schema, &doc_entries, &doc_body)?;
            self.put_blob(&doc_index_hash, &doc_body, &format!("{doc_uuid}.docSchema"))
                .await?;

            // Replace-or-append our doc into the root entries.
            let total_size: u64 = doc_entries.iter().map(|e| e.size).sum();
            let new_entry = IndexEntry {
                hash: doc_index_hash.clone(),
                id: doc_uuid.clone(),
                subfiles: doc_entries.len() as u32,
                size: total_size,
            };
            let new_root_entries = replace_or_append(root.entries.clone(), new_entry);

            // Serialize + hash + upload new root index.
            let root_body = serialize_index(root.schema, ".", &new_root_entries, true);
            let new_root_hash = hash_index(root.schema, &new_root_entries, &root_body)?;
            self.put_blob(&new_root_hash, &root_body, "root.docSchema")
                .await?;

            // Atomically swap the root pointer.
            match self.update_root(&new_root_hash, root.generation).await? {
                UpdateRootOutcome::Updated { new_generation } => {
                    return Ok(UploadResult {
                        doc_id: doc_uuid,
                        doc_index_hash,
                        new_root_hash,
                        new_generation,
                    });
                }
                UpdateRootOutcome::GenerationRace if attempt + 1 < MAX_ATTEMPTS => {
                    continue;
                }
                UpdateRootOutcome::GenerationRace => {
                    return Err(Error::Other(
                        "root generation race after 3 retries — try again".into(),
                    ));
                }
            }
        }
        unreachable!()
    }
}

/// What the root-update PUT returned.
#[derive(Debug)]
pub enum UpdateRootOutcome {
    Updated { new_generation: i64 },
    GenerationRace,
}

/// Summary returned by [`SyncClient::upload_bundle`].
#[derive(Debug, Clone)]
pub struct UploadResult {
    pub doc_id: String,
    pub doc_index_hash: String,
    pub new_root_hash: String,
    pub new_generation: i64,
}

/// One file the cloud needs: bytes plus the friendly filename the server
/// stamps the blob with via `rm-filename` and the doc index references it
/// by.
struct CloudFile {
    cloud_name: String,
    bytes: Vec<u8>,
}

/// Flatten a [`Bundle`] into the per-file list the cloud expects.
fn bundle_files(bundle: &Bundle) -> Vec<CloudFile> {
    let doc = bundle.doc_uuid.to_string();
    let mut out = Vec::with_capacity(3 + bundle.pages.len() * 2);
    out.push(CloudFile {
        cloud_name: format!("{doc}.metadata"),
        bytes: bundle.metadata_json.clone().into_bytes(),
    });
    out.push(CloudFile {
        cloud_name: format!("{doc}.content"),
        bytes: bundle.content_json.clone().into_bytes(),
    });
    out.push(CloudFile {
        cloud_name: format!("{doc}.pagedata"),
        bytes: bundle.pagedata.clone().into_bytes(),
    });
    for page in &bundle.pages {
        let pid = page.uuid.to_string();
        out.push(CloudFile {
            cloud_name: format!("{doc}/{pid}.rm"),
            bytes: page.rm_bytes.clone(),
        });
        out.push(CloudFile {
            cloud_name: format!("{doc}/{pid}-metadata.json"),
            bytes: page.metadata_json.clone().into_bytes(),
        });
        // Per-image PNG attachments live at
        // `<doc>/<page-uuid>/<image-uuid>.png` and ship as additional
        // blobs in the doc-index so the device knows to fetch them.
        for img in &page.images {
            out.push(CloudFile {
                cloud_name: format!("{doc}/{pid}/{}", img.filename),
                bytes: img.png_bytes.clone(),
            });
        }
    }
    out
}

/// Insert `entry` into `entries`, replacing any existing entry with the
/// same `id`. Returns a new vector — callers want the original preserved.
fn replace_or_append(mut entries: Vec<IndexEntry>, entry: IndexEntry) -> Vec<IndexEntry> {
    if let Some(slot) = entries.iter_mut().find(|e| e.id == entry.id) {
        *slot = entry;
    } else {
        entries.push(entry);
    }
    entries
}

/// Parse a root or doc-index blob body. Format:
/// - Line 1: schema id (`3` or `4`).
/// - v4 only: optional totals line `0:<label>:<count>:<totalSize>`.
/// - Each subsequent line: `<hash>:<type>:<id>:<subfiles>:<size>`.
fn parse_index(body: &[u8]) -> Result<(Schema, Vec<IndexEntry>)> {
    let text = std::str::from_utf8(body).map_err(|e| {
        Error::InvalidResponse(format!("index blob is not utf-8: {e}"))
    })?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let schema_line = lines
        .next()
        .ok_or_else(|| Error::InvalidResponse("empty index blob".into()))?;
    let schema = Schema::from_str(schema_line)?;

    let mut entries = Vec::new();
    let mut first_after_schema = true;
    for line in lines {
        // v4 totals row: "0:<label>:<count>:<totalSize>". Detected by 4
        // colon-separated fields whose first field is exactly "0".
        if schema == Schema::V4 && first_after_schema {
            first_after_schema = false;
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() == 4 && parts[0] == "0" {
                continue;
            }
        }
        first_after_schema = false;

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 5 {
            continue;
        }
        let subfiles = parts[3].parse::<u32>().unwrap_or(0);
        let size = parts[4].parse::<u64>().unwrap_or(0);
        entries.push(IndexEntry {
            hash: parts[0].to_string(),
            id: parts[2].to_string(),
            subfiles,
            size,
        });
    }
    Ok((schema, entries))
}

/// Serialize an index blob. `label` is the docID for doc indexes or `"."`
/// for the root. `is_root` controls the per-entry "type" field, which is
/// `"80000000"` only for the v3 root and `"0"` everywhere else.
fn serialize_index(
    schema: Schema,
    label: &str,
    entries: &[IndexEntry],
    is_root: bool,
) -> Vec<u8> {
    let mut sorted: Vec<&IndexEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::new();
    out.push_str(schema.as_str());
    out.push('\n');

    if schema == Schema::V4 && !label.is_empty() {
        let total: u64 = sorted.iter().map(|e| e.size).sum();
        out.push_str(&format!("0:{label}:{}:{}\n", sorted.len(), total));
    }

    let type_field = if schema == Schema::V3 && is_root {
        "80000000"
    } else {
        "0"
    };

    for e in sorted {
        out.push_str(&format!(
            "{}:{}:{}:{}:{}\n",
            e.hash, type_field, e.id, e.subfiles, e.size
        ));
    }
    out.into_bytes()
}

/// Compute the hash that goes into a parent (root pointer or parent
/// doc-index entry).
///
/// - v4: SHA-256 over the serialised blob bytes.
/// - v3: SHA-256 over the concatenation of the *binary-decoded* child
///   hashes, ordered by id. Yes, decoded — the server expects raw bytes
///   here, not the hex string.
fn hash_index(schema: Schema, entries: &[IndexEntry], body: &[u8]) -> Result<String> {
    match schema {
        Schema::V4 => Ok(sha256_hex(body)),
        Schema::V3 => {
            let mut sorted: Vec<&IndexEntry> = entries.iter().collect();
            sorted.sort_by(|a, b| a.id.cmp(&b.id));
            let mut hasher = Sha256::new();
            for e in sorted {
                let raw = hex::decode(&e.hash).map_err(|err| {
                    Error::InvalidResponse(format!("bad child hash {:?}: {err}", e.hash))
                })?;
                hasher.update(&raw);
            }
            Ok(hex::encode(hasher.finalize()))
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn crc32c_base64(data: &[u8]) -> String {
    let v: u32 = crc32c::crc32c(data);
    let bytes = v.to_be_bytes();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trip() {
        assert_eq!(Schema::V3.as_str(), "3");
        assert_eq!(Schema::V4.as_str(), "4");
        assert!(matches!(Schema::from_str("3"), Ok(Schema::V3)));
        assert!(matches!(Schema::from_str("4"), Ok(Schema::V4)));
        assert!(Schema::from_str("5").is_err());
    }

    #[test]
    fn parse_v4_index_with_totals_row() {
        let body = b"4\n0:.:2:300\nabc:0:doc1:1:100\ndef:0:doc2:1:200\n";
        let (schema, entries) = parse_index(body).unwrap();
        assert_eq!(schema, Schema::V4);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].hash, "abc");
        assert_eq!(entries[0].id, "doc1");
        assert_eq!(entries[1].size, 200);
    }

    #[test]
    fn parse_v3_index_without_totals_row() {
        let body = b"3\nabc:80000000:doc1:1:100\ndef:80000000:doc2:1:200\n";
        let (schema, entries) = parse_index(body).unwrap();
        assert_eq!(schema, Schema::V3);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let entries = vec![
            IndexEntry {
                hash: "deadbeef".repeat(8),
                id: "z.rm".into(),
                subfiles: 0,
                size: 42,
            },
            IndexEntry {
                hash: "cafebabe".repeat(8),
                id: "a.rm".into(),
                subfiles: 0,
                size: 7,
            },
        ];
        let body = serialize_index(Schema::V4, "docid", &entries, false);
        let (schema, parsed) = parse_index(&body).unwrap();
        assert_eq!(schema, Schema::V4);
        // Entries come back sorted by id, so a.rm before z.rm.
        assert_eq!(parsed[0].id, "a.rm");
        assert_eq!(parsed[1].id, "z.rm");
    }

    #[test]
    fn replace_or_append_replaces_by_id() {
        let entries = vec![
            IndexEntry { hash: "a".into(), id: "1".into(), subfiles: 0, size: 1 },
            IndexEntry { hash: "b".into(), id: "2".into(), subfiles: 0, size: 2 },
        ];
        let updated = replace_or_append(
            entries.clone(),
            IndexEntry { hash: "z".into(), id: "2".into(), subfiles: 0, size: 99 },
        );
        assert_eq!(updated.len(), 2);
        assert_eq!(updated.iter().find(|e| e.id == "2").unwrap().hash, "z");

        let appended = replace_or_append(
            entries,
            IndexEntry { hash: "c".into(), id: "3".into(), subfiles: 0, size: 3 },
        );
        assert_eq!(appended.len(), 3);
        assert_eq!(appended.last().unwrap().id, "3");
    }

    #[test]
    fn v4_hash_is_sha256_of_body() {
        let body = b"4\nabc:0:x:0:1\n";
        let entries = vec![IndexEntry {
            hash: "abc".into(),
            id: "x".into(),
            subfiles: 0,
            size: 1,
        }];
        let got = hash_index(Schema::V4, &entries, body).unwrap();
        assert_eq!(got, sha256_hex(body));
    }

    #[test]
    fn v3_hash_is_sha256_of_concatenated_decoded_child_hashes() {
        // Two children sorted by id; v3 hash is sha256 of decoded hashes,
        // sorted.
        let entries = vec![
            IndexEntry {
                hash: "aa".repeat(32), // 64 hex chars, valid 32 bytes
                id: "z".into(),
                subfiles: 0,
                size: 1,
            },
            IndexEntry {
                hash: "bb".repeat(32),
                id: "a".into(),
                subfiles: 0,
                size: 2,
            },
        ];
        // Expected: sha256(bytes("bb"*32) || bytes("aa"*32)) — sorted by id
        // means "a" first, then "z".
        let mut h = Sha256::new();
        h.update(hex::decode("bb".repeat(32)).unwrap());
        h.update(hex::decode("aa".repeat(32)).unwrap());
        let expected = hex::encode(h.finalize());
        let got = hash_index(Schema::V3, &entries, &[]).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn crc32c_matches_known_vector() {
        // From RFC 3720 Appendix B.4: CRC32C of "123456789" is 0xE3069283.
        let v = crc32c::crc32c(b"123456789");
        assert_eq!(v, 0xE306_9283);
    }

    #[test]
    fn urls_anchor_to_sync_host() {
        assert!(SYNC_HOST.contains("remarkable.com"));
        assert!(root_url().starts_with(SYNC_HOST));
        assert!(files_url("abc").starts_with(SYNC_HOST));
        assert!(files_url("abc").ends_with("/abc"));
    }
}

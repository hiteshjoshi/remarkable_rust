//! reMarkable Document API client.
//!
//! This is the *high-level* cloud API the official Read-on-reMarkable Chrome
//! extension uses (see `docs/protocol/`):
//!
//! - `POST /doc/v2/files`     — upload as native EPUB document
//! - `POST /import/v1/files`  — upload + server-side conversion to a
//!                              **native reMarkable notebook** (.rm). This
//!                              is the path we use by default.
//! - `POST /doc/v2/files` (Content-Type: folder) — create folder
//! - `GET  /doc/v2/files`     — list
//! - `DELETE /doc/v2/files`   — multi-delete by hash
//! - `PATCH /doc/v2/files`    — rename / move / pin
//!
//! All endpoints share three custom headers: `Authorization`, `rM-Source`,
//! `rM-Meta` (a base64 JSON blob carrying file metadata).

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const DEFAULT_SOURCE: &str = "RoR-Browser";
const DEFAULT_ORIENTATION: &str = "portrait";

/// Default base URL when no tectonic claim is available.
pub const DEFAULT_BASE_URL: &str = "https://internal.cloud.remarkable.com";
const TECTONIC_TEMPLATE: &str = "https://web.{}.tectonic.remarkable.com";

/// Resolve the per-user base URL using the JWT's `tectonic` claim.
/// Mirrors `resolveCloudWebHost` in the extension (`utils/config.ts`).
pub fn resolve_base_url(tectonic: Option<&str>) -> String {
    match tectonic.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => TECTONIC_TEMPLATE.replace("{}", t),
        None => DEFAULT_BASE_URL.to_owned(),
    }
}

/// A document or folder. Unifies the two response shapes the cloud returns:
///
/// * `POST /import/v1/files` and `POST /doc/v2/files` return `{docID, hash}`.
/// * `GET  /doc/v2/files` returns rows with `id`, `fileName`, `type`, …
///
/// We accept either via serde aliases and default missing fields.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileItem {
    #[serde(default, alias = "docID", alias = "docId")]
    pub id: String,
    #[serde(default)]
    pub hash: String,
    #[serde(rename = "type", default)]
    pub file_type: String,
    #[serde(rename = "fileName", alias = "file_name", default)]
    pub file_name: String,
    #[serde(rename = "createdAt", alias = "created_at", default)]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt", alias = "updated_at", default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// Fields that go into the `rM-Meta` base64 JSON.
#[derive(Debug, Clone, Serialize, Default)]
struct Meta<'a> {
    #[serde(rename = "file_name")]
    file_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orientation: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    convert: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CloudClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    source: String,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    files: Vec<FileItem>,
}

impl CloudClient {
    pub fn new(token: impl Into<String>, base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("rr/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(Error::Network)?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            token: token.into(),
            source: DEFAULT_SOURCE.to_string(),
        })
    }

    /// Build a client from a user token plus tectonic claim.
    pub fn from_token_and_tectonic(
        token: impl Into<String>,
        tectonic: Option<&str>,
    ) -> Result<Self> {
        Self::new(token, resolve_base_url(tectonic))
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    fn base_headers(&self, meta: Option<&str>) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        let bearer = format!("Bearer {}", self.token);
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&bearer)
                .map_err(|e| Error::Config(format!("bad bearer header: {e}")))?,
        );
        h.insert(
            HeaderName::from_static("rm-source"),
            HeaderValue::from_str(&self.source)
                .map_err(|e| Error::Config(format!("bad rm-source: {e}")))?,
        );
        if let Some(m) = meta {
            h.insert(
                HeaderName::from_static("rm-meta"),
                HeaderValue::from_str(m).map_err(|e| Error::Config(format!("bad rm-meta: {e}")))?,
            );
        }
        Ok(h)
    }

    fn encode_meta(
        name: &str,
        parent: Option<&str>,
        orientation: Option<&str>,
        convert: Option<bool>,
    ) -> String {
        let m = Meta {
            file_name: name,
            parent,
            orientation,
            convert,
        };
        let json = serde_json::to_string(&m).unwrap_or_else(|_| "{}".to_string());
        BASE64.encode(json.as_bytes())
    }

    /// `POST /doc/v2/files` — upload an EPUB so it lands on the device
    /// as a native EPUB-rendered document (no .rm conversion). Returns the
    /// server's view of the resulting item.
    pub async fn upload_document(
        &self,
        file_name: &str,
        epub_bytes: Vec<u8>,
        parent: Option<&str>,
    ) -> Result<FileItem> {
        let url = self.url("/doc/v2/files");
        let meta = Self::encode_meta(file_name, parent, Some(DEFAULT_ORIENTATION), None);
        let mut headers = self.base_headers(Some(&meta))?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/epub+zip"),
        );
        let resp = self
            .http
            .post(url)
            .headers(headers)
            .body(epub_bytes)
            .send()
            .await
            .map_err(Error::Network)?;
        Self::parse_response(resp).await
    }

    /// `POST /import/v1/files` with `convert: true` — upload an EPUB and
    /// have the cloud convert it server-side into a **native reMarkable
    /// notebook** (the yellow-icon document type). This is the default
    /// path used by the extension's "Notebook" file format.
    pub async fn import_as_notebook(
        &self,
        file_name: &str,
        epub_bytes: Vec<u8>,
        parent: Option<&str>,
    ) -> Result<FileItem> {
        let url = self.url("/import/v1/files");
        let meta = Self::encode_meta(file_name, parent, Some(DEFAULT_ORIENTATION), Some(true));
        let mut headers = self.base_headers(Some(&meta))?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/epub+zip"),
        );
        let resp = self
            .http
            .post(url)
            .headers(headers)
            .body(epub_bytes)
            .send()
            .await
            .map_err(Error::Network)?;
        Self::parse_response(resp).await
    }

    /// Create an empty folder. Mirrors `DocumentStorage.createFolder` in
    /// the extension: empty body, `Content-Type: folder`.
    pub async fn create_folder(&self, name: &str, parent: Option<&str>) -> Result<FileItem> {
        let url = self.url("/doc/v2/files");
        let meta = Self::encode_meta(name, parent, None, None);
        let mut headers = self.base_headers(Some(&meta))?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("folder"),
        );
        let resp = self
            .http
            .post(url)
            .headers(headers)
            .body(Vec::<u8>::new())
            .send()
            .await
            .map_err(Error::Network)?;
        Self::parse_response(resp).await
    }

    /// `GET /doc/v2/files[?onlyFolders=true]`.
    pub async fn list_files(&self, only_folders: bool) -> Result<Vec<FileItem>> {
        let url = self.url("/doc/v2/files");
        let resp = self
            .http
            .get(url)
            .headers(self.base_headers(None)?)
            .query(&[("onlyFolders", only_folders.to_string())])
            .send()
            .await
            .map_err(Error::Network)?;

        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Vec::new());
        }
        if !status.is_success() {
            return Err(api_error(status, resp).await);
        }
        let parsed: ListResponse = resp.json().await.map_err(Error::Network)?;
        Ok(parsed.files)
    }

    /// `DELETE /doc/v2/files` with `{ "hashes": [...] }`.
    pub async fn delete_many(&self, hashes: &[String]) -> Result<()> {
        let url = self.url("/doc/v2/files");
        let body = serde_json::json!({ "hashes": hashes });
        let mut headers = self.base_headers(None)?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let resp = self
            .http
            .request(Method::DELETE, url)
            .headers(headers)
            .body(body.to_string())
            .send()
            .await
            .map_err(Error::Network)?;
        if !resp.status().is_success() {
            return Err(api_error(resp.status(), resp).await);
        }
        Ok(())
    }

    /// Find an existing folder by name and (optional) parent. Folder names
    /// are not unique on reMarkable, so callers may filter by parent to
    /// disambiguate.
    pub async fn find_folder(&self, name: &str, parent: Option<&str>) -> Result<Option<FileItem>> {
        let files = self.list_files(true).await?;
        Ok(files.into_iter().find(|f| {
            f.file_name == name
                && match (parent, f.parent.as_deref()) {
                    (Some(p), Some(fp)) => p == fp,
                    (None, None) | (None, Some(_)) => true,
                    (Some(_), None) => false,
                }
        }))
    }

    /// Resolve a slash-delimited path like `Work/Projects` to its deepest
    /// folder id, creating intermediate folders as needed.
    pub async fn resolve_or_create_path(&self, path: &str) -> Result<String> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(Error::Other("empty folder path".into()));
        }
        let mut parent: Option<String> = None;
        for part in parts {
            let existing = self.find_folder(part, parent.as_deref()).await?;
            let next = match existing {
                Some(f) => f.id,
                None => {
                    let created = self.create_folder(part, parent.as_deref()).await?;
                    created.id
                }
            };
            parent = Some(next);
        }
        parent.ok_or_else(|| Error::Other("path resolution failed".into()))
    }

    fn url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let p = if path.starts_with('/') {
            path
        } else {
            return format!("{base}/{path}");
        };
        format!("{base}{p}")
    }

    async fn parse_response(resp: reqwest::Response) -> Result<FileItem> {
        let status = resp.status();
        if !status.is_success() {
            return Err(api_error(status, resp).await);
        }
        // Some endpoints return an empty body (early API versions). Treat
        // that as an unnamed success and let callers cope.
        let text = resp.text().await.map_err(Error::Network)?;
        if text.trim().is_empty() {
            return Ok(FileItem {
                id: String::new(),
                hash: String::new(),
                file_type: String::new(),
                file_name: String::new(),
                created_at: None,
                updated_at: None,
                parent: None,
                pinned: None,
            });
        }
        serde_json::from_str(&text)
            .map_err(|e| Error::InvalidResponse(format!("decode upload response: {e} :: {text}")))
    }
}

async fn api_error(status: StatusCode, resp: reqwest::Response) -> Error {
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let body = resp.text().await.unwrap_or_default();
    match status {
        StatusCode::UNAUTHORIZED => Error::AuthExpired,
        StatusCode::TOO_MANY_REQUESTS => Error::RateLimited {
            retry_after_secs: retry_after.unwrap_or(1),
        },
        _ => Error::Api {
            status: status.as_u16(),
            body,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tectonic_resolution() {
        assert_eq!(
            resolve_base_url(Some("eu")),
            "https://web.eu.tectonic.remarkable.com"
        );
        assert_eq!(
            resolve_base_url(Some("us")),
            "https://web.us.tectonic.remarkable.com"
        );
        assert_eq!(resolve_base_url(None), DEFAULT_BASE_URL);
        assert_eq!(resolve_base_url(Some("")), DEFAULT_BASE_URL);
        assert_eq!(resolve_base_url(Some("  ")), DEFAULT_BASE_URL);
    }

    #[test]
    fn meta_round_trip_base64() {
        let m = CloudClient::encode_meta("Hello.epub", Some("p1"), Some("portrait"), Some(true));
        let bytes = BASE64.decode(&m).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["file_name"], "Hello.epub");
        assert_eq!(v["parent"], "p1");
        assert_eq!(v["orientation"], "portrait");
        assert_eq!(v["convert"], true);
    }

    #[test]
    fn meta_omits_none_fields() {
        let m = CloudClient::encode_meta("Hello.epub", None, None, None);
        let bytes = BASE64.decode(&m).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["file_name"], "Hello.epub");
        assert!(v.get("parent").is_none() || v["parent"].is_null());
        assert!(v.get("orientation").is_none() || v["orientation"].is_null());
        assert!(v.get("convert").is_none() || v["convert"].is_null());
    }
}

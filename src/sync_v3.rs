use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use base64::Engine;

const SYNC_HOST: &str = "https://internal.cloud.remarkable.com";
const ROOT_URL: &str = "https://internal.cloud.remarkable.com/sync/v3/root";
const FILES_URL: &str = "https://internal.cloud.remarkable.com/sync/v3/files";

/// A file/folder entry from the sync index
#[derive(Debug, Clone)]
pub struct SyncEntry {
    pub hash: String,
    pub doc_id: String,
    pub name: String,
    pub entry_type: String, // "DocumentType" or "CollectionType"
    pub parent: Option<String>,
    pub size: i64,
}

/// Sync v3 API client
#[derive(Clone)]
pub struct SyncClient {
    client: Client,
    token: String,
}

/// Root pointer response
#[derive(Debug, Deserialize)]
struct RootResponse {
    hash: String,
    generation: i64,
}

impl SyncClient {
    pub fn new(token: String) -> Self {
        Self {
            client: Client::new(),
            token,
        }
    }

    /// List all files and folders from sync v3 (quick mode - just doc IDs)
    pub async fn list_files_quick(&self) -> Result<Vec<SyncEntry>> {
        let root = self.get_root().await?;
        let index_body = self.fetch_blob(&root.hash, "root.docSchema").await?;
        let entries = parse_index(&index_body)?;
        
        let result = entries.into_iter().map(|e| SyncEntry {
            hash: e.hash,
            doc_id: e.doc_id.clone(),
            name: e.doc_id.clone(),  // Use ID as name for now
            entry_type: "DocumentType".to_string(),
            parent: None,
            size: e.size,
        }).collect();
        
        Ok(result)
    }
    
    /// List all files with metadata (slower but shows actual names)
    pub async fn list_files(&self) -> Result<Vec<SyncEntry>> {
        let root = self.get_root().await?;
        let index_body = self.fetch_blob(&root.hash, "root.docSchema").await?;
        let entries = parse_index(&index_body)?;
        
        println!("  Found {} entries, fetching metadata...", entries.len());
        
        // Fetch metadata concurrently with limit
        let mut result = Vec::new();
        use futures::stream::{self, StreamExt};
        
        let futures = entries.into_iter().map(|entry| {
            let client = self.clone();
            async move {
                let meta = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    client.fetch_metadata(&entry.hash, &entry.doc_id)
                ).await;
                
                match meta {
                    Ok(Ok(meta)) => SyncEntry {
                        hash: entry.hash,
                        doc_id: entry.doc_id.clone(),
                        name: meta.visible_name,
                        entry_type: meta.doc_type,
                        parent: meta.parent,
                        size: entry.size,
                    },
                    _ => SyncEntry {
                        hash: entry.hash,
                        doc_id: entry.doc_id.clone(),
                        name: entry.doc_id.clone(),
                        entry_type: "DocumentType".to_string(),
                        parent: None,
                        size: entry.size,
                    }
                }
            }
        });
        
        let mut stream = stream::iter(futures).buffer_unordered(10);
        while let Some(entry) = stream.next().await {
            result.push(entry);
        }
        
        Ok(result)
    }

    /// Get root pointer
    async fn get_root(&self) -> Result<RootResponse> {
        let resp = self.client
            .get(ROOT_URL)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| "Failed to fetch root")?;
        
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Root fetch failed: HTTP {} - {}", status, text);
        }
        
        let root: RootResponse = resp.json().await
            .with_context(|| "Failed to parse root response")?;
        
        Ok(root)
    }

    /// Fetch a blob by hash
    /// Note: The server requires 'rm-filename' header even for GET requests!
    /// The filename must match the expected pattern: {docID}.docSchema or {docID}.metadata
    async fn fetch_blob(&self, hash: &str, filename: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}", FILES_URL, hash);
        let resp = self.client
            .get(&url)
            .bearer_auth(&self.token)
            .header("rm-filename", filename)
            .send()
            .await
            .with_context(|| format!("Failed to fetch blob {}", hash))?;
        
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Blob fetch failed: HTTP {} - {}", status, text);
        }
        
        let bytes = resp.bytes().await
            .with_context(|| "Failed to read blob bytes")?;
        
        Ok(bytes.to_vec())
    }

    /// Fetch metadata for a document
    async fn fetch_metadata(&self, doc_hash: &str, doc_id: &str) -> Result<Metadata> {
        // Fetch doc index to find metadata hash
        let doc_index = self.fetch_blob(doc_hash, &format!("{}.docSchema", doc_id)).await?;
        let meta_hash = find_file_hash(&doc_index, &format!("{}.metadata", doc_id));
        
        if meta_hash.is_empty() {
            anyhow::bail!("No metadata found for {}", doc_id);
        }
        
        // Fetch metadata blob
        let meta_bytes = self.fetch_blob(&meta_hash, &format!("{}.metadata", doc_id)).await?;
        let meta: Metadata = serde_json::from_slice(&meta_bytes)
            .with_context(|| "Failed to parse metadata")?;
        
        Ok(meta)
    }

    /// Upload a file to sync v3
    pub async fn upload_file(
        &self,
        doc_id: &str,
        name: &str,
        content_type: &str, // "pdf", "epub", "html"
        content: Vec<u8>,
        parent: Option<String>,
    ) -> Result<()> {
        // Retry once on 412 (generation race)
        for attempt in 0..2 {
            match self.upload_file_attempt(doc_id, name, content_type, &content, parent.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt == 0 && e.to_string().contains("412") => {
                    println!("Root generation race detected, retrying...");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Single attempt at uploading a file
    async fn upload_file_attempt(
        &self,
        doc_id: &str,
        name: &str,
        content_type: &str,
        content: &[u8],
        parent: Option<String>,
    ) -> Result<()> {
        // 1. Get current root
        let root = self.get_root().await?;
        let root_index = self.fetch_blob(&root.hash, "root.docSchema").await?;
        let mut entries = parse_index(&root_index)?;
        
        // 2. Create metadata with current timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        
        let meta = Metadata {
            doc_type: "DocumentType".to_string(),
            visible_name: name.to_string(),
            parent,
            last_modified: Some(now),
            pinned: None,
            modified: Some(true),
        };
        let meta_json = serde_json::to_vec(&meta)?;
        let meta_hash = sha256_hex(&meta_json);
        
        // 3. Create content info blob
        let content_info = json!({"fileType": content_type});
        let content_info_json = serde_json::to_vec(&content_info)?;
        let content_info_hash = sha256_hex(&content_info_json);
        
        // 4. Upload content info blob
        self.put_blob(&content_info_hash, &content_info_json, &format!("{}.content", doc_id)).await?;
        
        // 5. Upload actual file content blob
        let content_hash = sha256_hex(content);
        self.put_blob(&content_hash, content, &format!("{}.{}", doc_id, content_type)).await?;
        
        // 6. Upload metadata blob
        self.put_blob(&meta_hash, &meta_json, &format!("{}.metadata", doc_id)).await?;
        
        // 7. Build doc index
        let doc_entries = vec![
            IndexEntry {
                hash: content_hash,
                doc_id: format!("{}.{}", doc_id, content_type),
                subfiles: 0,
                size: content.len() as i64,
            },
            IndexEntry {
                hash: content_info_hash,
                doc_id: format!("{}.content", doc_id),
                subfiles: 0,
                size: content_info_json.len() as i64,
            },
            IndexEntry {
                hash: meta_hash,
                doc_id: format!("{}.metadata", doc_id),
                subfiles: 0,
                size: meta_json.len() as i64,
            },
        ];
        let doc_index_body = serialize_index("3", doc_id, &doc_entries);
        let doc_index_hash = hash_index("3", &doc_entries, &doc_index_body);
        
        // 8. Upload doc index
        self.put_blob(&doc_index_hash, &doc_index_body, &format!("{}.docSchema", doc_id)).await?;
        
        // 9. Update root
        let new_root_entry = IndexEntry {
            hash: doc_index_hash,
            doc_id: doc_id.to_string(),
            subfiles: 3,
            size: doc_entries.iter().map(|e| e.size).sum(),
        };
        
        // Replace or append entry
        let mut found = false;
        for entry in &mut entries {
            if entry.doc_id == doc_id {
                *entry = new_root_entry.clone();
                found = true;
                break;
            }
        }
        if !found {
            entries.push(new_root_entry);
        }
        
        // Build new root index
        let root_index_body = serialize_index("3", ".", &entries);
        let root_index_hash = hash_index("3", &entries, &root_index_body);
        
        // Upload root index
        self.put_blob(&root_index_hash, &root_index_body, "root.docSchema").await?;
        
        // Update root pointer
        self.update_root(&root_index_hash, root.generation).await?;
        
        Ok(())
    }

    /// Create a folder in sync v3
    pub async fn create_folder(
        &self,
        folder_id: &str,
        name: &str,
        parent: Option<String>,
    ) -> Result<()> {
        // Retry once on 412 (generation race)
        for attempt in 0..2 {
            match self.create_folder_attempt(folder_id, name, parent.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt == 0 && e.to_string().contains("412") => {
                    println!("Root generation race detected, retrying...");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Single attempt at creating a folder
    async fn create_folder_attempt(
        &self,
        folder_id: &str,
        name: &str,
        parent: Option<String>,
    ) -> Result<()> {
        // 1. Get current root
        let root = self.get_root().await?;
        let root_index = self.fetch_blob(&root.hash, "root.docSchema").await?;
        let mut entries = parse_index(&root_index)?;
        
        // 2. Create metadata with current timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        
        let meta = Metadata {
            doc_type: "CollectionType".to_string(),
            visible_name: name.to_string(),
            parent,
            last_modified: Some(now),
            pinned: None,
            modified: Some(true),
        };
        let meta_json = serde_json::to_vec(&meta)?;
        let meta_hash = sha256_hex(&meta_json);
        
        // 3. Upload metadata blob
        self.put_blob(&meta_hash, &meta_json, &format!("{}.metadata", folder_id)).await?;
        
        // 4. Build doc index (only metadata for folders)
        let doc_entries = vec![
            IndexEntry {
                hash: meta_hash,
                doc_id: format!("{}.metadata", folder_id),
                subfiles: 0,
                size: meta_json.len() as i64,
            },
        ];
        let doc_index_body = serialize_index("3", folder_id, &doc_entries);
        let doc_index_hash = hash_index("3", &doc_entries, &doc_index_body);
        
        // 5. Upload doc index
        self.put_blob(&doc_index_hash, &doc_index_body, &format!("{}.docSchema", folder_id)).await?;
        
        // 6. Update root
        let new_root_entry = IndexEntry {
            hash: doc_index_hash,
            doc_id: folder_id.to_string(),
            subfiles: 1,
            size: doc_entries.iter().map(|e| e.size).sum(),
        };
        
        // Replace or append entry
        let mut found = false;
        for entry in &mut entries {
            if entry.doc_id == folder_id {
                *entry = new_root_entry.clone();
                found = true;
                break;
            }
        }
        if !found {
            entries.push(new_root_entry);
        }
        
        // Build new root index
        let root_index_body = serialize_index("3", ".", &entries);
        let root_index_hash = hash_index("3", &entries, &root_index_body);
        
        // Upload root index
        self.put_blob(&root_index_hash, &root_index_body, "root.docSchema").await?;
        
        // Update root pointer
        self.update_root(&root_index_hash, root.generation).await?;
        
        Ok(())
    }

    /// Find a folder by name and return its ID
    pub async fn find_folder(&self, name: &str) -> Result<Option<String>> {
        let files = self.list_files().await?;
        
        for entry in &files {
            if entry.entry_type == "CollectionType" && entry.name == name {
                return Ok(Some(entry.doc_id.clone()));
            }
        }
        
        Ok(None)
    }

    /// List all folders with their parent info
    pub async fn list_folders(&self) -> Result<Vec<SyncEntry>> {
        let files = self.list_files().await?;
        
        let folders: Vec<SyncEntry> = files
            .into_iter()
            .filter(|f| f.entry_type == "CollectionType")
            .collect();
        
        Ok(folders)
    }

    /// Upload a blob
    async fn put_blob(&self, hash: &str, data: &[u8], filename: &str) -> Result<()> {
        let url = format!("{}/{}", FILES_URL, hash);
        
        // Calculate CRC32C checksum as required by the server
        let crc = crc32c::crc32c(data);
        let crc_bytes = crc.to_be_bytes();
        let crc_b64 = base64::engine::general_purpose::STANDARD.encode(crc_bytes);
        let goog_hash = format!("crc32c={}", crc_b64);
        
        let resp = self.client
            .put(&url)
            .bearer_auth(&self.token)
            .header("rm-filename", filename)
            .header("rm-filesize", data.len().to_string())
            .header("x-goog-hash", &goog_hash)
            .body(data.to_vec())
            .send()
            .await
            .with_context(|| format!("Failed to upload blob {}", hash))?;
        
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Blob upload failed: HTTP {} - {}", status, text);
        }
        
        Ok(())
    }

    /// Update root pointer atomically
    async fn update_root(&self, hash: &str, generation: i64) -> Result<()> {
        let body = json!({
            "broadcast": true,
            "hash": hash,
            "generation": generation,
        });
        
        let resp = self.client
            .put(ROOT_URL)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .header("rm-filename", "roothash")
            .json(&body)
            .send()
            .await
            .with_context(|| "Failed to update root")?;
        
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Root update failed: HTTP {} - {}", status, text);
        }
        
        Ok(())
    }
}

/// Parsed index entry
#[derive(Debug, Clone)]
struct IndexEntry {
    hash: String,
    doc_id: String,
    subfiles: i32,
    size: i64,
}

/// Parse sync v3 index blob
fn parse_index(body: &[u8]) -> Result<Vec<IndexEntry>> {
    let text = String::from_utf8_lossy(body);
    let mut entries = Vec::new();
    
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            // First line is schema version
            continue;
        }
        
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 5 {
            continue;
        }
        
        let size = parts[4].parse::<i64>().unwrap_or(0);
        let subfiles = parts[3].parse::<i32>().unwrap_or(0);
        
        entries.push(IndexEntry {
            hash: parts[0].to_string(),
            doc_id: parts[2].to_string(),
            subfiles,
            size,
        });
    }
    
    Ok(entries)
}

/// Serialize entries to index blob
fn serialize_index(schema: &str, label: &str, entries: &[IndexEntry]) -> Vec<u8> {
    let mut result = String::new();
    result.push_str(schema);
    result.push('\n');
    
    // Sort entries by docID (required for v3 hashing)
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
    
    // v4: emit totals row
    if schema == "4" && !label.is_empty() {
        let total: i64 = sorted.iter().map(|e| e.size).sum();
        result.push_str(&format!("0:{}:{}:{}\n", label, sorted.len(), total));
    }
    
    // v3 root uses 80000000 type, everything else uses 0
    let type_field = if schema == "3" && label == "." { "80000000" } else { "0" };
    
    for entry in &sorted {
        result.push_str(&format!(
            "{}:{}:{}:{}:{}\n",
            entry.hash, type_field, entry.doc_id, entry.subfiles, entry.size
        ));
    }
    
    result.into_bytes()
}

/// Compute index hash according to sync v3/v4 rules
/// v3: hash of concatenated child hash bytes (sorted by docID)
/// v4: SHA-256 of the serialized body
fn hash_index(schema: &str, entries: &[IndexEntry], body: &[u8]) -> String {
    if schema == "4" {
        sha256_hex(body)
    } else {
        // v3: sort by docID, concatenate raw hash bytes, then SHA-256
        let mut sorted: Vec<_> = entries.to_vec();
        sorted.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
        
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for entry in &sorted {
            let raw = hex::decode(&entry.hash).unwrap_or_default();
            hasher.update(&raw);
        }
        hex::encode(hasher.finalize())
    }
}

/// Find file hash in index
fn find_file_hash(index: &[u8], filename: &str) -> String {
    let text = String::from_utf8_lossy(index);
    for line in text.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[2] == filename {
            return parts[0].to_string();
        }
    }
    String::new()
}

/// SHA256 hex
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Metadata structure (matches reMarkable's format)
#[derive(Debug, Serialize, Deserialize)]
struct Metadata {
    #[serde(rename = "type")]
    doc_type: String,
    #[serde(rename = "visibleName")]
    visible_name: String,
    #[serde(rename = "parent")]
    parent: Option<String>,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(rename = "pinned", skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    #[serde(rename = "modified", skip_serializing_if = "Option::is_none")]
    modified: Option<bool>,
}

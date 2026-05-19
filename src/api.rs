use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::{header::HeaderMap, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

const SOURCE_HEADER: &str = "rM-Source";
const META_HEADER: &str = "rM-Meta";
const SOURCE_VALUE: &str = "rr-CLI";

#[derive(Debug, Clone)]
pub struct RemarkableApi {
    client: Client,
    token: String,
    base_url: String,
    import_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileItem {
    pub id: String,
    pub hash: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub file_name: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub parent: Option<String>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileListResponse {
    pub files: Vec<FileItem>,
}

#[derive(Debug, Serialize)]
struct UploadMetadata {
    file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orientation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    convert: Option<bool>,
}

#[derive(Debug)]
pub struct UploadOptions {
    pub parent: String,
    pub as_notebook: bool,
    pub title: String,
}

impl RemarkableApi {
    pub fn new(token: String, base_url: String) -> Self {
        let import_url = base_url.clone();
        Self {
            client: Client::new(),
            token,
            base_url,
            import_url,
        }
    }

    pub fn with_tectonic(token: String, tectonic: &str) -> Self {
        let base_url = format!("https://web.{}.tectonic.remarkable.com", tectonic);
        Self::new(token, base_url)
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        headers.insert(SOURCE_HEADER, SOURCE_VALUE.parse().unwrap());
        headers
    }

    fn encode_metadata(&self, meta: &UploadMetadata) -> String {
        let json = serde_json::to_string(meta).unwrap();
        BASE64.encode(json.as_bytes())
    }

    /// Upload a document (EPUB or PDF)
    pub async fn upload_document(
        &self,
        name: &str,
        content: Vec<u8>,
        options: &UploadOptions,
    ) -> Result<FileItem> {
        let url = format!("{}/doc/v2/files", self.base_url);

        let meta = UploadMetadata {
            file_name: options.title.clone(),
            parent: if options.parent.is_empty() {
                None
            } else {
                Some(options.parent.clone())
            },
            orientation: Some("portrait".to_string()),
            convert: None,
        };

        let mut headers = self.auth_headers();
        headers.insert(META_HEADER, self.encode_metadata(&meta).parse().unwrap());

        let part = reqwest::multipart::Part::bytes(content)
            .file_name(name.to_string())
            .mime_str("application/epub+zip")?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let res = self
            .client
            .post(&url)
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("Failed to upload document to {}", url))?;

        self.handle_response(res).await
    }

    /// Import a file with conversion (HTML → Notebook)
    pub async fn import_file(
        &self,
        name: &str,
        content: Vec<u8>,
        options: &UploadOptions,
    ) -> Result<FileItem> {
        let url = format!("{}/import/v1/files", self.import_url);

        let meta = UploadMetadata {
            file_name: options.title.clone(),
            parent: if options.parent.is_empty() {
                None
            } else {
                Some(options.parent.clone())
            },
            orientation: Some("portrait".to_string()),
            convert: Some(true),
        };

        let mut headers = self.auth_headers();
        headers.insert(META_HEADER, self.encode_metadata(&meta).parse().unwrap());

        let part = reqwest::multipart::Part::bytes(content)
            .file_name(name.to_string())
            .mime_str("text/html")?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let res = self
            .client
            .post(&url)
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("Failed to import file to {}", url))?;

        self.handle_response(res).await
    }

    /// Upload HTML content (converted to EPUB or Notebook)
    pub async fn upload_html(
        &self,
        title: &str,
        html_content: &str,
        options: &UploadOptions,
    ) -> Result<FileItem> {
        if options.as_notebook {
            // For notebook, upload as HTML with convert=true via import endpoint
            self.import_file(
                &format!("{}.html", title),
                html_content.as_bytes().to_vec(),
                options,
            )
            .await
        } else {
            // For EPUB, we need to create an EPUB from HTML
            // For now, upload as HTML - the API might handle it
            // Actually, let's just upload as HTML to the document endpoint
            // The server should handle conversion
            self.upload_document(
                &format!("{}.html", title),
                html_content.as_bytes().to_vec(),
                options,
            )
            .await
        }
    }

    /// List all files and folders
    pub async fn list_files(&self, only_folders: bool) -> Result<Vec<FileItem>> {
        let url = format!("{}/doc/v2/files", self.base_url);

        let mut query = HashMap::new();
        query.insert("onlyFolders", only_folders.to_string());

        let res = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .query(&query)
            .send()
            .await
            .with_context(|| "Failed to list files")?;

        let status = res.status();

        if status == StatusCode::NOT_MODIFIED {
            return Ok(vec![]);
        }

        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("List files failed: HTTP {} - {}", status, text);
        }

        let response: FileListResponse = res
            .json()
            .await
            .with_context(|| "Failed to parse file list")?;

        Ok(response.files)
    }

    /// Delete files by hash
    pub async fn delete_files(&self, hashes: Vec<String>) -> Result<()> {
        let url = format!("{}/doc/v2/files", self.base_url);

        let body = json!({ "hashes": hashes });

        let res = self
            .client
            .delete(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .with_context(|| "Failed to delete files")?;

        let status = res.status();

        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("Delete failed: HTTP {} - {}", status, text);
        }

        Ok(())
    }

    async fn handle_response(&self, res: reqwest::Response) -> Result<FileItem> {
        let status = res.status();

        match status {
            StatusCode::UNAUTHORIZED => {
                anyhow::bail!("Authentication expired. Please run 'rr auth' again.")
            }
            StatusCode::FORBIDDEN => {
                anyhow::bail!("Access forbidden. Check your reMarkable subscription.")
            }
            StatusCode::CONFLICT => {
                anyhow::bail!("File conflict. File may already exist.")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                anyhow::bail!("Rate limited. Please try again later.")
            }
            _ => {}
        }

        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("API error: HTTP {} - {}", status, text);
        }

        let file: FileItem = res
            .json()
            .await
            .with_context(|| "Failed to parse API response")?;

        Ok(file)
    }
}

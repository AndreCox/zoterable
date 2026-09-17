use std::fs;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::config;

/// Register this machine with the reMarkable cloud and store the device token.
pub fn pair(code: &str) -> Result<()> {
    let endpoints = config::remarkable_endpoints()?;
    let response = Client::new()
        .post(format!("{}/token/json/2/device/new", endpoints.auth_host))
        .bearer_auth("")
        .json(&serde_json::json!({
            "code": code,
            "deviceDesc": "browser-chrome",
            "deviceID": uuid::Uuid::new_v4().to_string(),
        }))
        .send()?
        .error_for_status()
        .context("device registration failed — one-time codes expire quickly, get a fresh one")?;
    let token = response.text()?;
    let path = config::device_token_path()?;
    fs::write(&path, &token)?;
    println!(
        "Paired with the reMarkable cloud (token stored in {}).",
        path.display()
    );
    Ok(())
}

pub struct Remarkable {
    client: Client,
    session_token: String,
    upload_host: String,
    storage_host: String,
}

#[derive(Clone, Deserialize)]
pub struct FileEntry {
    #[serde(alias = "docID", alias = "id", alias = "ID")]
    pub id: String,
    #[serde(default, alias = "Hash", alias = "hash")]
    pub hash: String,
    #[serde(default, alias = "fileName", alias = "VissibleName")]
    pub file_name: String,
    #[serde(default, alias = "type", alias = "Type")]
    pub entry_type: String,
    #[serde(default, alias = "Parent", alias = "parent")]
    pub parent: String,
}

#[derive(Deserialize)]
struct DownloadRef {
    #[serde(alias = "ID", alias = "id")]
    id: String,
    #[serde(default, alias = "BlobURL", alias = "BlobURLGet")]
    blob_url: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FilesResponse {
    Array(Vec<FileEntry>),
    Wrapped { files: Vec<FileEntry> },
}

impl Remarkable {
    /// Exchange the stored device token for a fresh session token.
    pub fn connect() -> Result<Self> {
        let endpoints = config::remarkable_endpoints()?;
        let path = config::device_token_path()?;
        let device_token = fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {} — run `zoterable pair <code>` first",
                path.display()
            )
        })?;
        let client = Client::new();
        let session_token = client
            .post(format!("{}/token/json/2/user/new", endpoints.auth_host))
            .bearer_auth(device_token.trim())
            // The auth frontend rejects bodiless POSTs with 411 Length
            // Required; an empty body forces a Content-Length: 0 header.
            .body("")
            .send()?
            .error_for_status()
            .context("could not refresh the reMarkable session token — try re-pairing")?
            .text()?;
        Ok(Self {
            client,
            session_token,
            upload_host: endpoints.upload_host,
            storage_host: endpoints.storage_host,
        })
    }

    /// Upload a PDF to the reMarkable cloud, optionally into a folder.
    pub fn upload_pdf(
        &self,
        visible_name: &str,
        bytes: Vec<u8>,
        parent: Option<&str>,
    ) -> Result<FileEntry> {
        let mut meta_json = serde_json::json!({ "file_name": visible_name });
        if let Some(parent) = parent.filter(|p| !p.is_empty()) {
            meta_json["parent"] = serde_json::Value::String(parent.to_string());
        }
        let meta = BASE64.encode(meta_json.to_string());
        Ok(self
            .client
            .post(format!("{}/doc/v2/files", self.upload_host))
            .bearer_auth(&self.session_token)
            .header("content-type", "application/pdf")
            .header("rm-meta", meta)
            .header("rm-source", "RoR-Browser")
            .body(bytes)
            .send()?
            .error_for_status()
            .with_context(|| format!("upload of {visible_name:?} failed"))?
            .json()?)
    }

    pub fn create_folder(&self, folder_name: &str, parent: Option<&str>) -> Result<FileEntry> {
        let mut meta_json = serde_json::json!({ "file_name": folder_name });
        if let Some(parent) = parent.filter(|p| !p.is_empty()) {
            meta_json["parent"] = serde_json::Value::String(parent.to_string());
        }
        let meta = BASE64.encode(meta_json.to_string());
        Ok(self
            .client
            .post(format!("{}/doc/v2/files", self.upload_host))
            .bearer_auth(&self.session_token)
            .header("content-type", "folder")
            .header("content-length", "0")
            .header("rm-meta", meta)
            .header("rm-source", "RoR-Browser")
            .body("")
            .send()?
            .error_for_status()
            .with_context(|| format!("folder creation failed for {folder_name:?}"))?
            .json()?)
    }

    pub fn list_files(&self) -> Result<Vec<FileEntry>> {
        let response = self
            .client
            .get(format!("{}/doc/v2/files", self.upload_host))
            .bearer_auth(&self.session_token)
            .header("rm-source", "RoR-Browser")
            .send()?
            .error_for_status()
            .context("could not list reMarkable files")?;
        let body = response.text()?;
        let parsed: FilesResponse = serde_json::from_str(&body)
            .with_context(|| format!("unexpected reMarkable listing response: {body}"))?;
        Ok(match parsed {
            FilesResponse::Array(files) => files,
            FilesResponse::Wrapped { files } => files,
        })
    }

    pub fn download_zip(&self, doc_id: &str) -> Result<Vec<u8>> {
        let refs: Vec<DownloadRef> = self
            .client
            .post(format!(
                "{}/document-storage/json/2/docs/download?withBlob=true",
                self.storage_host
            ))
            .bearer_auth(&self.session_token)
            .header("content-type", "application/json")
            .json(&vec![serde_json::json!({ "ID": doc_id, "Version": 1 })])
            .send()?
            .error_for_status()
            .with_context(|| format!("could not get download URL for reMarkable doc {doc_id}"))?
            .json()?;
        let Some(item) = refs.into_iter().find(|item| item.id == doc_id) else {
            bail!("reMarkable did not return download info for document {doc_id}");
        };
        let blob_url = item.blob_url.trim();
        if blob_url.is_empty() {
            bail!("reMarkable did not provide a blob URL for document {doc_id}");
        }
        Ok(self
            .client
            .get(blob_url)
            .send()?
            .error_for_status()
            .with_context(|| format!("could not download reMarkable document blob {doc_id}"))?
            .bytes()?
            .to_vec())
    }
}

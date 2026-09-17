use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;

const API: &str = "https://api.zotero.org";
const PAGE_SIZE: usize = 100;
/// How many times to retry a request that returned 429/503 before giving up.
const MAX_RETRIES: u32 = 5;

pub struct Zotero {
    client: Client,
    api_key: String,
    /// When set, the next request is held until this instant to honor a
    /// `Backoff` header the API returned on an earlier response.
    backoff_until: Cell<Option<Instant>>,
}

#[derive(Deserialize)]
pub struct Item {
    pub key: String,
    pub version: u64,
    pub data: ItemData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemData {
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub link_mode: Option<String>,
    #[serde(default)]
    pub parent_item: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub creators: Vec<Creator>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Creator {
    #[serde(default)]
    pub last_name: Option<String>,
    /// Single-field creator names (e.g. institutions).
    #[serde(default)]
    pub name: Option<String>,
}

impl Zotero {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.to_string(),
            backoff_until: Cell::new(None),
        }
    }

    /// `library` is an API path prefix: "users/<id>" or "groups/<id>".
    fn get(&self, library: &str, path_and_query: &str) -> RequestBuilder {
        self.client
            .get(format!("{API}/{library}/{path_and_query}"))
            .header("Zotero-API-Key", &self.api_key)
            .header("Zotero-API-Version", "3")
    }

    /// Send a request while honoring Zotero's rate-limit signals: wait out any
    /// pending `Backoff` before sending, retry on `429`/`503` after the
    /// server's `Retry-After` delay, and record any new `Backoff` for the next
    /// call. Returns the raw response; callers apply `error_for_status`.
    fn send(&self, builder: RequestBuilder) -> Result<Response> {
        if let Some(until) = self.backoff_until.take() {
            if let Some(remaining) = until.checked_duration_since(Instant::now()) {
                thread::sleep(remaining);
            }
        }

        let mut attempt = 0;
        loop {
            let request = builder
                .try_clone()
                .context("Zotero request could not be retried")?;
            let response = request.send()?;

            // A `Backoff` header asks us to slow down subsequent requests.
            if let Some(secs) = header_secs(&response, "backoff") {
                self.backoff_until
                    .set(Some(Instant::now() + Duration::from_secs(secs)));
            }

            let status = response.status();
            let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::SERVICE_UNAVAILABLE;
            if rate_limited && attempt < MAX_RETRIES {
                let wait = header_secs(&response, "retry-after").unwrap_or(1);
                eprintln!("Zotero rate-limited (HTTP {status}); waiting {wait}s before retrying…");
                thread::sleep(Duration::from_secs(wait));
                attempt += 1;
                continue;
            }
            return Ok(response);
        }
    }

    /// PDF attachments stored in the library that were added or modified after
    /// library version `since` (pass 0 for everything). Returns the items and
    /// the current library version, for use as the next `since`. Linked-file
    /// attachments are skipped because their content lives outside Zotero
    /// storage.
    pub fn pdf_attachments(&self, library: &str, since: u64) -> Result<(Vec<Item>, u64)> {
        let mut items: Vec<Item> = Vec::new();
        let mut library_version = since;
        let mut start = 0;
        loop {
            let response = self
                .send(self.get(
                    library,
                    &format!(
                        "items?itemType=attachment&since={since}&limit={PAGE_SIZE}&start={start}"
                    ),
                ))?
                .error_for_status()
                .with_context(|| {
                    format!(
                        "Zotero item listing for {library} failed — check the IDs in the config \
                         and, for groups, that the API key has group read access"
                    )
                })?;
            if let Some(version) = response
                .headers()
                .get("Last-Modified-Version")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
            {
                library_version = version;
            }
            let batch: Vec<Item> = response.json()?;
            let done = batch.len() < PAGE_SIZE;
            start += batch.len();
            items.extend(batch);
            if done {
                break;
            }
        }
        items.retain(|item| {
            item.data.content_type.as_deref() == Some("application/pdf")
                && matches!(
                    item.data.link_mode.as_deref(),
                    Some("imported_file" | "imported_url")
                )
        });
        Ok((items, library_version))
    }

    pub fn item(&self, library: &str, key: &str) -> Result<Item> {
        Ok(self
            .send(self.get(library, &format!("items/{key}")))?
            .error_for_status()
            .with_context(|| format!("could not fetch Zotero item {library}/{key}"))?
            .json()?)
    }

    /// Download an attachment's file content (follows the redirect to storage).
    /// Returns `None` when Zotero has no file stored for the attachment (404):
    /// the item exists but its PDF was never uploaded to Zotero's cloud, so
    /// there is nothing to fetch.
    pub fn download(&self, library: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let response = self.send(self.get(library, &format!("items/{key}/file")))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .with_context(|| format!("could not download attachment {library}/{key}"))?;
        Ok(Some(response.bytes()?.to_vec()))
    }
}

/// Parse a header whose value is a whole number of seconds. Zotero sends both
/// `Backoff` and `Retry-After` as integer seconds.
fn header_secs(response: &Response, name: &str) -> Option<u64> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
}

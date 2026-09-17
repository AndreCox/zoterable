use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::config;
use crate::remarkable::{FileEntry, Remarkable};
use crate::zotero::{Collection, Item, Zotero};

#[derive(Clone, Default, Serialize, Deserialize)]
struct RemarkableDocState {
    doc_id: String,
    #[serde(default)]
    last_hash: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct LibraryState {
    /// Zotero library version at the last completed sync; only items modified
    /// after this are fetched.
    #[serde(default)]
    last_library_version: u64,
    /// Zotero attachment key -> item version at last successful upload.
    #[serde(default)]
    synced: HashMap<String, u64>,
    /// Zotero attachment key -> reMarkable cloud document details.
    #[serde(default)]
    remarkable_docs: HashMap<String, RemarkableDocState>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Per-library sync state, keyed by API prefix ("users/…" or "groups/…").
    #[serde(default)]
    libraries: HashMap<String, LibraryState>,
    // Legacy fields from the single-library format; migrated into `libraries`
    // (under the personal library) on load.
    #[serde(default, skip_serializing_if = "is_zero")]
    last_library_version: u64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    synced: HashMap<String, u64>,
}

#[derive(Default)]
struct FolderIndex {
    by_parent_and_name: HashMap<(String, String), String>,
    docs_by_id: HashMap<String, FileEntry>,
}

#[derive(Default)]
struct ItemPlacement {
    name: String,
    folder_path: Vec<String>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl State {
    fn load(path: &Path, user_library: &str) -> Self {
        let mut state: State = fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if state.last_library_version != 0 || !state.synced.is_empty() {
            let lib = state.libraries.entry(user_library.to_string()).or_default();
            lib.last_library_version = lib.last_library_version.max(state.last_library_version);
            lib.synced.extend(std::mem::take(&mut state.synced));
            state.last_library_version = 0;
        }
        state
    }

    fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

impl FolderIndex {
    fn from_files(files: Vec<FileEntry>) -> Self {
        let mut index = Self::default();
        for file in files {
            if is_folder(&file) {
                index.by_parent_and_name.insert(
                    (file.parent.clone(), file.file_name.clone()),
                    file.id.clone(),
                );
            } else {
                index.docs_by_id.insert(file.id.clone(), file);
            }
        }
        index
    }
}

pub fn run(dry_run: bool) -> Result<()> {
    let cfg = config::load()?;
    let zotero = Zotero::new(&cfg.zotero_api_key);

    let state_path = config::state_path()?;
    let mut state = State::load(&state_path, &cfg.user_library());

    let mut remarkable: Option<Remarkable> = None;
    let mut folder_index: Option<FolderIndex> = None;
    let mut failures = 0usize;

    for library in cfg.libraries() {
        let mut lib = state.libraries.get(&library).cloned().unwrap_or_default();
        let collections_by_key: HashMap<String, Collection> = zotero
            .collections(&library)?
            .into_iter()
            .map(|collection| (collection.key.clone(), collection))
            .collect();

        println!(
            "[{library}] fetching PDF attachments changed since library version {}…",
            lib.last_library_version
        );
        let (attachments, library_version) =
            zotero.pdf_attachments(&library, lib.last_library_version)?;

        // Only never-seen attachments are uploaded. Re-uploading a known key
        // would create a duplicate document on the reMarkable, so metadata-only
        // edits just refresh the record.
        let (new, updated): (Vec<&Item>, Vec<&Item>) = attachments
            .iter()
            .partition(|item| !lib.synced.contains_key(&item.key));
        println!(
            "[{library}] {} changed attachment(s): {} new, {} already on the tablet.",
            attachments.len(),
            new.len(),
            updated.len()
        );

        if dry_run {
            for item in new {
                let placement = item_placement(&zotero, &library, item, &collections_by_key);
                if placement.folder_path.is_empty() {
                    println!("would upload: {}", placement.name);
                } else {
                    println!(
                        "would upload: {} (folder: {})",
                        placement.name,
                        placement.folder_path.join("/")
                    );
                }
            }
            continue;
        }

        for item in &updated {
            lib.synced.insert(item.key.clone(), item.version);
        }

        let mut lib_failures = 0usize;
        if !lib.remarkable_docs.is_empty() {
            let remarkable = match &remarkable {
                Some(r) => r,
                None => remarkable.insert(Remarkable::connect()?),
            };
            if folder_index.is_none() {
                folder_index = Some(FolderIndex::from_files(remarkable.list_files()?));
            }
            let Some(index) = folder_index.as_ref() else {
                bail!("folder index unexpectedly missing");
            };
            let tracked: Vec<(String, RemarkableDocState)> = lib
                .remarkable_docs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (attachment_key, tracked_doc) in tracked {
                let Some(remote) = index.docs_by_id.get(&tracked_doc.doc_id) else {
                    continue;
                };
                if remote.hash.is_empty() || remote.hash == tracked_doc.last_hash {
                    continue;
                }
                match sync_marked_up_copy(&zotero, remarkable, &library, &attachment_key, remote) {
                    Ok(()) => {
                        println!("synced marked-up copy back to Zotero: {}", remote.file_name);
                        if let Some(saved) = lib.remarkable_docs.get_mut(&attachment_key) {
                            saved.last_hash = remote.hash.clone();
                        }
                        state.libraries.insert(library.clone(), lib.clone());
                        state.save(&state_path)?;
                    }
                    Err(err) => {
                        lib_failures += 1;
                        eprintln!(
                            "FAILED syncing marked-up copy for attachment {}: {err:#}",
                            attachment_key
                        );
                    }
                }
            }
        }

        for item in new {
            let placement = item_placement(&zotero, &library, item, &collections_by_key);
            let bytes = match zotero.download(&library, &item.key) {
                Ok(Some(bytes)) => bytes,
                // No file in Zotero storage — nothing to upload. Not recorded
                // as synced, so it is retried automatically if the PDF is
                // later uploaded to Zotero (its item version will change).
                Ok(None) => {
                    eprintln!("skipped (no PDF stored in Zotero yet): {}", placement.name);
                    continue;
                }
                Err(err) => {
                    lib_failures += 1;
                    eprintln!("FAILED: {}: {err:#}", placement.name);
                    continue;
                }
            };
            let remarkable = match &remarkable {
                Some(r) => r,
                None => remarkable.insert(Remarkable::connect()?),
            };
            if folder_index.is_none() {
                folder_index = Some(FolderIndex::from_files(remarkable.list_files()?));
            }
            let Some(index) = folder_index.as_mut() else {
                bail!("folder index unexpectedly missing");
            };
            let parent = ensure_folder_path(remarkable, index, &placement.folder_path)?;
            match remarkable.upload_pdf(&placement.name, bytes, parent.as_deref()) {
                Ok(uploaded) => {
                    println!("uploaded: {}", placement.name);
                    lib.synced.insert(item.key.clone(), item.version);
                    lib.remarkable_docs.insert(
                        item.key.clone(),
                        RemarkableDocState {
                            doc_id: uploaded.id.clone(),
                            last_hash: uploaded.hash.clone(),
                        },
                    );
                    if !uploaded.id.is_empty() {
                        index.docs_by_id.insert(uploaded.id.clone(), uploaded);
                    }
                    state.libraries.insert(library.clone(), lib.clone());
                    state.save(&state_path)?;
                }
                Err(err) => {
                    lib_failures += 1;
                    eprintln!("FAILED: {}: {err:#}", placement.name);
                }
            }
        }

        // Only advance the version when everything uploaded, so failed items
        // are picked up again on the next run.
        if lib_failures == 0 {
            lib.last_library_version = library_version;
        }
        failures += lib_failures;
        state.libraries.insert(library.clone(), lib);
        state.save(&state_path)?;
    }

    if failures > 0 {
        bail!("{failures} sync step(s) failed — they will be retried on the next run");
    }
    Ok(())
}

/// Record every PDF attachment currently in the configured libraries as
/// already synced, without uploading anything. After this, `sync` only sends
/// future additions.
pub fn baseline() -> Result<()> {
    let cfg = config::load()?;
    let zotero = Zotero::new(&cfg.zotero_api_key);

    let state_path = config::state_path()?;
    let mut state = State::load(&state_path, &cfg.user_library());

    for library in cfg.libraries() {
        println!("[{library}] fetching all PDF attachments…");
        let (attachments, library_version) = zotero.pdf_attachments(&library, 0)?;
        let lib = state.libraries.entry(library.clone()).or_default();
        let already = lib.synced.len();
        for item in &attachments {
            lib.synced.insert(item.key.clone(), item.version);
        }
        lib.last_library_version = library_version;
        println!(
            "[{library}] marked {} attachment(s) as synced ({} were already recorded).",
            attachments.len(),
            already
        );
    }
    state.save(&state_path)?;
    println!("Future `zoterable sync` runs will only upload newly added PDFs.");
    Ok(())
}

fn sync_marked_up_copy(
    zotero: &Zotero,
    remarkable: &Remarkable,
    library: &str,
    attachment_key: &str,
    remote: &FileEntry,
) -> Result<()> {
    let zip_bytes = remarkable.download_zip(&remote.id)?;
    let pdf = pdf_from_zip(&zip_bytes, &remote.id)?;
    let mut name = remote.file_name.trim().to_string();
    if !name.to_ascii_lowercase().ends_with(".pdf") {
        name.push_str(".pdf");
    }
    zotero.upload_attachment_pdf(library, attachment_key, &name, &pdf)?;
    Ok(())
}

fn ensure_folder_path(
    remarkable: &Remarkable,
    index: &mut FolderIndex,
    folder_path: &[String],
) -> Result<Option<String>> {
    if folder_path.is_empty() {
        return Ok(None);
    }
    let mut parent = String::new();
    for part in folder_path {
        let key = (parent.clone(), part.clone());
        let folder_id = match index.by_parent_and_name.get(&key) {
            Some(existing) => existing.clone(),
            None => {
                let created =
                    remarkable.create_folder(part, (!parent.is_empty()).then_some(&parent))?;
                index.by_parent_and_name.insert(key, created.id.clone());
                if !created.id.is_empty() {
                    index.docs_by_id.insert(created.id.clone(), created.clone());
                }
                created.id
            }
        };
        parent = folder_id;
    }
    Ok((!parent.is_empty()).then_some(parent))
}

fn is_folder(file: &FileEntry) -> bool {
    let kind = file.entry_type.to_ascii_lowercase();
    kind == "collectiontype" || kind == "folder"
}

fn item_placement(
    zotero: &Zotero,
    library: &str,
    item: &Item,
    collections_by_key: &HashMap<String, Collection>,
) -> ItemPlacement {
    let fallback = item
        .data
        .filename
        .clone()
        .or_else(|| item.data.title.clone())
        .unwrap_or_else(|| item.key.clone());
    let fallback = sanitize(fallback.trim_end_matches(".pdf").trim_end_matches(".PDF"));

    let Some(parent_key) = &item.data.parent_item else {
        return ItemPlacement {
            name: fallback,
            folder_path: vec![],
        };
    };
    let Ok(parent) = zotero.item(library, parent_key) else {
        return ItemPlacement {
            name: fallback,
            folder_path: vec![],
        };
    };

    let mut parts: Vec<String> = Vec::new();
    let authors: Vec<&str> = parent
        .data
        .creators
        .iter()
        .filter_map(|c| c.last_name.as_deref().or(c.name.as_deref()))
        .collect();
    match authors[..] {
        [] => {}
        [one] => parts.push(one.to_string()),
        [first, second] => parts.push(format!("{first} & {second}")),
        [first, ..] => parts.push(format!("{first} et al.")),
    }
    if let Some(year) = parent.data.date.as_deref().and_then(extract_year) {
        parts.push(year);
    }
    if let Some(title) = parent.data.title.as_deref().filter(|t| !t.is_empty()) {
        parts.push(title.to_string());
    }
    let folder_path = collection_path(&parent, collections_by_key);

    ItemPlacement {
        name: if parts.is_empty() {
            fallback
        } else {
            sanitize(&parts.join(" - "))
        },
        folder_path,
    }
}

fn collection_path(parent: &Item, collections_by_key: &HashMap<String, Collection>) -> Vec<String> {
    let Some(collection_key) = parent.data.collections.first() else {
        return vec![];
    };
    let mut path: Vec<String> = Vec::new();
    let mut current = Some(collection_key.as_str());
    while let Some(key) = current {
        let Some(collection) = collections_by_key.get(key) else {
            break;
        };
        path.push(sanitize(&collection.data.name));
        current = collection.data.parent_collection.as_deref();
    }
    path.reverse();
    path
}

fn pdf_from_zip(zip_bytes: &[u8], doc_id: &str) -> Result<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(zip_bytes))?;
    let direct_name = format!("{doc_id}.pdf");
    if let Ok(mut file) = zip.by_name(&direct_name) {
        let mut out = Vec::new();
        file.read_to_end(&mut out)?;
        return Ok(out);
    }
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.name().to_ascii_lowercase().ends_with(".pdf") {
            let mut out = Vec::new();
            file.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    bail!("downloaded reMarkable archive did not contain a PDF file")
}

/// First run of four consecutive digits, e.g. "2023" from "2023-05-01".
fn extract_year(date: &str) -> Option<String> {
    let mut run = String::new();
    for c in date.chars() {
        if c.is_ascii_digit() {
            run.push(c);
            if run.len() == 4 {
                return Some(run);
            }
        } else {
            run.clear();
        }
    }
    None
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > 128 {
        let mut cut = 128;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

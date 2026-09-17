use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const DEFAULT_REMARKABLE_AUTH_HOST: &str = "https://webapp-prod.cloud.remarkable.engineering";
const DEFAULT_REMARKABLE_UPLOAD_HOST: &str = "https://internal.cloud.remarkable.com";

#[derive(Deserialize)]
pub struct Config {
    /// Numeric user ID shown at https://www.zotero.org/settings/keys
    pub zotero_user_id: String,
    /// API key with library read access
    pub zotero_api_key: String,
    /// Numeric IDs of group libraries to sync in addition to the personal
    /// library. The API key needs group read access for these.
    #[serde(default)]
    pub zotero_group_ids: Vec<String>,
    /// Optional: override the reMarkable auth host (for rmfakecloud, etc.).
    #[serde(default)]
    pub remarkable_auth_host: Option<String>,
    /// Optional: override the reMarkable upload host (for rmfakecloud, etc.).
    #[serde(default)]
    pub remarkable_upload_host: Option<String>,
}

impl Config {
    /// API path prefixes of all libraries to sync ("users/…" and "groups/…").
    pub fn libraries(&self) -> Vec<String> {
        let mut libraries = vec![self.user_library()];
        libraries.extend(
            self.zotero_group_ids
                .iter()
                .map(|id| format!("groups/{id}")),
        );
        libraries
    }

    pub fn user_library(&self) -> String {
        format!("users/{}", self.zotero_user_id)
    }
}

pub struct RemarkableEndpoints {
    pub auth_host: String,
    pub upload_host: String,
}

pub fn remarkable_endpoints() -> Result<RemarkableEndpoints> {
    let path = config_path()?;
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read {} — run `zoterable init` first",
            path.display()
        )
    })?;
    let config: Config =
        toml::from_str(&raw).with_context(|| format!("invalid config at {}", path.display()))?;
    Ok(RemarkableEndpoints {
        auth_host: normalize_endpoint(config.remarkable_auth_host, DEFAULT_REMARKABLE_AUTH_HOST),
        upload_host: normalize_endpoint(
            config.remarkable_upload_host,
            DEFAULT_REMARKABLE_UPLOAD_HOST,
        ),
    })
}

fn normalize_endpoint(value: Option<String>, default: &str) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/')
        .to_string()
}

pub fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine the user config directory")?
        .join("zoterable");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn device_token_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("remarkable-device-token"))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("state.json"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read {} — run `zoterable init` first",
            path.display()
        )
    })?;
    let config: Config =
        toml::from_str(&raw).with_context(|| format!("invalid config at {}", path.display()))?;
    if config.zotero_user_id.is_empty() || config.zotero_api_key.is_empty() {
        bail!(
            "fill in zotero_user_id and zotero_api_key in {}",
            path.display()
        );
    }
    Ok(config)
}

const TEMPLATE: &str = "\
# Your numeric user ID and an API key with library read access,
# both from https://www.zotero.org/settings/keys
zotero_user_id = \"\"
zotero_api_key = \"\"

# Optional: numeric IDs of group libraries to sync too (the number in the
# group's URL, https://www.zotero.org/groups/<id>/<name>). The API key must
# have group read access enabled.
zotero_group_ids = []

# Optional: override reMarkable endpoints (for rmfakecloud, etc.).
# remarkable_auth_host = \"https://webapp-prod.cloud.remarkable.engineering\"
# remarkable_upload_host = \"https://internal.cloud.remarkable.com\"
";

pub fn init() -> Result<()> {
    let path = config_path()?;
    if path.exists() {
        println!("Config already exists at {}", path.display());
    } else {
        fs::write(&path, TEMPLATE)?;
        println!("Wrote config template to {}", path.display());
    }
    println!();
    println!("Next steps:");
    println!(
        "  1. Create an API key at https://www.zotero.org/settings/keys and fill in the config."
    );
    println!("  2. Get a one-time code at https://my.remarkable.com/device/browser/connect");
    println!("     and run `zoterable pair <code>` (codes expire after a few minutes).");
    println!("  3. Run `zoterable sync`.");
    Ok(())
}

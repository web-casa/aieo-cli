//! Credentials that belong to the person, not the project: Google refresh
//! tokens, the Bing Webmaster API key, the aieo.ee API key. One JSON file
//! in the user's config directory (`$AIEO_CONFIG_DIR`, else
//! `$XDG_CONFIG_HOME/aieo`, `~/.config/aieo`, `%APPDATA%\aieo`), readable
//! by its owner only — the way `gh` and `gcloud` keep theirs. Environment
//! variables (`AIEO_API_KEY`, `BING_WEBMASTER_API_KEY`) take precedence, so
//! CI never needs the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GoogleToken {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// unix seconds
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AieoKey {
    pub api_key: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CredentialsFile {
    /// keyed by account label (default "default")
    #[serde(default)]
    pub google: BTreeMap<String, GoogleToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bing_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aieo: Option<AieoKey>,
}

pub struct Credentials {
    pub path: PathBuf,
    pub data: CredentialsFile,
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AIEO_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("aieo"));
    }
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("APPDATA") {
            return Some(PathBuf::from(d).join("aieo"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/aieo"))
}

impl Credentials {
    pub fn load() -> Result<Credentials> {
        let dir = config_dir().context("no home directory: set AIEO_CONFIG_DIR")?;
        Credentials::load_from(&dir.join("credentials.json"))
    }

    pub fn load_from(path: &Path) -> Result<Credentials> {
        let data = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CredentialsFile::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Credentials {
            path: path.to_path_buf(),
            data,
        })
    }

    /// Write with owner-only permissions.
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&self.data)?;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(text.as_bytes())?;
        }
        #[cfg(not(unix))]
        std::fs::write(&tmp, &text)?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }

    pub fn bing_key(&self) -> Option<String> {
        std::env::var("BING_WEBMASTER_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| self.data.bing_api_key.clone())
    }

    /// aieo.ee key and endpoint: `AIEO_API_KEY` / `AIEO_ENDPOINT`, else the file.
    pub fn aieo(&self) -> Option<AieoKey> {
        let endpoint = std::env::var("AIEO_ENDPOINT")
            .ok()
            .filter(|e| !e.is_empty());
        match std::env::var("AIEO_API_KEY").ok().filter(|k| !k.is_empty()) {
            Some(api_key) => Some(AieoKey {
                api_key,
                endpoint: endpoint
                    .or_else(|| self.data.aieo.as_ref().map(|a| a.endpoint.clone()))
                    .unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            }),
            None => self.data.aieo.clone().map(|mut a| {
                if let Some(e) = endpoint {
                    a.endpoint = e;
                }
                a
            }),
        }
    }
}

pub const DEFAULT_ENDPOINT: &str = "https://aieo.ee";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aieo/credentials.json");
        let mut c = Credentials::load_from(&path).unwrap();
        assert_eq!(c.data, CredentialsFile::default());
        c.data.bing_api_key = Some("k".into());
        c.data.google.insert(
            "default".into(),
            GoogleToken {
                client_id: "id".into(),
                refresh_token: "r".into(),
                ..Default::default()
            },
        );
        c.save().unwrap();
        let again = Credentials::load_from(&path).unwrap();
        assert_eq!(again.data, c.data);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}

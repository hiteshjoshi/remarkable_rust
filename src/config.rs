//! Configuration & token storage.
//!
//! On-disk layout (TOML), under `dirs::config_dir()/rr/config.toml`:
//!
//! ```toml
//! default_folder = "AI Notes"
//!
//! [auth]
//! access_token = "..."
//! device_token = "..."
//! device_id    = "..."
//! tectonic     = "eu"
//! expires_at   = 1779232253
//! ```

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const APP_NAME: &str = "rr";

#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_SERVICE: &str = "rr";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_USER: &str = "remarkable_token";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub default_folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub access_token: String,
    #[serde(default)]
    pub device_token: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub tectonic: Option<String>,
    /// JWT expiry as a UNIX timestamp.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read config {:?}", path))?;
        toml::from_str(&raw).with_context(|| format!("parse config {:?}", path))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let raw = toml::to_string_pretty(self).context("serialize config")?;
        write_private_file(&path, raw.as_bytes())
    }

    pub fn is_authenticated(&self) -> bool {
        self.auth
            .as_ref()
            .is_some_and(|a| !a.access_token.is_empty())
    }

    pub fn get_token(&self) -> Option<&str> {
        self.auth.as_ref().map(|a| a.access_token.as_str())
    }

    pub fn tectonic(&self) -> Option<&str> {
        self.auth.as_ref().and_then(|a| a.tectonic.as_deref())
    }

    /// Refresh user token using the stored device token. Returns true on
    /// success; the token is persisted to disk before return.
    pub async fn refresh_token(&mut self) -> Result<bool> {
        let Some(auth) = self.auth.clone() else {
            return Ok(false);
        };
        let Some(device_token) = auth.device_token else {
            return Ok(false);
        };
        let Some(device_id) = auth.device_id else {
            return Ok(false);
        };

        match crate::device_auth::refresh_with_device_token(device_token, device_id).await {
            Ok(tokens) => {
                if let Some(a) = &mut self.auth {
                    a.access_token = tokens.user_token.clone();
                    a.tectonic = crate::device_auth::extract_tectonic_claim(&tokens.user_token)
                        .or_else(|| a.tectonic.clone());
                    a.expires_at = jwt_expiry(&tokens.user_token);
                }
                self.save()?;
                Ok(true)
            }
            Err(e) => {
                tracing::warn!(error = %e, "token refresh failed");
                Ok(false)
            }
        }
    }

    pub fn token_expired(&self) -> bool {
        match self.auth.as_ref().and_then(|a| a.expires_at) {
            Some(exp) => chrono::Utc::now().timestamp() + 300 >= exp,
            None => true,
        }
    }
}

fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("create config dir {:?}", path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure config dir {:?}", path))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("open config {:?}", path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure config {:?}", path))?;
    }
    file.write_all(contents)
        .with_context(|| format!("write config {:?}", path))?;
    file.sync_all()
        .with_context(|| format!("sync config {:?}", path))
}

pub fn delete_legacy_tokens() -> Result<()> {
    let Some(base) = dirs::config_dir() else {
        return Ok(());
    };
    let path = base.join(APP_NAME).join("tokens.json");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("delete legacy tokens {:?}", path)),
    }
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir")?;
    Ok(base.join(APP_NAME).join("config.toml"))
}

// OS-keyring mirror of the user token. macOS uses the Keychain, Windows
// uses Credential Manager. On Linux we deliberately skip this: the
// available backends pull in either libdbus or libkeyutils, which is
// painful to satisfy inside a static cross-compile. The TOML config file
// remains the source of truth on every platform — keyring is just an
// extra layer where it's cheap.

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn store_token_secure(token: &str) -> Result<()> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("create keyring entry")?;
    entry.set_password(token).context("set keyring password")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn store_token_secure(_token: &str) -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn get_token_secure() -> Result<Option<String>> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("create keyring entry")?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring: {e}")),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn get_token_secure() -> Result<Option<String>> {
    Ok(None)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn delete_token_secure() -> Result<()> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("create keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring delete: {e}")),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn delete_token_secure() -> Result<()> {
    Ok(())
}

/// Parse the `exp` claim from a JWT without verification.
pub fn jwt_expiry(token: &str) -> Option<i64> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let padded = match payload.len() % 4 {
        0 => payload.to_string(),
        n => format!("{}{}", payload, "=".repeat(4 - n)),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp").and_then(|v| v.as_i64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_storage_enforces_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("rr");
        create_private_dir(&dir).unwrap();
        let file = dir.join("config.toml");
        write_private_file(&file, b"placeholder = true\n").unwrap();

        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

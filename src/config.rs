use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const APP_NAME: &str = "rr";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub auth: Option<AuthConfig>,
    pub default_format: Option<String>,
    pub default_folder: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthConfig {
    pub access_token: String,   // user token (for API calls)
    pub device_token: Option<String>, // device token (for refreshing)
    pub device_id: Option<String>,    // device ID
    pub tectonic: Option<String>,
    pub api_url: Option<String>,
    pub expires_at: Option<i64>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| "Failed to parse config TOML")?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config dir {:?}", parent))?;
        }
        let contents = toml::to_string_pretty(self)
            .with_context(|| "Failed to serialize config")?;
        std::fs::write(&path, contents)
            .with_context(|| format!("Failed to write config to {:?}", path))?;
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.auth.is_some()
    }

    pub fn get_token(&self) -> Option<&str> {
        self.auth.as_ref().map(|a| a.access_token.as_str())
    }

    /// Refresh the user token using the stored device token
    pub async fn refresh_token(&mut self) -> Result<bool> {
        let auth = match &self.auth {
            Some(auth) => auth.clone(),
            None => return Ok(false),
        };

        let device_token = match &auth.device_token {
            Some(token) => token.clone(),
            None => return Ok(false),
        };

        let device_id = match &auth.device_id {
            Some(id) => id.clone(),
            None => return Ok(false),
        };

        match super::device_auth::refresh_with_device_token(device_token, device_id).await {
            Ok(tokens) => {
                if let Some(auth) = &mut self.auth {
                    auth.access_token = tokens.user_token;
                    auth.expires_at = None;
                }
                self.save()?;
                Ok(true)
            }
            Err(e) => {
                eprintln!("Token refresh failed: {}", e);
                Ok(false)
            }
        }
    }

    pub fn get_api_url(&self) -> String {
        self.auth
            .as_ref()
            .and_then(|a| a.api_url.clone())
            .unwrap_or_else(|| "https://internal.cloud.remarkable.com".to_string())
    }

    pub fn get_tectonic_url(&self) -> String {
        let tectonic = self
            .auth
            .as_ref()
            .and_then(|a| a.tectonic.clone())
            .unwrap_or_else(|| "eu".to_string());
        format!("https://web.{}.tectonic.remarkable.com", tectonic)
    }
}

pub fn config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Could not find config directory")?;
    Ok(config_dir.join(APP_NAME).join("config.toml"))
}

/// Store token securely using keyring (macOS Keychain / Linux Secret Service / Windows Credential Manager)
pub fn store_token_secure(token: &str) -> Result<()> {
    let entry = keyring::Entry::new("rr", "remarkable_token")
        .with_context(|| "Failed to create keyring entry")?;
    entry.set_password(token)
        .with_context(|| "Failed to store token in keyring")?;
    Ok(())
}

pub fn get_token_secure() -> Result<Option<String>> {
    let entry = keyring::Entry::new("rr", "remarkable_token")
        .with_context(|| "Failed to create keyring entry")?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("Keyring error: {}", e)),
    }
}

pub fn delete_token_secure() -> Result<()> {
    let entry = keyring::Entry::new("rr", "remarkable_token")
        .with_context(|| "Failed to create keyring entry")?;
    entry.delete_credential()
        .with_context(|| "Failed to delete token from keyring")?;
    Ok(())
}

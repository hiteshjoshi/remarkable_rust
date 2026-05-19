use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedTokens {
    pub device_token: String,
    pub user_token: String,
    pub device_id: String,
    pub created_at: String,
}

pub fn tokens_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not find config directory")?;
    let dir = config_dir.join("rr");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn save_tokens_debug(tokens: &crate::device_auth::AuthTokens) -> Result<()> {
    let dir = tokens_dir()?;
    let path = dir.join("tokens.json");

    let saved = SavedTokens {
        device_token: tokens.device_token.clone(),
        user_token: tokens.user_token.clone(),
        device_id: tokens.device_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let json = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, json)?;

    println!("  Tokens saved to: {:?}", path);

    Ok(())
}

pub fn load_tokens_debug() -> Result<Option<SavedTokens>> {
    let dir = tokens_dir()?;
    let path = dir.join("tokens.json");

    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)?;
    let tokens: SavedTokens = serde_json::from_str(&content)?;

    Ok(Some(tokens))
}

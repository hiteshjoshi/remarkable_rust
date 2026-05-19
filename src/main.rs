mod api;
mod auth;
mod cli;
mod config;
mod convert;
mod device_auth;
mod skills;
mod sync;
mod sync_v3;
mod token_helper;
mod token_store;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::config::{Config, config_path};
use crate::sync::{SyncOptions, sync_directory, watch_and_sync};
use crate::sync_v3::SyncClient;

/// Refresh token if needed, returns new token string
async fn refresh_if_needed(config: &mut Config) -> Result<Option<String>> {
    if let Some(auth) = &config.auth {
        // Check if token is expired (with 5 min buffer)
        let is_expired = auth.expires_at.map(|exp| {
            let now = chrono::Utc::now().timestamp();
            now >= exp - 300
        }).unwrap_or(true);
        
        if is_expired {
            println!("Token expired or missing expiry, refreshing...");
            if config.refresh_token().await? {
                println!("Token refreshed successfully!");
                return Ok(config.get_token().map(|t| t.to_string()));
            } else {
                eprintln!("Token refresh failed. Run 'rr auth' to re-authenticate.");
                return Ok(None);
            }
        }
    }
    Ok(config.get_token().map(|t| t.to_string()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    match args.command {
        Commands::Auth { token, open } => {
            handle_auth(token, open).await?;
        }
        Commands::Upload {
            file,
            notebook,
            folder,
            dir,
            title,
        } => {
            handle_upload(file, notebook, folder, dir, title).await?;
        }
        Commands::Sync {
            dir,
            watch,
            notebook,
            folder,
        } => {
            handle_sync(dir, watch, notebook, folder).await?;
        }
        Commands::Ls { long, folders } => {
            handle_ls(long, folders).await?;
        }
        Commands::Mkdir { name, parent } => {
            handle_mkdir(name, parent).await?;
        }
        Commands::Rm { id } => {
            handle_rm(id).await?;
        }
        Commands::Status => {
            handle_status().await?;
        }
        Commands::Test => {
            handle_test().await?;
        }
        Commands::Logout => {
            handle_logout()?;
        }
        Commands::Skills { target, dry_run } => {
            crate::skills::install_skills(&target, dry_run)?;
        }
    }

    Ok(())
}

async fn handle_auth(manual_token: Option<String>, open_helper: bool) -> Result<()> {
    // If user provided a token manually, use it directly
    if let Some(token) = manual_token {
        println!("Validating token...");
        
        match device_auth::validate_token(&token).await {
            Ok(_) => {
                let auth_config = config::AuthConfig {
                    access_token: token,
                    device_token: None,
                    device_id: None,
                    tectonic: Some("eu".to_string()),
                    api_url: Some("https://internal.cloud.remarkable.com".to_string()),
                    expires_at: None,
                };
                let mut config = Config::load()?;
                config.auth = Some(auth_config);
                config.save()?;
                println!("✓ Token validated and saved successfully!");
                println!();
                println!("You can now use:");
                println!("  rr upload myfile.md");
                println!("  rr sync ~/notes");
                println!("  rr ls");
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ Token validation failed: {}", e);
                eprintln!("  The token may be expired or invalid.");
                return Ok(());
            }
        }
    }

    // Default: use proper device pairing flow
    match device_auth::authenticate_terminal().await {
        Ok(tokens) => {
            let auth_config = config::AuthConfig {
                access_token: tokens.user_token.clone(),
                device_token: Some(tokens.device_token.clone()),
                device_id: Some(tokens.device_id.clone()),
                tectonic: Some("eu".to_string()),
                api_url: Some("https://internal.cloud.remarkable.com".to_string()),
                expires_at: None,
            };
            
            // Save tokens for debugging/reverse engineering
            let _ = token_store::save_tokens_debug(&tokens);
            
            let mut config = Config::load()?;
            config.auth = Some(auth_config);
            config.save()?;
            
            println!();
            println!("Credentials saved to: {:?}", config_path()?);
            println!();
            println!("You can now use:");
            println!("  rr upload myfile.md");
            println!("  rr sync ~/notes");
            println!("  rr ls");
        }
        Err(e) => {
            eprintln!();
            eprintln!("✗ Authentication failed: {}", e);
            eprintln!();
            eprintln!("Alternative: If you have an existing token from the Chrome extension,");
            eprintln!("use: rr auth --token <YOUR_TOKEN>");
        }
    }

    Ok(())
}

async fn handle_upload(
    file: std::path::PathBuf,
    _notebook: bool,
    folder: Option<String>,
    dir: Option<String>,
    custom_title: Option<String>,
) -> Result<()> {
    let mut config = Config::load()?;
    if !config.is_authenticated() {
        eprintln!("Not authenticated. Run 'rr auth' first.");
        return Ok(());
    }

    println!("Converting {:?} to PDF...", file);
    let (title, pdf_bytes) = convert::convert_file_to_pdf(&file)?;
    let title = custom_title.unwrap_or(title);

    let token = match refresh_if_needed(&mut config).await? {
        Some(t) => t,
        None => return Ok(()),
    };
    let client = SyncClient::new(token);

    // Resolve target folder - --dir takes precedence over --folder
    let parent_id = if let Some(dir_path) = dir {
        // Create directory structure (supports nested paths like "Work/Projects")
        Some(resolve_or_create_path(&client, &dir_path).await?)
    } else if let Some(folder_name) = folder {
        // Single folder name (backward compatibility)
        println!("Resolving folder '{}'...", folder_name);
        match client.find_folder(&folder_name).await? {
            Some(id) => {
                println!("  Found folder: {}", id);
                Some(id)
            }
            None => {
                println!("  Folder '{}' not found, creating it...", folder_name);
                let folder_id = uuid::Uuid::new_v4().to_string();
                if let Err(e) = client.create_folder(&folder_id, &folder_name, None).await {
                    eprintln!("  ✗ Failed to create folder: {}", e);
                    return Ok(());
                }
                println!("  Created folder: {}", folder_id);
                Some(folder_id)
            }
        }
    } else {
        None
    };

    println!("Uploading '{}' to reMarkable...", title);

    // Generate a UUID for the document
    let doc_id = uuid::Uuid::new_v4().to_string();
    
    match client.upload_file(&doc_id, &title, "pdf", pdf_bytes, parent_id.clone()).await {
        Ok(_) => {
            println!("✓ Uploaded successfully!");
            println!("  ID: {}", doc_id);
            println!("  Name: {}", title);
            if let Some(pid) = parent_id {
                println!("  Folder: {}", pid);
            }
        }
        Err(e) => {
            eprintln!("✗ Upload failed: {}", e);
        }
    }

    Ok(())
}

/// Resolve a path like "Work/Projects" by creating intermediate folders as needed
/// Returns the ID of the deepest folder in the path
async fn resolve_or_create_path(client: &SyncClient, path: &str) -> Result<String> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        anyhow::bail!("Empty directory path");
    }

    let mut current_parent_id: Option<String> = None;

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let depth = i + 1;

        // Search for existing folder at this level
        println!("  [{}/{}] Looking for '{}'...", depth, parts.len(), part);
        
        let existing = find_folder_at_level(client, part, current_parent_id.clone()).await?;
        
        if let Some(id) = existing {
            println!("    Found existing folder: {}", id);
            current_parent_id = Some(id);
        } else {
            println!("    Creating folder '{}'...", part);
            let new_id = uuid::Uuid::new_v4().to_string();
            client.create_folder(&new_id, part, current_parent_id.clone()).await
                .with_context(|| format!("Failed to create folder '{}'", part))?;
            println!("    Created: {}", new_id);
            current_parent_id = Some(new_id);
        }
    }

    current_parent_id.ok_or_else(|| anyhow::anyhow!("Failed to resolve path: {}", path))
}

/// Find a folder with a specific name under a specific parent
async fn find_folder_at_level(
    client: &SyncClient,
    name: &str,
    parent_id: Option<String>,
) -> Result<Option<String>> {
    let all_folders = client.list_folders().await?;
    
    for folder in all_folders {
        if folder.name == name && folder.parent == parent_id {
            return Ok(Some(folder.doc_id));
        }
    }
    
    Ok(None)
}

async fn handle_sync(
    _dir: std::path::PathBuf,
    _watch: bool,
    _notebook: bool,
    _folder: Option<String>,
) -> Result<()> {
    let config = Config::load()?;
    if !config.is_authenticated() {
        eprintln!("Not authenticated. Run 'rr auth' first.");
        return Ok(());
    }

    println!("Sync command is being updated for Sync v3 API.");
    println!("Please use 'rr upload <file>' for now.");

    Ok(())
}

async fn handle_ls(_long: bool, folders_only: bool) -> Result<()> {
    let config = Config::load()?;
    if !config.is_authenticated() {
        eprintln!("Not authenticated. Run 'rr auth' first.");
        return Ok(());
    }

    let token = config.get_token().unwrap().to_string();
    let client = SyncClient::new(token);

    println!("Fetching files from reMarkable cloud...");
    
    let result = match client.list_files().await {
        Ok(files) => {
            if files.is_empty() {
                println!("No files found.");
                return Ok(());
            }

            let filtered = if folders_only {
                files.into_iter().filter(|f| f.entry_type == "CollectionType").collect::<Vec<_>>()
            } else {
                files
            };

            println!();
            println!("{:<36} {:<12} {}", "ID", "Type", "Name");
            println!("{}", "-".repeat(80));
            for file in &filtered {
                let icon = if file.entry_type == "CollectionType" { "📁" } else { "📄" };
                let parent_info = if let Some(parent) = &file.parent {
                    format!(" [in: {}]", parent)
                } else {
                    String::new()
                };
                println!(
                    "{:<36} {:<12} {} {}{}",
                    file.doc_id, file.entry_type, icon, file.name, parent_info
                );
            }
            println!();
            println!("Total: {} items", filtered.len());
            Ok(())
        }
        Err(e) => {
            if e.to_string().contains("401") || e.to_string().contains("Unauthorized") || e.to_string().contains("expired") {
                println!("Token expired, attempting refresh...");
                let mut config = Config::load()?;
                if config.refresh_token().await? {
                    println!("Token refreshed! Retrying...");
                    let token = config.get_token().unwrap().to_string();
                    let client = SyncClient::new(token);
                    match client.list_files().await {
                        Ok(files) => {
                            let filtered = if folders_only {
                                files.into_iter().filter(|f| f.entry_type == "CollectionType").collect::<Vec<_>>()
                            } else {
                                files
                            };
                            println!();
                            for file in &filtered {
                                let icon = if file.entry_type == "CollectionType" { "📁" } else { "📄" };
                                println!("{:<36} {:<12} {} {}", file.doc_id, file.entry_type, icon, file.name);
                            }
                            println!("\nTotal: {} items", filtered.len());
                            Ok(())
                        }
                        Err(e2) => {
                            eprintln!("✗ Still failed after refresh: {}", e2);
                            Ok(())
                        }
                    }
                } else {
                    eprintln!("✗ Token refresh failed. Run 'rr auth' to re-authenticate.");
                    Ok(())
                }
            } else {
                eprintln!("✗ Failed to list files: {}", e);
                Ok(())
            }
        }
    };
    
    result
}

async fn handle_mkdir(name: String, parent: Option<String>) -> Result<()> {
    let mut config = Config::load()?;
    if !config.is_authenticated() {
        eprintln!("Not authenticated. Run 'rr auth' first.");
        return Ok(());
    }

    let token = match refresh_if_needed(&mut config).await? {
        Some(t) => t,
        None => return Ok(()),
    };
    let client = SyncClient::new(token);

    println!("Creating folder '{}'...", name);

    let folder_id = uuid::Uuid::new_v4().to_string();
    match client.create_folder(&folder_id, &name, parent).await {
        Ok(_) => {
            println!("✓ Folder created successfully!");
            println!("  ID: {}", folder_id);
            println!("  Name: {}", name);
        }
        Err(e) => {
            eprintln!("✗ Failed to create folder: {}", e);
        }
    }

    Ok(())
}

async fn handle_rm(id: String) -> Result<()> {
    let mut config = Config::load()?;
    if !config.is_authenticated() {
        eprintln!("Not authenticated. Run 'rr auth' first.");
        return Ok(());
    }

    let token = match refresh_if_needed(&mut config).await? {
        Some(t) => t,
        None => return Ok(()),
    };
    let client = SyncClient::new(token);

    println!("Deleting file {}...", id);
    println!("  (Note: delete not yet implemented for sync v3)");

    Ok(())
}

async fn handle_status() -> Result<()> {
    let mut config = Config::load()?;

    println!("rr - reMarkable Rust CLI");
    println!();

    if config.is_authenticated() {
        println!("Authentication: ✓ Logged in");
        if let Some(auth) = &config.auth {
            println!("  API URL: {}", config.get_api_url());
            if let Some(tectonic) = &auth.tectonic {
                println!("  Region: {}", tectonic);
            }
            if let Some(expires) = auth.expires_at {
                let now = chrono::Utc::now().timestamp();
                if expires > now {
                    let mins = (expires - now) / 60;
                    println!("  Token expires in: {} minutes", mins);
                } else {
                    println!("  Token: ✗ Expired (will refresh on next API call)");
                }
            } else {
                println!("  Token: No expiry set");
            }
        }

        // Test connection with auto-refresh
        let token = match refresh_if_needed(&mut config).await? {
            Some(t) => t,
            None => {
                println!("  Cloud connection: ✗ Token refresh failed");
                return Ok(());
            }
        };
        let client = SyncClient::new(token);
        match client.list_files().await {
            Ok(files) => {
                println!("  Cloud connection: ✓ OK ({} items)", files.len());
            }
            Err(e) => {
                println!("  Cloud connection: ✗ Failed ({})", e);
            }
        }
    } else {
        println!("Authentication: ✗ Not logged in");
        println!("  Run 'rr auth' to authenticate with your reMarkable account");
    }

    println!();
    println!("Config file: {:?}", config_path()?);

    Ok(())
}

async fn handle_test() -> Result<()> {
    println!("rr - Token Debug Tool");
    println!();

    // Check saved tokens
    match token_store::load_tokens_debug()? {
        Some(tokens) => {
            println!("Saved tokens found:");
            println!("  Device ID: {}", tokens.device_id);
            println!("  Device token: {}...", &tokens.device_token[..50]);
            println!("  User token: {}...", &tokens.user_token[..50]);
            println!("  Created: {}", tokens.created_at);
            println!();
            
            // Test the user token against the API
            println!("Testing user token against API...");
            let client = SyncClient::new(tokens.user_token.clone());
            
            match client.list_files().await {
                Ok(files) => {
                    println!("✓ API connection successful! Found {} items", files.len());
                }
                Err(e) => {
                    println!("✗ API test failed: {}", e);
                }
            }
        }
        None => {
            println!("No saved tokens found.");
            println!("Run 'rr auth' first to save tokens.");
        }
    }

    Ok(())
}

fn handle_logout() -> Result<()> {
    let mut config = Config::load()?;
    config.auth = None;
    config.save()?;

    // Also try to clear from keyring
    let _ = config::delete_token_secure();

    println!("✓ Logged out and removed credentials");

    Ok(())
}

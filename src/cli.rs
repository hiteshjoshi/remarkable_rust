//! CLI surface: argument parsing + command dispatch.
//!
//! Every subcommand lives in its own `handle_*` async function. The public
//! entry point is [`run`], which `main.rs` invokes.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;

use crate::cloud_api::{CloudClient, FileItem};
use crate::config::{self, Config};
use crate::epub::{build_article_epub_with_assets, EpubAsset, EpubMeta};
use crate::error::Error as RrError;
use crate::jobs;
use crate::logging;
use crate::retry::{retry_default, Policy};
use crate::skills;
use crate::source::{prepare_from_path, prepare_from_string, Prepared, SourceKind};

pub type CliError = anyhow::Error;

#[derive(Debug, Parser)]
#[command(
    name = "rr",
    version,
    about = "Sync markdown to your reMarkable Paper Pro as native notebooks",
    long_about = "rr is a small Rust CLI that wraps the official reMarkable cloud\n\
                  Document API. Markdown is packaged into an EPUB locally, posted\n\
                  to /import/v1/files with convert=true, and the cloud renders it\n\
                  into a native reMarkable notebook on the device. No PDFs.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Verbose logging (DEBUG-level) to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Pair this machine with a reMarkable account.
    Auth {
        /// Use a manually-supplied user token (skip device pairing).
        #[arg(short, long)]
        token: Option<String>,
    },

    /// Upload a markdown file. Default delivers a native reMarkable notebook.
    Upload {
        /// Path to a markdown file (or `-` for stdin).
        file: PathBuf,

        /// Override the document title (otherwise inferred from H1 or filename).
        #[arg(short, long)]
        title: Option<String>,

        /// Target folder name (created if absent).
        #[arg(short, long)]
        folder: Option<String>,

        /// Target folder path (`Work/2026`); creates intermediate folders.
        #[arg(short, long, conflicts_with = "folder")]
        dir: Option<String>,

        /// Wire format. `notebook` (default) yields a native reMarkable
        /// notebook; `epub` ships the EPUB itself unchanged.
        #[arg(long, value_enum, default_value_t = WireFormat::Notebook)]
        format: WireFormat,

        /// Run the upload as a detached background job. Returns immediately
        /// with a job id; check `rr jobs` / `rr logs <id>` for progress.
        #[arg(long)]
        background: bool,
    },

    /// List documents and folders in the reMarkable cloud.
    Ls {
        /// Only show folders.
        #[arg(short, long)]
        folders: bool,
    },

    /// Create a folder at root (or under --parent).
    Mkdir {
        name: String,
        #[arg(short, long)]
        parent: Option<String>,
    },

    /// Delete a document or folder by id.
    Rm {
        /// The document or folder id (from `rr ls`).
        id: String,
    },

    /// Show authentication & cloud connectivity status.
    Status,

    /// Forget all stored credentials.
    Logout,

    /// Install agent skills for Claude / OpenCode / Codex.
    Skills {
        #[arg(short, long, default_value = "all")]
        target: String,
        #[arg(short, long)]
        dry_run: bool,
    },

    /// List background jobs.
    Jobs,

    /// Print the log for a background job.
    Logs { id: String },

    /// Cancel a running background job.
    Cancel { id: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WireFormat {
    /// EPUB → /import/v1/files (convert=true) → native reMarkable notebook.
    Notebook,
    /// EPUB → /doc/v2/files (no conversion).
    Epub,
}

/// Entry point used by `main.rs`.
pub async fn run() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let cli = Cli::parse();
    logging::init(cli.verbose);

    let job_id = jobs::current_job_id();
    if let Some(id) = &job_id {
        let kind = match &cli.command {
            Command::Upload { .. } => "upload",
            Command::Mkdir { .. } => "mkdir",
            Command::Rm { .. } => "rm",
            _ => "other",
        };
        // Best-effort: failure to write the pid/meta file shouldn't kill the job.
        if let Err(e) = jobs::child_init(id, kind, &raw_args) {
            tracing::warn!(error = %e, "child_init failed");
        }
    }

    let outcome = dispatch(cli.command).await;

    if let Some(id) = job_id {
        let (status, code) = match &outcome {
            Ok(()) => (jobs::JobStatus::Succeeded, Some(0_i32)),
            Err(_) => (jobs::JobStatus::Failed, Some(1_i32)),
        };
        if let Err(e) = jobs::child_finalise(&id, status, code) {
            tracing::warn!(error = %e, "child_finalise failed");
        }
    }
    outcome
}

async fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Auth { token } => handle_auth(token).await,
        Command::Upload {
            file,
            title,
            folder,
            dir,
            format,
            background,
        } => handle_upload(file, title, folder, dir, format, background).await,
        Command::Ls { folders } => handle_ls(folders).await,
        Command::Mkdir { name, parent } => handle_mkdir(name, parent).await,
        Command::Rm { id } => handle_rm(id).await,
        Command::Status => handle_status().await,
        Command::Logout => handle_logout(),
        Command::Skills { target, dry_run } => skills::install_skills(&target, dry_run),
        Command::Jobs => handle_jobs(),
        Command::Logs { id } => handle_logs(&id),
        Command::Cancel { id } => handle_cancel(&id),
    }
}

/// Pair a device with reMarkable's auth server.
async fn handle_auth(manual_token: Option<String>) -> Result<()> {
    if let Some(token) = manual_token {
        println!("Validating token...");
        crate::device_auth::validate_token(&token)
            .await
            .context("token did not validate")?;

        let tectonic = crate::device_auth::extract_tectonic_claim(&token);
        let expires_at = config::jwt_expiry(&token);
        let auth = config::AuthConfig {
            access_token: token.clone(),
            device_token: None,
            device_id: None,
            tectonic,
            expires_at,
        };
        let mut cfg = Config::load()?;
        cfg.auth = Some(auth);
        cfg.save()?;
        let _ = config::store_token_secure(&token);
        println!("{} Token saved.", "✓".green());
        return Ok(());
    }

    let tokens = crate::device_auth::authenticate_terminal()
        .await
        .context("device pairing")?;
    let _ = crate::token_store::save_tokens_debug(&tokens);
    let tectonic = crate::device_auth::extract_tectonic_claim(&tokens.user_token);
    let expires_at = config::jwt_expiry(&tokens.user_token);

    let auth = config::AuthConfig {
        access_token: tokens.user_token.clone(),
        device_token: Some(tokens.device_token),
        device_id: Some(tokens.device_id),
        tectonic,
        expires_at,
    };

    let mut cfg = Config::load()?;
    cfg.auth = Some(auth);
    cfg.save()?;
    let _ = config::store_token_secure(&tokens.user_token);

    println!("{} Paired with reMarkable cloud.", "✓".green());
    println!("Config saved to {:?}", config::config_path()?);
    Ok(())
}

/// Resolve the active client, refreshing tokens if required.
async fn cloud_client() -> Result<(Config, CloudClient)> {
    let mut cfg = Config::load()?;
    if !cfg.is_authenticated() {
        bail!("not authenticated; run `rr auth` first");
    }

    if cfg.token_expired() {
        tracing::debug!("token expired or missing expiry — refreshing");
        if !cfg.refresh_token().await? {
            bail!("token refresh failed; run `rr auth` to re-pair");
        }
    }

    let token = cfg
        .get_token()
        .context("missing token after refresh")?
        .to_owned();
    let tectonic = cfg.tectonic().map(str::to_owned);
    let client = CloudClient::from_token_and_tectonic(token, tectonic.as_deref())
        .context("build cloud client")?;
    Ok((cfg, client))
}

async fn handle_upload(
    file: PathBuf,
    custom_title: Option<String>,
    folder: Option<String>,
    dir: Option<String>,
    format: WireFormat,
    background: bool,
) -> Result<()> {
    if background {
        return spawn_background_upload(file, custom_title, folder, dir, format);
    }

    let prepared: Prepared = if file.as_os_str() == "-" {
        let mut buf = String::new();
        use std::io::Read;
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("read stdin")?;
        // Sniff: if it looks like an HTML document, treat as such.
        let kind = if buf.trim_start().starts_with('<') {
            SourceKind::Html
        } else {
            SourceKind::Markdown
        };
        prepare_from_string(buf, kind, "stdin", None).map_err(anyhow::Error::from)?
    } else {
        prepare_from_path(&file).map_err(anyhow::Error::from)?
    };

    let title = custom_title.unwrap_or_else(|| prepared.title.clone());
    println!("Building EPUB '{}'...", title);

    let mut meta = EpubMeta::from_title(&title);
    if let Some(author) = prepared.metadata.get("author") {
        meta.author = author.clone();
    }
    if let Some(desc) = prepared.metadata.get("description") {
        meta.description = desc.clone();
    }
    if let Some(lang) = prepared.metadata.get("lang") {
        meta.language = lang.clone();
    }
    if let Some(url) = prepared.metadata.get("source") {
        meta.source_url = url.clone();
    }

    let assets: Vec<EpubAsset> = prepared
        .assets
        .iter()
        .map(|a| EpubAsset {
            name: a.name.clone(),
            mime: a.mime.clone(),
            bytes: a.bytes.clone(),
        })
        .collect();
    if !assets.is_empty() {
        println!("  Embedded {} image(s).", assets.len());
    }
    let epub = build_article_epub_with_assets(&meta, &prepared.xhtml, &assets)
        .map_err(anyhow::Error::from)?;

    let (_cfg, client) = cloud_client().await?;

    let parent_id = if let Some(path) = dir {
        println!("Resolving folder path '{}'...", path);
        Some(client.resolve_or_create_path(&path).await?)
    } else if let Some(name) = folder {
        match client.find_folder(&name, None).await? {
            Some(f) => {
                println!("  Found folder '{}': {}", name, f.id);
                Some(f.id)
            }
            None => {
                println!("  Creating folder '{}'...", name);
                Some(client.create_folder(&name, None).await?.id)
            }
        }
    } else {
        None
    };

    let file_name = if title.to_ascii_lowercase().ends_with(".epub") {
        title.clone()
    } else {
        format!("{title}.epub")
    };

    println!(
        "Uploading to reMarkable cloud as a {}...",
        match format {
            WireFormat::Notebook => "native notebook (server-converted)".to_string(),
            WireFormat::Epub => "EPUB document".to_string(),
        }
    );

    let parent = parent_id.as_deref();
    let bytes = epub.clone();
    let item: FileItem = retry_default(Policy::default_network(), || {
        let client = client.clone();
        let file_name = file_name.clone();
        let bytes = bytes.clone();
        let parent = parent.map(str::to_owned);
        async move {
            match format {
                WireFormat::Notebook => {
                    client
                        .import_as_notebook(&file_name, bytes, parent.as_deref())
                        .await
                }
                WireFormat::Epub => {
                    client
                        .upload_document(&file_name, bytes, parent.as_deref())
                        .await
                }
            }
        }
    })
    .await
    .map_err(map_rr_err)?;

    println!("{} Uploaded.", "✓".green());
    if !item.id.is_empty() {
        println!("  ID:   {}", item.id);
    }
    println!(
        "  Name: {}",
        item.file_name
            .is_empty()
            .then(|| title.as_str())
            .unwrap_or(&item.file_name)
    );
    if let Some(p) = item.parent.as_deref() {
        println!("  Parent: {p}");
    }
    Ok(())
}

fn spawn_background_upload(
    file: PathBuf,
    title: Option<String>,
    folder: Option<String>,
    dir: Option<String>,
    format: WireFormat,
) -> Result<()> {
    // If we're already the background child, no-op (handled by callee).
    if jobs::current_job_id().is_some() {
        bail!("--background passed to an already-detached process");
    }

    let mut child_args: Vec<String> = vec!["upload".into()];
    child_args.push(
        file.to_str()
            .context("file path must be valid utf-8")?
            .to_string(),
    );
    if let Some(t) = title {
        child_args.push("--title".into());
        child_args.push(t);
    }
    if let Some(f) = folder {
        child_args.push("--folder".into());
        child_args.push(f);
    }
    if let Some(d) = dir {
        child_args.push("--dir".into());
        child_args.push(d);
    }
    child_args.push("--format".into());
    child_args.push(match format {
        WireFormat::Notebook => "notebook".into(),
        WireFormat::Epub => "epub".into(),
    });

    let handle = jobs::spawn_detached("upload", &child_args).map_err(anyhow::Error::from)?;
    println!("{} Background job {} started.", "✓".green(), handle.id);
    println!("  pid:  {}", handle.pid);
    println!("  log:  {}", handle.log_path.display());
    println!("Watch with: rr logs {}", handle.id);
    Ok(())
}

async fn handle_ls(folders_only: bool) -> Result<()> {
    let (_cfg, client) = cloud_client().await?;
    let files = client.list_files(folders_only).await.map_err(map_rr_err)?;
    if files.is_empty() {
        println!("No files found.");
        return Ok(());
    }
    println!();
    println!("{:<38} {:<18} {}", "ID", "Type", "Name");
    println!("{}", "-".repeat(80));
    for f in &files {
        let icon = if f.file_type == "CollectionType" {
            "📁"
        } else {
            "📄"
        };
        let parent = f
            .parent
            .as_deref()
            .map(|p| format!(" [in: {p}]"))
            .unwrap_or_default();
        println!(
            "{:<38} {:<18} {} {}{}",
            f.id, f.file_type, icon, f.file_name, parent
        );
    }
    println!();
    println!("Total: {} items", files.len());
    Ok(())
}

async fn handle_mkdir(name: String, parent: Option<String>) -> Result<()> {
    let (_cfg, client) = cloud_client().await?;
    let item = client
        .create_folder(&name, parent.as_deref())
        .await
        .map_err(map_rr_err)?;
    println!("{} Folder '{}' created.", "✓".green(), name);
    if !item.id.is_empty() {
        println!("  ID: {}", item.id);
    }
    Ok(())
}

async fn handle_rm(id: String) -> Result<()> {
    let (_cfg, client) = cloud_client().await?;
    let files = client.list_files(false).await.map_err(map_rr_err)?;
    let target = files
        .iter()
        .find(|f| f.id == id)
        .with_context(|| format!("no item with id {id}"))?;
    if target.hash.is_empty() {
        bail!("server returned no hash for {id}; cannot delete");
    }
    client
        .delete_many(&[target.hash.clone()])
        .await
        .map_err(map_rr_err)?;
    println!("{} Deleted '{}' ({})", "✓".green(), target.file_name, id);
    Ok(())
}

async fn handle_status() -> Result<()> {
    let mut cfg = Config::load()?;
    println!("rr {}", env!("CARGO_PKG_VERSION"));
    println!();
    if !cfg.is_authenticated() {
        println!("Authentication: {}", "not logged in".red());
        println!("Run `rr auth` to pair.");
        return Ok(());
    }
    if let Some(auth) = &cfg.auth {
        println!(
            "Tectonic: {}",
            auth.tectonic.as_deref().unwrap_or("(default)")
        );
        match auth.expires_at {
            Some(exp) => {
                let now = chrono::Utc::now().timestamp();
                if exp > now {
                    println!("Token expires in {} minutes", (exp - now) / 60);
                } else {
                    println!("Token expired ({}s ago)", now - exp);
                }
            }
            None => println!("Token expiry: unknown"),
        }
    }

    if cfg.token_expired() {
        println!("Refreshing token...");
        if !cfg.refresh_token().await? {
            println!("{} refresh failed", "✗".red());
            return Ok(());
        }
    }

    let token = cfg.get_token().unwrap_or("").to_owned();
    let client = CloudClient::from_token_and_tectonic(token, cfg.tectonic())?;
    match client.list_files(true).await {
        Ok(items) => println!("Cloud: {} ({} folders)", "ok".green(), items.len()),
        Err(e) => println!("Cloud: {} ({})", "fail".red(), e),
    }
    println!("Config: {:?}", config::config_path()?);
    Ok(())
}

fn handle_logout() -> Result<()> {
    let mut cfg = Config::load()?;
    cfg.auth = None;
    cfg.save()?;
    let _ = config::delete_token_secure();
    println!("{} Logged out.", "✓".green());
    Ok(())
}

fn handle_jobs() -> Result<()> {
    let jobs = jobs::list().map_err(anyhow::Error::from)?;
    if jobs.is_empty() {
        println!("No background jobs.");
        return Ok(());
    }
    println!(
        "{:<22} {:<10} {:<10} {:<8} {}",
        "ID", "KIND", "STATUS", "EXIT", "STARTED"
    );
    for j in jobs {
        let status = match j.status {
            jobs::JobStatus::Running => "running".yellow(),
            jobs::JobStatus::Succeeded => "ok".green(),
            jobs::JobStatus::Failed => "failed".red(),
            jobs::JobStatus::Cancelled => "cancelled".magenta(),
        };
        let exit = j
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<22} {:<10} {:<10} {:<8} {}",
            j.id,
            j.kind,
            status,
            exit,
            j.started_at.format("%Y-%m-%d %H:%M:%S"),
        );
    }
    Ok(())
}

fn handle_logs(id: &str) -> Result<()> {
    let log = jobs::read_log(id).map_err(anyhow::Error::from)?;
    print!("{log}");
    Ok(())
}

fn handle_cancel(id: &str) -> Result<()> {
    if jobs::cancel(id).map_err(anyhow::Error::from)? {
        println!("Sent SIGTERM to job {id}");
    } else {
        println!("Job {id} not running.");
    }
    Ok(())
}

fn map_rr_err(e: RrError) -> anyhow::Error {
    match e {
        RrError::AuthExpired => anyhow::anyhow!("token expired — run `rr auth` to re-pair"),
        other => anyhow::Error::from(other),
    }
}

/// If we were spawned as a background job, register with the jobs subsystem
/// and finalise on drop. Returns `ExitCode` to allow main to forward state.
///
/// Currently unused — wired in when we extend `main.rs` to call the child
/// pathway. Kept as a hook for the next iteration.
pub fn _background_lifecycle_marker() -> ExitCode {
    ExitCode::SUCCESS
}

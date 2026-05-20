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
    about = "Push markdown to your reMarkable as a native notebook",
    long_about = "rr builds a native v6 reMarkable notebook from a markdown\n\
                  file and uploads it via the cloud sync API. Headings,\n\
                  paragraphs, and bullets render as typed text; tables\n\
                  render as embedded raster images. Works with any\n\
                  reMarkable account — Connect is not required.",
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

    /// Push markdown to your reMarkable as a native v6 notebook.
    ///
    /// Works on any reMarkable account — Connect not required. Tables,
    /// headings, bullets, and prose all ship as native typed text plus
    /// embedded raster images, locally built and uploaded directly via
    /// the cloud sync API.
    Push {
        /// Path to a markdown file (or `-` for stdin).
        file: PathBuf,

        /// Override the document title (otherwise inferred from H1 or filename).
        #[arg(short, long)]
        title: Option<String>,

        /// Target device model. Determines page dimensions and text frame
        /// geometry. Default: paper-pro.
        #[arg(long, value_enum, default_value_t = DeviceArg::PaperPro)]
        device: DeviceArg,

        /// Skip markdown parsing and ship a pre-built `.rm` v6 page verbatim
        /// as the page content. Useful for isolating whether issues are
        /// in the v6 generator or in the cloud bundle layer.
        #[arg(long, value_name = "PATH")]
        rm: Option<PathBuf>,
    },

    /// (hidden) Legacy upload path that builds an EPUB locally and asks
    /// the reMarkable cloud to convert it to a native notebook. Kept for
    /// fallback only — the default `push` produces the same result
    /// without any cloud-side conversion.
    #[command(hide = true)]
    ConnectPush {
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

/// CLI-facing device selector. Maps to [`crate::v6::page::Device`].
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DeviceArg {
    /// reMarkable Paper Pro (10.5" color). 1620 × 2160 drawable area.
    PaperPro,
    /// reMarkable Paper Pro Move (8" color). ~954 × 1696 drawable area.
    PaperProMove,
    /// reMarkable 2 (10.3"). 1404 × 1872 drawable area.
    Rm2,
}

impl From<DeviceArg> for crate::v6::page::Device {
    fn from(d: DeviceArg) -> Self {
        match d {
            DeviceArg::PaperPro => crate::v6::page::Device::PaperPro,
            DeviceArg::PaperProMove => crate::v6::page::Device::PaperProMove,
            DeviceArg::Rm2 => crate::v6::page::Device::Rm2,
        }
    }
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
            Command::Push { .. } => "push",
            Command::ConnectPush { .. } => "connect-push",
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
        Command::Push {
            file,
            title,
            device,
            rm,
        } => handle_push(file, title, device, rm).await,
        Command::ConnectPush {
            file,
            title,
            folder,
            dir,
            format,
            background,
        } => handle_connect_push(file, title, folder, dir, format, background).await,
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

async fn handle_push(
    file: PathBuf,
    custom_title: Option<String>,
    device: DeviceArg,
    rm_override: Option<PathBuf>,
) -> Result<()> {
    // Token: reuse the same load+refresh path the cloud upload uses; sync v3
    // takes the same bearer.
    let mut cfg = Config::load()?;
    if !cfg.is_authenticated() {
        bail!("not authenticated; run `rr auth` first");
    }
    if cfg.token_expired() && !cfg.refresh_token().await? {
        bail!("token refresh failed; run `rr auth` to re-pair");
    }
    let token = cfg
        .get_token()
        .context("missing token after refresh")?
        .to_owned();

    // Read markdown source. Title falls back to H1, then filename, then a
    // generic "Untitled".
    let md = if file.as_os_str() == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).context("read stdin")?;
        buf
    } else {
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?
    };

    let title = custom_title
        .or_else(|| extract_h1_title(&md))
        .or_else(|| {
            file.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Untitled".into());

    println!("Building bundle '{}' for {:?}...", title, device);
    // Split on `---` horizontal rules to create multi-page notebooks.
    // Each chunk becomes one page with its tables rendered as images.
    let pages = crate::notebook::PageInput::pages_from_markdown(&md);
    let opts =
        crate::notebook::BundleOptions::new(title.clone(), pages).with_device(device.into());
    let mut bundle = crate::notebook::Bundle::build(&opts)?;

    if let Some(rm_path) = rm_override {
        let raw_rm = std::fs::read(&rm_path)
            .with_context(|| format!("read --rm {}", rm_path.display()))?;
        println!(
            "  --rm override: replacing page .rm with {} ({} bytes)",
            rm_path.display(),
            raw_rm.len()
        );
        if let Some(first) = bundle.pages.first_mut() {
            first.rm_bytes = raw_rm;
        }
    }
    let total_bytes: usize = bundle.metadata_json.len()
        + bundle.content_json.len()
        + bundle.pagedata.len()
        + bundle.pages.iter().map(|p| p.rm_bytes.len() + p.metadata_json.len()).sum::<usize>();
    println!(
        "  doc: {} | pages: {} | bytes to upload: {}",
        bundle.doc_uuid,
        bundle.pages.len(),
        total_bytes
    );

    println!("Uploading via cloud sync v3...");
    let client = crate::sync_v3::SyncClient::new(token).context("build sync client")?;
    let result = client
        .upload_bundle(&bundle)
        .await
        .context("upload bundle")?;
    println!(
        "{} doc {} (root gen {})",
        "✓".green(),
        result.doc_id,
        result.new_generation
    );
    Ok(())
}

/// Pull the first markdown H1 as a title, if there is one.
fn extract_h1_title(md: &str) -> Option<String> {
    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
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

/// Load config, refresh the user token if expired, and hand back the bearer.
///
/// Same auth flow as [`cloud_client`] / [`handle_push`], but without
/// constructing a [`CloudClient`] — callers that talk to `/sync/v3/*`
/// directly don't need the tectonic-rewritten base URL.
async fn ensure_fresh_token() -> Result<String> {
    let mut cfg = Config::load()?;
    if !cfg.is_authenticated() {
        bail!("not authenticated; run `rr auth` first");
    }
    if cfg.token_expired() && !cfg.refresh_token().await? {
        bail!("token refresh failed; run `rr auth` to re-pair");
    }
    Ok(cfg
        .get_token()
        .context("missing token after refresh")?
        .to_owned())
}

async fn handle_connect_push(
    file: PathBuf,
    custom_title: Option<String>,
    folder: Option<String>,
    dir: Option<String>,
    format: WireFormat,
    background: bool,
) -> Result<()> {
    if background {
        return spawn_background_connect_push(file, custom_title, folder, dir, format);
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
    let displayed_name = if item.file_name.is_empty() {
        title.as_str()
    } else {
        &item.file_name
    };
    println!("  Name: {}", displayed_name);
    if let Some(p) = item.parent.as_deref() {
        println!("  Parent: {p}");
    }
    Ok(())
}

fn spawn_background_connect_push(
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

    let mut child_args: Vec<String> = vec!["connect-push".into()];
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

    let handle =
        jobs::spawn_detached("connect-push", &child_args).map_err(anyhow::Error::from)?;
    println!("{} Background job {} started.", "✓".green(), handle.id);
    println!("  pid:  {}", handle.pid);
    println!("  log:  {}", handle.log_path.display());
    println!("Watch with: rr logs {}", handle.id);
    Ok(())
}

async fn handle_ls(folders_only: bool) -> Result<()> {
    // Use sync/v3 (the same API push uses) — it's open to every paired
    // account, while /doc/v2/files requires Connect-tier scopes and would
    // 401 here for non-Connect tokens even with a perfectly valid bearer.
    let token = ensure_fresh_token().await?;
    let client = crate::sync_v3::SyncClient::new(token).context("build sync client")?;
    let docs = client.list_documents().await.map_err(map_rr_err)?;
    let docs: Vec<_> = docs
        .into_iter()
        .filter(|d| !d.deleted)
        .filter(|d| !folders_only || d.is_folder())
        .collect();
    if docs.is_empty() {
        println!("No files found.");
        return Ok(());
    }
    println!();
    println!("{:<38} {:<18} Name", "ID", "Type");
    println!("{}", "-".repeat(80));
    for d in &docs {
        let icon = if d.is_folder() { "📁" } else { "📄" };
        let parent = d
            .parent
            .as_deref()
            .map(|p| format!(" [in: {p}]"))
            .unwrap_or_default();
        println!(
            "{:<38} {:<18} {} {}{}",
            d.id, d.doc_type, icon, d.visible_name, parent
        );
    }
    println!();
    println!("Total: {} items", docs.len());
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
        .delete_many(std::slice::from_ref(&target.hash))
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

    // Refresh if needed before probing the cloud. We separate "could not
    // refresh — device pairing is stale" from "refresh worked but the cloud
    // still rejected us" so the user gets actionable output instead of a
    // confusing "token expired" when the bearer is fresh.
    if cfg.token_expired() {
        println!("Refreshing token...");
        if !cfg.refresh_token().await? {
            println!(
                "{} refresh failed — device pairing is stale; run `rr auth` to re-pair",
                "✗".red()
            );
            return Ok(());
        }
        println!("{} token refreshed", "✓".green());
    }

    // Probe sync/v3 — the same endpoint family `rr push` uses, and the only
    // one open to non-Connect tokens. `/doc/v2/files` would 401 for tokens
    // with scope `sync:fox` (the default for free accounts) even on a fresh
    // bearer, which is what the old probe used to mis-report as "token
    // expired".
    let token = cfg.get_token().unwrap_or("").to_owned();
    let sync = crate::sync_v3::SyncClient::new(token).context("build sync client")?;
    match sync.load_root().await {
        Ok(root) => println!(
            "Cloud: {} (root gen {}, {} docs)",
            "ok".green(),
            root.generation,
            root.entries.len()
        ),
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
        "{:<22} {:<10} {:<10} {:<8} STARTED",
        "ID", "KIND", "STATUS", "EXIT"
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

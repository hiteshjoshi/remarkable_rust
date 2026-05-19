use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::api::{FileItem, RemarkableApi, UploadOptions};
use crate::convert::convert_file;

/// Scan a directory for markdown files
pub fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "md" || ext == "markdown" {
                    files.push(path.to_path_buf());
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

/// Sync a directory to reMarkable
pub async fn sync_directory(
    api: &RemarkableApi,
    dir: &Path,
    options: &SyncOptions,
) -> Result<SyncResult> {
    let local_files = scan_markdown_files(dir)?;
    let cloud_files = api.list_files(false).await?;

    // Build index of cloud files by name
    let cloud_by_name: HashMap<String, FileItem> = cloud_files
        .into_iter()
        .map(|f| (f.file_name.clone(), f))
        .collect();

    let mut uploaded = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for local_path in local_files {
        let (title, html) = match convert_file(&local_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error converting {:?}: {}", local_path, e);
                errors += 1;
                continue;
            }
        };

        // Check if file already exists in cloud
        let file_name = format!("{}.html", title);
        if cloud_by_name.contains_key(&file_name) && !options.force {
            println!("  Skipping {} (already exists)", title);
            skipped += 1;
            continue;
        }

        let upload_opts = UploadOptions {
            parent: options.folder.clone(),
            as_notebook: options.as_notebook,
            title: title.clone(),
        };

        match api.upload_html(&title, &html, &upload_opts).await {
            Ok(file) => {
                println!("  Uploaded: {} (ID: {})", title, file.id);
                uploaded += 1;
            }
            Err(e) => {
                eprintln!("  Failed to upload {}: {}", title, e);
                errors += 1;
            }
        }

        // Small delay to avoid rate limiting
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Ok(SyncResult {
        uploaded,
        skipped,
        errors,
    })
}

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub folder: String,
    pub as_notebook: bool,
    pub force: bool,
}

#[derive(Debug)]
pub struct SyncResult {
    pub uploaded: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Watch a directory for changes and sync
pub async fn watch_and_sync(
    api: RemarkableApi,
    dir: PathBuf,
    options: SyncOptions,
) -> Result<()> {
    use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .context("Failed to create file watcher")?;

    watcher
        .watch(&dir, RecursiveMode::Recursive)
        .context("Failed to watch directory")?;

    println!("Watching {:?} for changes... (Press Ctrl+C to stop)", dir);

    let mut last_sync = std::time::Instant::now() - Duration::from_secs(60);

    loop {
        match rx.recv() {
            Ok(event) => {
                // Only react to markdown file changes
                let is_md = event.paths.iter().any(|p| {
                    p.extension()
                        .map(|e| e == "md" || e == "markdown")
                        .unwrap_or(false)
                });

                if is_md && last_sync.elapsed() > Duration::from_secs(5) {
                    println!("\nChange detected, syncing...");
                    match sync_directory(&api, &dir, &options).await {
                        Ok(result) => {
                            println!(
                                "Sync complete: {} uploaded, {} skipped, {} errors",
                                result.uploaded, result.skipped, result.errors
                            );
                        }
                        Err(e) => {
                            eprintln!("Sync failed: {}", e);
                        }
                    }
                    last_sync = std::time::Instant::now();
                }
            }
            Err(e) => {
                eprintln!("Watch error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

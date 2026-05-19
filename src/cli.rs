use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rr")]
#[command(about = "Sync markdown files to your reMarkable tablet")]
#[command(version = "0.1.0")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate with reMarkable cloud
    Auth {
        /// Manually provide an access token (from Chrome extension)
        #[arg(short, long)]
        token: Option<String>,

        /// Open browser helper page with instructions
        #[arg(short, long)]
        open: bool,
    },

    /// Upload a markdown file to reMarkable
    Upload {
        /// Path to markdown file
        file: PathBuf,

        /// Upload as notebook (converted) instead of EPUB
        #[arg(short, long)]
        notebook: bool,

        /// Target folder name in reMarkable (creates if doesn't exist)
        #[arg(short, long)]
        folder: Option<String>,

        /// Target directory path (e.g. "Work/Projects" - creates nested folders)
        #[arg(short, long)]
        dir: Option<String>,

        /// Custom document title
        #[arg(short, long)]
        title: Option<String>,
    },

    /// Sync a directory of markdown files
    Sync {
        /// Directory to sync
        #[arg(default_value = ".")]
        dir: PathBuf,

        /// Watch for changes and auto-sync
        #[arg(short, long)]
        watch: bool,

        /// Upload as notebooks instead of EPUBs
        #[arg(short, long)]
        notebook: bool,

        /// Target folder ID in reMarkable
        #[arg(short, long)]
        folder: Option<String>,
    },

    /// List files in reMarkable cloud
    Ls {
        /// Show detailed information
        #[arg(short, long)]
        long: bool,

        /// Only show folders
        #[arg(short, long)]
        folders: bool,
    },

    /// Create a folder in reMarkable cloud
    Mkdir {
        /// Folder name
        name: String,

        /// Parent folder ID (empty for root)
        #[arg(short, long)]
        parent: Option<String>,
    },

    /// Delete a file or folder from reMarkable cloud
    Rm {
        /// File or folder ID to delete
        id: String,
    },

    /// Show current authentication status
    Status,

    /// Test saved tokens against reMarkable API (debug)
    Test,

    /// Logout and remove stored credentials
    Logout,

    /// Install agent skills for Claude, OpenCode, or Codex
    Skills {
        /// Target agent: claude, opencode, codex, or all
        #[arg(short, long, default_value = "all")]
        target: String,

        /// Show what would be installed without actually doing it
        #[arg(short, long)]
        dry_run: bool,
    },
}

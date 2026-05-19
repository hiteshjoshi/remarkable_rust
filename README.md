# rr — Sync Markdown to reMarkable

A Rust CLI tool that uploads markdown files to your reMarkable Paper Pro tablet as PDFs. Works with the reMarkable cloud API — no SSH or developer mode required.

## Features

- ✅ **Upload markdown** — Converts to PDF and uploads to cloud
- ✅ **Create folders** — Organize your documents
- ✅ **Nested directories** — `rr upload file.md --dir "Work/2024"`
- ✅ **Agent skills** — Install skills for Claude, OpenCode, Codex so AI agents can push content to your tablet
- ✅ **Cross-platform** — macOS (Intel & Apple Silicon), Linux (x86_64 & ARM64)
- ✅ **No Rust required** — Install pre-built binaries without Cargo

## Quick Install

### macOS / Linux (No Rust/Cargo needed)

```bash
curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/reMarkable-rust/main/install.sh | bash
```

This downloads the latest pre-built binary for your platform and installs agent skills.

### Or install to a custom directory

```bash
curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/reMarkable-rust/main/install.sh | INSTALL_DIR=$HOME/bin bash
```

### From source (requires Rust)

```bash
git clone https://github.com/hiteshjoshi/reMarkable-rust.git
cd reMarkable-rust
./install.sh --dev
```

## Usage

### First-time setup

```bash
rr auth
```

Shows a one-time code. Enter it at `https://my.remarkable.com` to pair your device.

### Upload a file

```bash
rr upload notes.md                    # Upload to root
rr upload notes.md --folder "Notes"    # Upload to folder
rr upload notes.md --dir "Work/2024"   # Create nested folders and upload
```

### List documents

```bash
rr ls
```

### Create folders

```bash
rr mkdir "Projects"
rr mkdir "Meeting Notes"
```

### Install AI agent skills

```bash
rr skills --target all        # Install for all agents
rr skills --target claude     # Install only for Claude
rr skills --target opencode   # Install only for OpenCode
rr skills --target codex      # Install only for Codex
```

Once installed, your AI agents will automatically know how to push content to your reMarkable when you say things like:
- "Push this to my remarkable"
- "Send these notes to my tablet"
- "Save this for remarkable"

## How It Works

1. Converts markdown → PDF using `markdown2pdf`
2. Uploads PDF to reMarkable cloud via Sync API v3
3. Creates folders and metadata via cloud API
4. Optionally installs agent skills so AI agents can upload content

## Requirements

- reMarkable Paper Pro (or any reMarkable with cloud sync)
- reMarkable account with cloud sync enabled
- macOS 11+ or Linux

## Platform Support

| Platform | Architecture | Status |
|----------|-------------|--------|
| macOS | Apple Silicon (ARM64) | ✅ |
| macOS | Intel (x86_64) | ✅ |
| Linux | x86_64 | ✅ |
| Linux | ARM64 | ✅ |

## Limitations

- One-way sync only (local → cloud, no download)
- Uploads create new documents (no update-in-place)
- Markdown is converted to PDF, not native notebook format
- Requires cloud connectivity

## License

MIT

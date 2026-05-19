#!/usr/bin/env bash
set -euo pipefail

# rr installer - downloads pre-built binaries from GitHub releases
# Usage: curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/reMarkable-rust/main/install.sh | bash

REPO="hiteshjoshi/reMarkable-rust"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() { printf "${BLUE}ℹ${NC} %s\n" "$*"; }
success() { printf "${GREEN}✓${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}⚠${NC} %s\n" "$*"; }
error() { printf "${RED}✗${NC} %s\n" "$*"; exit 1; }

detect_platform() {
    local os arch
    
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    
    case "$os" in
        linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64|aarch64) echo "aarch64-apple-darwin" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        *)
            error "Unsupported OS: $os"
            ;;
    esac
}

get_latest_version() {
    local url="https://api.github.com/repos/${REPO}/releases/latest"
    curl -fsSL "$url" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/'
}

download() {
    local url="$1"
    local output="$2"
    
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$output"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

main() {
    local dev_mode=false
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --dev)
                dev_mode=true
                shift
                ;;
            --dir)
                INSTALL_DIR="$2"
                shift 2
                ;;
            *)
                shift
                ;;
        esac
    done
    
    info "rr installer"
    info "============"
    echo
    
    # Detect platform
    local platform
    platform=$(detect_platform)
    info "Detected platform: $platform"
    
    # Create install directory
    if [ ! -d "$INSTALL_DIR" ]; then
        info "Creating directory: $INSTALL_DIR"
        mkdir -p "$INSTALL_DIR"
    fi
    
    # Check if directory is in PATH
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            warn "$INSTALL_DIR is not in your PATH"
            warn "Add this to your shell profile:"
            warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
    
    local binary_dest="$INSTALL_DIR/rr"
    
    if [ "$dev_mode" = true ]; then
        # Development mode: install from local repo
        info "Development mode: installing from local build"
        
        local repo_dir
        repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
        local binary_source="$repo_dir/target/release/rr"
        
        if [ ! -f "$binary_source" ]; then
            info "Building release binary..."
            cd "$repo_dir" && cargo build --release
        fi
        
        if [ ! -f "$binary_source" ]; then
            error "Build failed. Make sure Rust/Cargo is installed."
        fi
        
        info "Installing binary from: $binary_source"
        cp "$binary_source" "$binary_dest"
        chmod +x "$binary_dest"
        
        # Copy skills directory next to binary
        local skills_source="$repo_dir/skills"
        local skills_dest="$INSTALL_DIR/skills"
        if [ -d "$skills_source" ]; then
            info "Copying skills directory..."
            rm -rf "$skills_dest"
            cp -r "$skills_source" "$skills_dest"
        fi
        
        # Install agent skills
        if [ -d "$skills_dest" ]; then
            info "Installing agent skills..."
            "$binary_dest" skills --target all || warn "Skills installation failed, but binary is installed"
        fi
        
        local version="dev"
    else
        # Production mode: download from GitHub releases
        local version
        version=$(get_latest_version)
        if [ -z "$version" ]; then
            error "Could not determine latest version. Are you connected to the internet?"
        fi
        info "Latest version: $version"
        
        # Download binary
        local tmp_dir
        tmp_dir=$(mktemp -d)
        local archive="rr-${platform}.tar.gz"
        local download_url="https://github.com/${REPO}/releases/download/${version}/${archive}"
        
        info "Downloading from: $download_url"
        if ! download "$download_url" "$tmp_dir/$archive"; then
            error "Download failed. The release may not exist yet for your platform."
        fi
        
        # Extract
        info "Extracting archive..."
        tar xzf "$tmp_dir/$archive" -C "$tmp_dir"
        
        # Install binary
        local binary_source="$tmp_dir/rr-${platform}/rr"
        
        info "Installing binary to: $binary_dest"
        cp "$binary_source" "$binary_dest"
        chmod +x "$binary_dest"
        
        # Copy skills directory next to binary
        local skills_source="$tmp_dir/rr-${platform}/skills"
        local skills_dest="$INSTALL_DIR/skills"
        if [ -d "$skills_source" ]; then
            info "Copying skills directory..."
            rm -rf "$skills_dest"
            cp -r "$skills_source" "$skills_dest"
        fi
        
        # Install agent skills
        if [ -d "$skills_dest" ]; then
            info "Installing agent skills..."
            "$binary_dest" skills --target all || warn "Skills installation failed, but binary is installed"
        fi
        
        # Cleanup
        rm -rf "$tmp_dir"
    fi
    
    # Verify
    if command -v rr >/dev/null 2>&1 || [ -x "$binary_dest" ]; then
        success "rr $version installed successfully!"
        echo
        info "Run 'rr auth' to authenticate with your reMarkable tablet"
        info "Run 'rr --help' to see all commands"
        
        if ! command -v rr >/dev/null 2>&1; then
            echo
            warn "rr is installed at $binary_dest but not in your PATH"
            warn "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
            warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
        fi
    else
        error "Installation verification failed"
    fi
}

# Run main if script is executed directly
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    main "$@"
fi

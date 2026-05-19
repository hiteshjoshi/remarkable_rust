#!/usr/bin/env bash
# Build release tarballs locally and (optionally) upload them to a GitHub
# release. Use this when GitHub Actions is unavailable.
#
# Usage:
#   scripts/build-release.sh                       # build only, no upload
#   scripts/build-release.sh --tag v0.2.0 --upload # build + create/update release
#   scripts/build-release.sh --dist out            # custom artifact directory
#
# Required:
#   - rustup, cargo (always)
#   - For Linux cross-builds on macOS, install ONE of:
#       brew install zig && cargo install --locked cargo-zigbuild   (lighter, no Docker)
#       cargo install --locked cross                                 (uses Docker)
#   - For --upload: gh CLI (brew install gh).

set -euo pipefail

TAG=""
UPLOAD=false
DIST=dist

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag)    TAG="$2"; shift 2 ;;
        --upload) UPLOAD=true; shift ;;
        --dist)   DIST="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,15p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

has() { command -v "$1" >/dev/null 2>&1; }

note()  { printf '\033[34m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[33mwarn:\033[0m %s\n' "$*" >&2; }
fatal() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

mkdir -p "$DIST"
# Wipe stale artifacts so the SHA256SUMS roll-up matches what we just built.
find "$DIST" -maxdepth 1 -type f \( -name '*.tar.gz' -o -name '*.sha256' -o -name 'SHA256SUMS' \) -delete

# Resolve the version from Cargo.toml (only used for log lines).
version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.+)"/\1/')"
note "rr v$version"

build_target() {
    local target="$1" builder="$2"
    note "building $target via $builder"
    case "$builder" in
        cargo)
            rustup target add "$target" >/dev/null 2>&1 || true
            cargo build --release --locked --target "$target"
            ;;
        zigbuild)
            rustup target add "$target" >/dev/null 2>&1 || true
            cargo zigbuild --release --locked --target "$target"
            ;;
        cross)
            cross build --release --locked --target "$target"
            ;;
        *) fatal "unknown builder: $builder" ;;
    esac

    local stage="rr-$target"
    rm -rf "${DIST:?}/$stage"
    mkdir -p "$DIST/$stage"
    cp "target/$target/release/rr" "$DIST/$stage/"
    cp -r skills "$DIST/$stage/"
    [[ -f README.md ]]                  && cp README.md                  "$DIST/$stage/" || true
    [[ -f docs/formatting-guide.md ]]   && cp docs/formatting-guide.md   "$DIST/$stage/" || true

    (cd "$DIST" && tar -czf "$stage.tar.gz" "$stage")
    rm -rf "$DIST/$stage"

    if has sha256sum; then
        (cd "$DIST" && sha256sum "$stage.tar.gz") > "$DIST/$stage.tar.gz.sha256"
    else
        (cd "$DIST" && shasum -a 256 "$stage.tar.gz") > "$DIST/$stage.tar.gz.sha256"
    fi
}

os="$(uname -s)"

if [[ "$os" == "Darwin" ]]; then
    # Both Apple targets build natively from any Mac with the right Rust target installed.
    build_target aarch64-apple-darwin cargo
    build_target x86_64-apple-darwin  cargo
elif [[ "$os" == "Linux" ]]; then
    build_target x86_64-unknown-linux-gnu  cargo
    build_target aarch64-unknown-linux-gnu cargo
else
    warn "unrecognised host OS $os — building only the native target"
    cargo build --release --locked
fi

# Linux cross-builds from macOS / non-Linux: prefer zigbuild (no Docker).
linux_builder=""
if [[ "$os" != "Linux" ]]; then
    if has cargo-zigbuild && has zig; then
        linux_builder=zigbuild
    elif has cross; then
        linux_builder=cross
    fi
    if [[ -n "$linux_builder" ]]; then
        build_target x86_64-unknown-linux-gnu  "$linux_builder"
        build_target aarch64-unknown-linux-gnu "$linux_builder"
    else
        warn "skipping Linux targets (install: brew install zig && cargo install --locked cargo-zigbuild)"
    fi
fi

# Combined SHA256SUMS file (one line per archive, GitHub-release style).
: > "$DIST/SHA256SUMS"
for f in "$DIST"/*.tar.gz.sha256; do
    [[ -e "$f" ]] || continue
    cat "$f" >> "$DIST/SHA256SUMS"
done

note "artifacts:"
ls -lh "$DIST"/*.tar.gz "$DIST/SHA256SUMS" 2>/dev/null || true

if [[ "$UPLOAD" == "true" ]]; then
    [[ -n "$TAG" ]] || fatal "--upload requires --tag <vX.Y.Z>"
    has gh || fatal "gh CLI not installed (brew install gh)"

    if gh release view "$TAG" >/dev/null 2>&1; then
        note "release $TAG exists — uploading (--clobber)"
        gh release upload "$TAG" \
            "$DIST"/*.tar.gz "$DIST"/*.sha256 "$DIST/SHA256SUMS" \
            --clobber
    else
        note "creating release $TAG"
        gh release create "$TAG" \
            --title "rr $TAG" \
            --generate-notes \
            "$DIST"/*.tar.gz "$DIST"/*.sha256 "$DIST/SHA256SUMS"
    fi
fi

note "done"

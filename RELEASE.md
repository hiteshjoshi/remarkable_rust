# Release Checklist

## Creating a Release

### 1. Update version

```bash
# Edit Cargo.toml and bump version
vim Cargo.toml
```

### 2. Update CHANGELOG

```bash
vim CHANGELOG.md
```

### 3. Commit and tag

```bash
git add Cargo.toml CHANGELOG.md
git commit -m "Release v0.1.0"
git tag v0.1.0
git push origin main --tags
```

### 4. GitHub Actions builds binaries

The `.github/workflows/release.yml` workflow will:
- Build for macOS (Intel + Apple Silicon)
- Build for Linux (x86_64 + ARM64)
- Create GitHub release with binaries attached

### 5. Verify release

```bash
# Download and test
curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/reMarkable-rust/main/install.sh | bash
```

## Manual Release (if CI fails)

```bash
# Build for all platforms locally
cargo build --release

# Create package
mkdir -p rr-aarch64-apple-darwin
cp target/release/rr rr-aarch64-apple-darwin/
cp -r skills rr-aarch64-apple-darwin/
tar czf rr-aarch64-apple-darwin.tar.gz rr-aarch64-apple-darwin

# Upload to GitHub release manually
```

## Post-Release

- [ ] Update README with new version
- [ ] Test install script on clean machine
- [ ] Test on macOS Intel
- [ ] Test on Linux
- [ ] Verify agent skills install correctly
- [ ] Close related issues

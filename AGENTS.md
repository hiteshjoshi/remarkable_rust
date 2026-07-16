# Project Memory

## Security invariants

- Never persist authentication tokens outside `config.toml` and the platform keyring.
- `rr logout` must remove both current credentials and the legacy `tokens.json` file.
- On Unix, the configuration directory must be mode `0700` and `config.toml` mode `0600`.
- Signed notebook upload redirects must use HTTPS and an approved cloud-storage hostname.
- Markdown image embedding is confined to canonical files beneath the Markdown source directory; absolute paths, traversal, symlink escapes, and files over 10 MiB are rejected.
- Release installation must stop when SHA-256 verification is unavailable, missing, or mismatched.

## Verification

Run before completing changes:

```bash
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked --all-targets
cargo clippy --all-targets --locked -- -D warnings
```

The `cloud_api::tests::create_folder_sends_content_length_zero` test binds a localhost ephemeral port and may require network permission in a restricted sandbox.

## Security backlog

- CircleCI still downloads rustup and Zig in release jobs. Pin exact tool versions and verify official checksums before extraction or execution. Keep release credentials isolated from build steps.

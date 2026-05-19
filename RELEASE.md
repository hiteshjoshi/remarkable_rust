# Release checklist

Three paths are supported, in roughly decreasing preference:

- **CircleCI** (active default — runs on tag push, `.circleci/config.yml`)
- **GitHub Actions** (parked under `.github/workflows-disabled/`,
  re-enable with a single `git mv` if you'd rather use Actions)
- **Local build + manual upload** (no CI required, useful for ad-hoc
  releases or when you want full control)

---

## Path A — CircleCI

A working CircleCI config lives at `.circleci/config.yml`. It builds the
same four targets via two jobs (macOS M1 covers both Apple targets
natively, Linux x86_64 covers both Linux targets via `cargo-zigbuild`),
then publishes the GitHub release with the `gh` CLI.

### One-time setup

1. Connect the repo on https://app.circleci.com (free plan).
2. Create a fine-grained GitHub PAT with `contents:write` on the repo.
3. In CircleCI, create a **context** named `rr-release` and add the PAT
   as `GITHUB_TOKEN`. The publish job references this context.

### Triggering

```bash
git tag v0.2.0
git push origin v0.2.0
```

CircleCI's filter is `tags: only: /^v.+/`, so only tag pushes run the
workflow. ~6–8 min end-to-end.

---

## Path B — Local build + manual upload

Useful for ad-hoc releases, or when you want the whole pipeline on your
laptop.

### Prerequisites

- `rustup`, `cargo`
- For Linux cross-builds from macOS, install **one** of:
  - `brew install zig && cargo install --locked cargo-zigbuild` (lighter, no Docker)
  - `cargo install --locked cross` (uses Docker)
- For upload: `gh` (`brew install gh && gh auth login`)

### Build only

```bash
./scripts/build-release.sh
```

Drops tarballs + `.sha256` files + a combined `SHA256SUMS` into `./dist/`.

### Build and upload to a GitHub release

```bash
git tag v0.2.0
git push origin v0.2.0          # tag must exist on the remote first
./scripts/build-release.sh --tag v0.2.0 --upload
```

If the release already exists for that tag, the script uploads with
`--clobber` (re-uploads, overwrites). Otherwise it creates the release
with auto-generated notes.

### Verifying locally

```bash
ls -lh dist/

INSTALL_DIR=/tmp/rr-test ./install.sh
/tmp/rr-test/rr --help
```

---

## Path C — GitHub Actions (parked)

The Actions workflows live under `.github/workflows-disabled/`. To put
the project back on Actions:

```bash
git mv .github/workflows-disabled/ci.yml      .github/workflows/ci.yml
git mv .github/workflows-disabled/release.yml .github/workflows/release.yml
git commit -m "ci: re-enable GitHub Actions"
git push
```

Both workflows fire on the next push / tag. You can keep CircleCI in
place — they don't conflict.

---

## Other free CI alternatives (not configured here)

- **Cirrus CI** — free for public repos, native macOS + Linux runners.
- **GitLab CI mirror** — push the repo as a mirror, run CI there,
  pull artifacts back.

---

## Post-release sanity

- [ ] `curl | bash` install works on a clean machine
- [ ] `rr auth` flow works
- [ ] `rr upload examples/format-test.md` produces a native notebook on
      the tablet
- [ ] Test on macOS Intel + Apple Silicon, Linux x86_64 + ARM64

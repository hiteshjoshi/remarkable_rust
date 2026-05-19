# GitHub Actions workflows (parked)

These workflows are kept out of `.github/workflows/` for now. The
project's primary release pipeline runs on **CircleCI** — see
`../../.circleci/config.yml` and `../../RELEASE.md`.

## Re-enabling

To switch the project back to GitHub Actions:

```bash
git mv .github/workflows-disabled/ci.yml      .github/workflows/ci.yml
git mv .github/workflows-disabled/release.yml .github/workflows/release.yml
git commit -m "ci: re-enable GitHub Actions"
git push
```

Both workflows fire on the next push / tag, exactly as before. You can
leave the CircleCI config in place — they don't conflict.

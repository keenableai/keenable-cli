# Release Process

## Cutting a release (automated)

One command, from any machine with `gh` logged in:

```bash
tools/release.sh patch      # or minor, major, or an exact version like 0.6.0
```

It runs the **Cut release** workflow (bumps `Cargo.toml` + `Cargo.lock` on a `release/vX.Y.Z` branch and runs CI), opens the PR as you, waits for checks and the Qodo approval, squash-merges it, and waits for the release. The push to `main` tags `vX.Y.Z` and dispatches the release workflow below on that tag. Pass `--yes` to skip the confirmation, `--dry-run` to see the version only.

Without the script: Actions -> **Cut release** -> Run workflow, then open the PR from the `release/vX.Y.Z` branch yourself and merge it. The workflow doesn't open the PR itself: a PR authored by `github-actions[bot]` can't be approved by the approval bridge, which also runs as that bot.

No tokens: the workflow uses the default `GITHUB_TOKEN`, so `release.yml` has a hand-added `workflow_dispatch` trigger (`allow-dirty = ["ci"]` in `dist-workspace.toml`); re-add it if you ever run `dist generate`.

## Cutting a release (manual)

1. **Bump the version** in `Cargo.toml`:

   ```bash
   # Install cargo-edit if you don't have it: cargo install cargo-edit
   cargo set-version 0.2.0
   ```

2. **Commit and tag**:

   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "Release v0.2.0"
   git tag v0.2.0
   git push && git push --tags
   ```

3. **Watch the release workflow**: https://github.com/keenableai/keenable-cli/actions

The workflow will:
- Build binaries for macOS (Intel + Apple Silicon), Linux (x86_64 + ARM), Windows (x86_64)
- Generate shell and PowerShell installer scripts
- Create a GitHub Release with all artifacts and checksums
- Publish an updated Homebrew formula to `keenableai/homebrew-tap`

## Prerelease

Tag with a prerelease suffix to create a prerelease on GitHub (won't update Homebrew):

```bash
git tag v0.3.0-beta.1
git push --tags
```

## Regenerating CI

If you change `dist-workspace.toml`, regenerate the workflow:

```bash
dist generate
```

Then commit the updated `.github/workflows/release.yml`.

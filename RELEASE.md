# Release Process

## Cutting a release

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

# CI & Releases

## Continuous integration

CI runs automatically on every push and pull request via
`.github/workflows/ci.yml`.

### CI pipeline

| Step | Command |
|---|---|
| Checkout | `actions/checkout@v4` |
| Rust toolchain | `dtolnay/rust-toolchain@stable` |
| Cache | `Swatinem/rust-cache@v2` |
| Lint | `cargo clippy --tests -- -D warnings` |
| Test | `cargo test` |

CI must pass before merging any PR.

## Release pipeline

Releases are triggered by pushing a version tag. The workflow is defined in
`.github/workflows/release.yml`.

### Triggering a release

```sh
# 1. Update version in Cargo.toml
# 2. Commit the version bump
# 3. Tag and push
git tag v0.3.0
git push origin v0.3.0
```

### Release pipeline steps

1. **Lint & Test** -- same as CI (clippy + tests)
2. **Build** -- compiles release binaries for 4 targets:

| Target | Runner | Archive |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| `x86_64-apple-darwin` | `macos-latest` | `.tar.gz` |
| `aarch64-apple-darwin` | `macos-latest` | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |

3. **Package** -- creates archives (`tar.gz` on Unix, `zip` on Windows)
4. **Release** -- creates a GitHub Release with auto-generated changelog and
   attached archives (via `softprops/action-gh-release@v2`)

### Version tags

Use semantic versioning: `v0.1.0`, `v0.2.0`, `v1.0.0`.

Remember to update the `version` field in `Cargo.toml` before creating the tag.

## Install scripts

Two install scripts are provided in the repository root:

### `install.sh` (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

Downloads the latest release binary for the detected platform and checks for
signal-cli.

### `install.ps1` (Windows)

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Downloads the latest Windows release binary and checks for signal-cli.

## Documentation deployment

Documentation is built and deployed via `.github/workflows/docs.yml`. See the
docs workflow for details on how changes to the `docs/` directory trigger a
rebuild and deployment to GitHub Pages.

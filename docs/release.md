# Release Process

Tagged releases run all target CI workflows first, then publish release assets
through [`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Version Preflight

Before pushing a release tag, update the `version` in `Cargo.toml` to match
the tag without the leading `v`, then update `Cargo.lock`. The Cargo package
version is the source of truth for packaged app metadata, including Windows
`FileVersion` / `ProductVersion` and the macOS bundle version.

For example, a `v1.0.0` tag must be created from a commit where `Cargo.toml`
and the root `fast-diff-viewer` entry in `Cargo.lock` both say `1.0.0`.
Before publishing, verify the built Windows executable reports matching
`FileVersion` and `ProductVersion`, and verify the macOS `Info.plist` uses the
same version.

## Assets

Each release publishes these assets:

- `fast-diff-viewer-windows-x64.zip`
- `fast-diff-viewer-ubuntu-22.04-x64.tar.gz`
- `fast-diff-viewer-ubuntu-24.04-x64.tar.gz`
- `fast-diff-viewer-macos-26-arm64.tar.gz`
- `SHA256SUMS.txt`

## Package Contents

All binary packages include these legal files at the archive root:

- `LICENSE-MIT`
- `LICENSE-APACHE`
- `THIRD-PARTY-NOTICES.md`

`THIRD-PARTY-NOTICES.md` is generated during release builds with
`cargo-about 0.9.0` from the locked runtime dependency graph. Build-only and
dev-only dependencies are excluded.

`SHA256SUMS.txt` is generated after final release artifacts are downloaded into
the publish job, so the checksums cover the exact public archive bytes.

The Windows zip places `fast-diff-viewer.exe` at the archive root.

Ubuntu tarballs place `fast-diff-viewer` at the archive root and include
desktop integration files under `share/applications/` and
`share/icons/hicolor/scalable/apps/`.

The macOS tarball contains `Fast Diff Viewer.app`, an unsigned personal-use app
bundle with the native executable under `Contents/MacOS/` and the app icon
under `Contents/Resources/`.

For public releases, the macOS bundle is intentionally shipped unsigned and
not notarized unless the release plan explicitly says otherwise. Browser
downloads will usually receive the `com.apple.quarantine` attribute, so
Gatekeeper may block first launch and report that the developer cannot be
verified. Keep the README install instructions explicit about that behavior:
users should verify `SHA256SUMS.txt`, make their own trust decision, and then
use Finder's Control-click Open flow or clear quarantine with
`xattr -dr com.apple.quarantine "Fast Diff Viewer.app"` before launching.

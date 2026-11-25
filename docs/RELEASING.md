# Releasing

What a **Release** must look like for the self-updater to offer it as an **Update**, and how Releases are built. Pushing a `v<ver>` tag builds all three platforms in GitHub Actions and publishes the Release ([`.github/workflows/release.yml`](../.github/workflows/release.yml)). Why the updater trusts GitHub's digest: [ADR 0001](adr/0001-self-update-trusts-github-digest.md). How each platform installs: [ADR 0002](adr/0002-per-platform-update-install.md).

## The contract

Every Release carries one asset per platform. The updater looks only for its own platform's asset and installs it only if rules 1-3 hold (checked before offering Update now); rule 4 holds when installing:

1. The tag is exactly `v<ver>` (a single lowercase `v`, then `major.minor.patch`). The updater builds the expected asset name from the tag's version. That `<ver>` also equals the app version in the workspace manifest (`[workspace.package] version` in the root `Cargo.toml`) is enforced by the release workflow, not by the updater.
2. Exactly one asset is *named* as below for the platform. Other, differently named assets are tolerated; two assets with that name make the Release announce-only.

   | Platform | Asset | Contains |
   |---|---|---|
   | macOS (Apple Silicon) | `MacMischer-<ver>-macos-arm64.dmg` | top-level `MacMischer.app` |
   | Windows (x64) | `MacMischer-<ver>-windows-x64-setup.exe` | Inno Setup installer (per-user, silent-capable) |
   | Linux (x86_64) | `MacMischer-<ver>-linux-x86_64.AppImage` | an executable AppImage (ELF) |
3. GitHub publishes a `sha256:<hex>` digest for that asset.
4. The asset installs: a DMG without a top-level `MacMischer.app`, or an AppImage that is not an ELF file, fails the install and leaves the installed app untouched (the banner shows an error with Try again). The Windows setup runs detached after the app quits, so a failing setup is not reported in the app; the old version stays installed.

A Release that breaks rule 1, 2 or 3 for a platform is **announce-only** there: the updater tells the Operator it exists but never installs it. The updater also falls back to announce-only when the running app cannot be replaced in place: a macOS app outside a writable `.app`, a Linux binary that is not an AppImage (or in a read-only folder), a Windows exe that was not installed by the setup program (no `unins000.exe` beside it, e.g. `cargo run`).

## Making a Release

1. Bump `version` under `[workspace.package]` in `Cargo.toml`; commit and push it.
2. Tag and push:
   ```sh
   git tag "v0.1.4" && git push origin "v0.1.4"
   ```
3. The workflow runs the tests, builds the three assets in parallel, and only when all three exist creates the GitHub Release (notes from `tools/release/notes.sh`: the commit subjects since the previous tag, grouped into "What's new" and "Fixes") and checks that GitHub published a digest for each asset. If it fails before publishing, nothing is released; fix and re-tag (delete the tag first).
4. Write commit subjects as user-facing sentences ("Fix …" lands under Fixes; version bumps, merges, docs/test/ci/chore commits are left out). Edit the Release notes on GitHub if you want them nicer: the update window shows them.

## Building an asset by hand

Each asset has a script, which the workflow also uses. Run from the repo root; outputs land in `dist/` (git-ignored).

- macOS (Apple Silicon): `cargo install cargo-bundle` once, then `tools/macos/build-dmg.sh` (bundles, ad hoc signs, makes the DMG). `tools/install-macos.sh` is for installing locally, not for releasing.
- Windows: `cargo build --release -p macmischer`, then with [Inno Setup 6](https://jrsoftware.org/isinfo.php) installed, set `MM_VERSION` to the version and run `iscc tools\windows\macmischer.iss`.
- Linux: `cargo build --release -p macmischer`, then `tools/linux/build-appimage.sh` (needs [`appimagetool`](https://github.com/AppImage/appimagetool) on `PATH` or in `APPIMAGETOOL`). The AppImage uses the host's graphics, ALSA and windowing libraries; it is built on Ubuntu 22.04 so it runs on that and newer.

Upload by hand only if the workflow is unavailable: `gh release create "v$VER" dist/* --title "v$VER"`, then verify the digests:

```sh
gh release view "v$VER" --json assets --jq '.assets[] | {name, digest}'
```

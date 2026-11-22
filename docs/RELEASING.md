# Releasing

What a **Release** must look like for the self-updater to offer it as an **Update**, and how the DMG is built by hand today. There is no CI; releases are made manually. Why the updater trusts GitHub's digest: [ADR 0001](adr/0001-self-update-trusts-github-digest.md).

## The contract

The updater installs a Release only if rules 1-3 hold (checked before offering Update now) and rule 4 holds when installing:

1. The tag is exactly `v<ver>` (a single lowercase `v`, then `major.minor.patch`). The updater enforces this form, and builds the expected asset name from the tag's version. That `<ver>` also equals the app version in the workspace manifest (`[workspace.package] version` in the root `Cargo.toml`) is a release-checklist item, not enforced by the updater.
2. Exactly one asset is *named* `MacMischer-<ver>-macos-arm64.dmg`. Other, differently named assets are tolerated; two assets with that name make the Release announce-only.
3. GitHub publishes a `sha256:<hex>` digest for that asset.
4. The DMG contains a top-level `MacMischer.app`.

A Release that breaks rule 1, 2 or 3 is **announce-only**: the updater tells the Operator it exists but never installs it. Rule 4 is only discovered at install time: a DMG without a top-level `MacMischer.app` fails the install, the installed app is rolled back untouched, and the banner shows an error with Try again. It is not announce-only.

## Checklist

1. Bump `version` under `[workspace.package]` in `Cargo.toml` (the app crate inherits it); commit it.
2. Build the bundle (on an Apple Silicon Mac, since the asset is `arm64`):
   ```sh
   cargo install cargo-bundle    # once
   cargo bundle --release -p macmischer
   codesign --force --deep -s - target/release/bundle/osx/MacMischer.app
   ```
   This is what `tools/install-macos.sh` does, except that script also installs to `/Applications` and deletes `target/release/bundle`, so do not use it to produce the release bundle. The ad hoc signature matches the script; the app is not notarised.
3. Create the DMG with `MacMischer.app` at its top level. `tools/install-macos.sh` does not do this step, and no script in the repo does; the manual command below was run once on macOS (it prints a deprecation warning from `hdiutil` but works, and the resulting DMG mounts with `MacMischer.app` at its top level):
   ```sh
   VER=0.1.1   # must equal the manifest version
   STAGE=$(mktemp -d)
   cp -R target/release/bundle/osx/MacMischer.app "$STAGE/"
   hdiutil create -volname MacMischer -srcfolder "$STAGE" -ov -format UDZO \
     "MacMischer-$VER-macos-arm64.dmg"
   ```
4. Tag and create the Release with that single asset:
   ```sh
   git tag "v$VER" && git push origin "v$VER"
   gh release create "v$VER" "MacMischer-$VER-macos-arm64.dmg" --title "v$VER" --notes "..."
   ```
5. After upload, verify GitHub shows the digest (rule 3):
   ```sh
   gh release view "v$VER" --json assets --jq '.assets[] | {name, digest}'
   ```
   `digest` must be `sha256:<hex>`. If it is missing or the asset name is wrong, fix or re-upload before announcing; until then the Release is announce-only.

# Each platform installs its Update with its own native package

Releases ship one asset per platform (DMG, Inno Setup installer, AppImage) and the Updater installs the one for the platform it runs on, after the Operator confirms (see ADR 0001 for the macOS in-place swap and the digest check, which apply to all three assets).

- **macOS** swaps the `.app` bundle from the mounted DMG, as before.
- **Linux** replaces the running AppImage file (`$APPIMAGE`) with the downloaded one (keeping the old one until the new one starts) and launches it. Only offered when running from an AppImage in a writable folder.
- **Windows** runs the downloaded setup program silently and detached, then the app quits; the setup closes any leftover instance and starts the new one. The installer is per-user, so no elevation prompt is needed. Only offered when the exe was installed by that setup program.

We chose native packages over a bare zip/tarball because they are what users expect to install, at the cost of a Windows install the app cannot observe once it has quit: a failing setup is not reported back. Windows and Linux builds are not code-signed, so Windows SmartScreen may warn on the first manual install (the in-app Update downloads the file itself, so it is not subject to the mark-of-the-web prompt). Revisit signing if the audience grows.

A Release is only published when all three assets exist, so no platform ever sees a Release without its asset.

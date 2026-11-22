# Self-update trusts GitHub's asset digest and strips quarantine

MacMischer is not notarised, so an Update could either send the Operator to drag-install a downloaded DMG (Gatekeeper prompt every time) or swap the app in place itself. We chose the in-place swap: the app downloads the Release DMG, checks its SHA-256 against the `digest` GitHub publishes for the asset, replaces the bundle (keeping the old one until the new one is in place) and relaunches without the quarantine flag.

The digest guards against corrupt downloads, not against a compromised GitHub account or release: there is no separate signing key. Revisit this if the app gains notarisation or a wider audience.

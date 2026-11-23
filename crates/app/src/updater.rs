//! The Updater: checks GitHub for a newer Release at launch and reports its state for the banner.
//! An Update is not an Action; this module is independent of the Console and the mixer.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

const LATEST_URL: &str = "https://api.github.com/repos/ItsSebis/MacMischer/releases/latest";

/// Where the latest Release comes from. Returns the raw JSON body, or None on any failure.
pub trait ReleaseFeed: Send + Sync {
    fn latest(&self) -> Option<String>;
}

/// Opens a URL in the operator's browser.
pub trait Browser: Send + Sync {
    fn open(&self, url: &str);
}

/// Fetches the Release's DMG bytes (about 6 MB). The error string is shown to the Operator.
pub trait DmgSource: Send + Sync {
    fn download(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// The macOS built-ins the install step relies on, plus the file moves around them. Production
/// uses `hdiutil`, `ditto`, `xattr` and `open`; tests fake the mount and run the moves on a temp
/// directory.
pub trait Installer: Send + Sync {
    /// Mount the DMG; returns the folder holding its contents.
    fn mount(&self, dmg: &Path) -> Result<PathBuf, String>;
    /// Unmount; best effort.
    fn unmount(&self, mount: &Path);
    /// Copy a bundle (preserving everything inside it).
    fn copy(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
    /// Remove a bundle (or any folder).
    fn remove(&self, path: &Path) -> Result<(), String>;
    fn strip_quarantine(&self, path: &Path) -> Result<(), String>;
    /// Start the bundle as a new instance.
    fn relaunch(&self, path: &Path) -> Result<(), String>;
    fn volume_read_only(&self, path: &Path) -> bool;
}

/// Installs a downloaded, verified package on platforms without a bundle swap (Windows setup
/// program, Linux AppImage). Does everything including starting the new version; the Updater then
/// reports Relaunching and the app quits.
pub trait PackageInstaller: Send + Sync {
    fn install(&self, package: &Path) -> Result<(), String>;
    /// Remove leftovers of a previous Update; best effort.
    fn cleanup(&self) {}
}

/// The platforms we ship Releases for; each has exactly one asset per Release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacosArm64,
    WindowsX64,
    LinuxX64,
}

impl Platform {
    /// The platform this binary was built for, if we ship Releases for it.
    pub fn current() -> Option<Platform> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Platform::MacosArm64)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Platform::WindowsX64)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Platform::LinuxX64)
        } else {
            None
        }
    }

    /// The Release asset name for `version` (see `docs/RELEASING.md`).
    pub fn asset_name(self, version: &str) -> String {
        match self {
            Platform::MacosArm64 => format!("MacMischer-{version}-macos-arm64.dmg"),
            Platform::WindowsX64 => format!("MacMischer-{version}-windows-x64-setup.exe"),
            Platform::LinuxX64 => format!("MacMischer-{version}-linux-x86_64.AppImage"),
        }
    }
}

/// A Release that meets the contract in `docs/RELEASING.md` and can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub notes: String,
    /// The Release page.
    pub url: String,
    asset_url: String,
    /// Lowercase hex SHA-256 published by GitHub for the DMG asset.
    sha256: String,
}

/// What the banner renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    /// A newer Release that breaks the contract: announce-only, offers Open release page.
    Available { version: String, notes: String, url: String },
    /// A newer Release that meets the contract: offers Update now.
    Updatable(Update),
    Downloading(Update),
    Verifying(Update),
    /// The DMG is on disk and its digest matched. Installing builds on this.
    Ready { update: Update, dmg: PathBuf },
    /// The verified DMG is being installed over the running app.
    Installing(Update),
    /// The new bundle is in place and started; the app should quit now.
    Relaunching(Update),
    /// Download, verification or install failed; Try again is `update_now()`.
    Failed { update: Update, error: String },
}

/// What the banner shows for a State.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BannerView {
    pub version: String,
    pub notes: String,
    pub text: &'static str,
    pub error: Option<String>,
}

impl State {
    /// Display data for the banner; None when nothing is shown.
    pub fn banner(&self) -> Option<BannerView> {
        let view = |version: &str, notes: &str, text, error: Option<&str>| {
            Some(BannerView { version: version.into(), notes: notes.into(), text, error: error.map(String::from) })
        };
        match self {
            State::Idle => None,
            State::Available { version, notes, .. } => view(version, notes, "is available", None),
            State::Updatable(u) => view(&u.version, &u.notes, "is available", None),
            State::Downloading(u) => view(&u.version, "", "Downloading…", None),
            State::Verifying(u) => view(&u.version, "", "Verifying…", None),
            State::Ready { update, .. } => view(&update.version, "", "is downloaded and verified", None),
            State::Installing(u) => view(&u.version, "", "Installing…", None),
            State::Relaunching(u) => view(&u.version, "", "installed, restarting…", None),
            State::Failed { update, error } => view(&update.version, &update.notes, "update failed", Some(error)),
        }
    }

    /// "Later" applies: an offer or a failure the Operator can wave away.
    pub fn is_dismissible(&self) -> bool {
        matches!(self, State::Available { .. } | State::Updatable(_) | State::Failed { .. })
    }

    /// "Skip this version" applies: a fresh offer, not a failure.
    pub fn is_skippable(&self) -> bool {
        matches!(self, State::Available { .. } | State::Updatable(_))
    }

    /// The new bundle is in place and started; the app should quit.
    pub fn should_quit(&self) -> bool {
        matches!(self, State::Relaunching(_))
    }
}

/// The Updater's slice of the preferences file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    /// "Check for updates at launch".
    pub check_at_launch: bool,
    /// A Release the Operator chose to skip; only strictly newer ones are offered.
    pub skipped_version: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs { check_at_launch: true, skipped_version: None }
    }
}

/// Whether and how the Updater can replace the running app.
enum Install {
    /// No install step configured: a verified download stays `Ready`.
    Unwired,
    /// In-place install is impossible here: announcements offer Open release page only.
    Degraded,
    /// macOS: mount the DMG and swap the `.app`.
    Possible { target: PathBuf, installer: Arc<dyn Installer> },
    /// Windows / Linux: hand the verified package to the platform installer.
    Package(Arc<dyn PackageInstaller>),
}

type SavePrefs = Arc<dyn Fn(&UpdatePrefs) + Send + Sync>;

pub struct Updater {
    current: String,
    feed: Arc<dyn ReleaseFeed>,
    browser: Arc<dyn Browser>,
    /// False on platforms we do not ship Releases for: never checks, never shows.
    enabled: bool,
    on_change: Arc<dyn Fn() + Send + Sync>,
    state: State,
    pending: Option<Receiver<State>>,
    /// The check runs once per launch.
    checked: bool,
    dmgs: Arc<dyn DmgSource>,
    /// Where verified DMGs are kept.
    downloads: PathBuf,
    /// The running download job reports Verifying, then Ready or Failed.
    job: Option<Receiver<State>>,
    prefs: UpdatePrefs,
    save_prefs: SavePrefs,
    install: Install,
    platform: Platform,
}

impl Updater {
    pub fn new(current: &str, feed: Arc<dyn ReleaseFeed>, browser: Arc<dyn Browser>, enabled: bool) -> Updater {
        Updater {
            current: current.to_string(),
            feed,
            browser,
            enabled,
            on_change: Arc::new(|| {}),
            state: State::Idle,
            pending: None,
            checked: false,
            dmgs: Arc::new(HttpDmgs),
            downloads: std::env::temp_dir().join("MacMischer-update"),
            job: None,
            prefs: UpdatePrefs::default(),
            save_prefs: Arc::new(|_| {}),
            install: Install::Unwired,
            platform: Platform::MacosArm64,
        }
    }

    /// Which platform's asset to look for (default macOS arm64).
    pub fn with_platform(mut self, platform: Platform) -> Updater {
        self.platform = platform;
        self
    }

    /// Install by handing the package to `installer`; None when the running app cannot be
    /// replaced in place (Releases are then announced with Open release page only).
    pub fn with_package_install(mut self, installer: Option<Arc<dyn PackageInstaller>>) -> Updater {
        self.install = match installer {
            Some(i) => Install::Package(i),
            None => Install::Degraded,
        };
        self
    }

    /// Install into `target`, the running app's bundle (None when not running from one). When
    /// the bundle cannot be replaced in place, Releases are announced with Open release page only.
    pub fn with_install(mut self, target: Option<PathBuf>, installer: Arc<dyn Installer>) -> Updater {
        self.install = match target.filter(|t| replaceable(&*installer, t)) {
            Some(target) => Install::Possible { target, installer },
            None => Install::Degraded,
        };
        self
    }

    /// Use these preferences, and call `save` whenever the Updater changes them.
    pub fn with_prefs(mut self, prefs: UpdatePrefs, save: SavePrefs) -> Updater {
        self.prefs = prefs;
        self.save_prefs = save;
        self
    }

    pub fn prefs(&self) -> &UpdatePrefs {
        &self.prefs
    }

    pub fn set_check_at_launch(&mut self, on: bool) {
        self.prefs.check_at_launch = on;
        (self.save_prefs)(&self.prefs);
    }

    /// "Skip this version": hide the banner and remember the version in the preferences.
    pub fn skip_this_version(&mut self) {
        self.state();
        if let State::Available { version, .. } | State::Updatable(Update { version, .. }) = &self.state {
            self.prefs.skipped_version = Some(version.clone());
            self.state = State::Idle;
            (self.save_prefs)(&self.prefs);
        }
    }

    /// Replace where DMG bytes come from and where they are stored (tests only; production uses
    /// the GitHub download and a temp folder).
    #[cfg(test)]
    pub fn with_downloads(mut self, dmgs: Arc<dyn DmgSource>, dir: PathBuf) -> Updater {
        self.dmgs = dmgs;
        self.downloads = dir;
        self
    }

    /// The real thing: GitHub feed, system browser, the platform's own installer.
    pub fn production(repaint: impl Fn() + Send + Sync + 'static) -> Updater {
        let prefs = crate::prefs::Prefs::load().updates;
        let save: SavePrefs = Arc::new(|u: &UpdatePrefs| {
            let mut p = crate::prefs::Prefs::load();
            p.updates = u.clone();
            p.save();
        });
        let platform = Platform::current();
        let mut u = Updater::new(
            env!("CARGO_PKG_VERSION"),
            Arc::new(GithubFeed),
            Arc::new(SystemBrowser),
            platform.is_some(),
        )
        .with_platform(platform.unwrap_or(Platform::MacosArm64));
        u.on_change = Arc::new(repaint);
        let mut u = u.with_prefs(prefs, save);
        u = match platform {
            Some(Platform::MacosArm64) => u.with_install(running_bundle(), Arc::new(SystemInstaller)),
            Some(Platform::WindowsX64) => u.with_package_install(crate::updater_pkg::windows_installer()),
            Some(Platform::LinuxX64) => u.with_package_install(crate::updater_pkg::appimage_installer()),
            None => u,
        };
        u.cleanup_old_bundle();
        u
    }

    /// Start the one check of this launch on a plain thread; never blocks the caller.
    pub fn check_at_launch(&mut self) {
        if !self.enabled || !self.prefs.check_at_launch || self.checked {
            return;
        }
        let (tx, rx) = channel();
        let (feed, current, notify) = (self.feed.clone(), self.current.clone(), self.on_change.clone());
        let skipped = self.prefs.skipped_version.clone();
        let degraded = matches!(self.install, Install::Degraded);
        let platform = self.platform;
        std::thread::spawn(move || {
            let found = feed
                .latest()
                .and_then(|json| announce(&current, &json, platform))
                .map(|s| if degraded { announce_only(s) } else { s })
                .filter(|s| !is_skipped(s, skipped.as_deref()));
            if let Some(found) = found {
                let _ = tx.send(found);
                notify();
            }
        });
        self.checked = true;
        self.pending = Some(rx);
    }

    /// Current state; picks up a finished check.
    pub fn state(&mut self) -> &State {
        if let Some(rx) = &self.pending {
            if let Ok(found) = rx.try_recv() {
                self.state = found;
            }
        }
        if let Some(rx) = &self.job {
            while let Ok(next) = rx.try_recv() {
                self.state = next;
            }
        }
        &self.state
    }

    /// Block until the running check has finished (tests only).
    #[cfg(test)]
    fn wait_for_background_work(&mut self) {
        if let Some(rx) = self.pending.take() {
            if let Ok(found) = rx.recv() {
                self.state = found;
            }
        }
        if let Some(rx) = self.job.take() {
            while let Ok(next) = rx.recv() {
                self.state = next;
            }
        }
    }

    /// "Later": hide the banner for this launch (the check is not repeated).
    pub fn later(&mut self) {
        self.state();
        if self.state.is_dismissible() {
            self.state = State::Idle;
        }
    }

    /// "Open release page": show the Release in the browser; the banner stays.
    pub fn open_release_page(&mut self) {
        match self.state().clone() {
            State::Available { url, .. } => self.browser.open(&url),
            State::Updatable(u) | State::Failed { update: u, .. } => self.browser.open(&u.url),
            _ => {}
        }
    }

    /// "Update now" / "Try again": download the DMG on a background thread and verify it against
    /// GitHub's digest. Ignores Lock and Safe mode; there is no cancel.
    pub fn update_now(&mut self) {
        let (State::Updatable(update) | State::Failed { update, .. }) = self.state().clone() else { return };
        let (tx, rx) = channel();
        let (dmgs, dir, notify) = (self.dmgs.clone(), self.downloads.clone(), self.on_change.clone());
        let job = update.clone();
        let platform = self.platform;
        let install = match &self.install {
            Install::Possible { target, installer } => Some(Step::Bundle(target.clone(), installer.clone())),
            Install::Package(p) => Some(Step::Package(p.clone())),
            _ => None,
        };
        std::thread::spawn(move || {
            let send = |s: State| {
                let _ = tx.send(s);
                notify();
            };
            match fetch_verified(&*dmgs, &dir, &job, platform, &|| send(State::Verifying(job.clone()))) {
                Ok(dmg) => match &install {
                    None => send(State::Ready { update: job.clone(), dmg }),
                    Some(step) => {
                        send(State::Installing(job.clone()));
                        let done = match step {
                            Step::Bundle(target, installer) => install_bundle(&**installer, &dmg, target),
                            Step::Package(p) => p.install(&dmg),
                        };
                        match done {
                            Ok(()) => send(State::Relaunching(job.clone())),
                            Err(error) => send(State::Failed { update: job.clone(), error }),
                        }
                    }
                },
                Err(error) => send(State::Failed { update: job.clone(), error }),
            }
        });
        self.state = State::Downloading(update);
        self.job = Some(rx);
    }

    /// Delete the `.old` bundle a previous Update left next to the installed app.
    pub fn cleanup_old_bundle(&self) {
        match &self.install {
            Install::Possible { target, installer } => {
                let old = sibling(target, ".old");
                if old.exists() {
                    let _ = installer.remove(&old);
                }
            }
            Install::Package(p) => p.cleanup(),
            _ => {}
        }
    }
}

/// What the download thread does with a verified file.
enum Step {
    Bundle(PathBuf, Arc<dyn Installer>),
    Package(Arc<dyn PackageInstaller>),
}

pub(crate) fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    target.with_file_name(name)
}

/// True when the bundle can be swapped in place: an `.app`, not App-Translocated, on a writable
/// volume, in a folder we can write to.
fn replaceable(installer: &dyn Installer, target: &Path) -> bool {
    let Some(parent) = target.parent() else { return false };
    target.extension().is_some_and(|e| e == "app")
        && !target.components().any(|c| c.as_os_str() == "AppTranslocation")
        && !installer.volume_read_only(parent)
        && tempfile::Builder::new().prefix(".mm-write-probe").tempfile_in(parent).is_ok()
}

/// A Release that cannot be installed is announced only.
fn announce_only(s: State) -> State {
    match s {
        State::Updatable(u) => State::Available { version: u.version, notes: u.notes, url: u.url },
        other => other,
    }
}

/// Replace `target` with the app inside the DMG. On any failure the original bundle is back in
/// place (it is never deleted before the new one is), and the error says what went wrong.
fn install_bundle(installer: &dyn Installer, dmg: &Path, target: &Path) -> Result<(), String> {
    let name = target.file_name().ok_or("no app name")?;
    let (fresh, old) = (sibling(target, ".new"), sibling(target, ".old"));
    let mount = installer.mount(dmg).map_err(|e| format!("Cannot open the downloaded disk image: {e}"))?;
    let copied = remove_if_present(installer, &fresh).and_then(|()| installer.copy(&mount.join(name), &fresh));
    installer.unmount(&mount);
    if let Err(e) = copied {
        let _ = remove_if_present(installer, &fresh);
        return Err(format!("Cannot copy the new version: {e}"));
    }
    // A `.old` still here belongs to an earlier Update; the installed bundle is what matters now.
    let aside = remove_if_present(installer, &old).and_then(|()| installer.rename(target, &old));
    if let Err(e) = aside {
        let _ = remove_if_present(installer, &fresh);
        return Err(format!("Cannot move the installed app aside: {e}"));
    }
    let placed = installer.rename(&fresh, target);
    let finished = placed.clone().and_then(|()| {
        installer.strip_quarantine(target).map_err(|e| format!("Cannot clear quarantine: {e}"))?;
        installer.relaunch(target).map_err(|e| format!("Cannot start the new version: {e}"))
    });
    let Err(e) = finished else { return Ok(()) };
    // Roll back: drop whatever new bundle is in place, then put the old one back.
    let restored = remove_if_present(installer, if placed.is_ok() { target } else { &fresh })
        .and_then(|()| installer.rename(&old, target));
    match restored {
        Ok(()) => Err(format!("{e}. The installed version was left unchanged.")),
        Err(r) => Err(format!("{e}. Restoring the old version also failed: {r}. It is at {}", old.display())),
    }
}

fn remove_if_present(installer: &dyn Installer, path: &Path) -> Result<(), String> {
    if path.exists() {
        installer.remove(path)
    } else {
        Ok(())
    }
}

/// Download, check the digest, and leave only a verified file in `dir`.
fn fetch_verified(
    dmgs: &dyn DmgSource,
    dir: &std::path::Path,
    u: &Update,
    platform: Platform,
    verifying: &dyn Fn(),
) -> Result<PathBuf, String> {
    let bytes = dmgs.download(&u.asset_url)?;
    verifying();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot prepare download folder: {e}"))?;
    let name = platform.asset_name(&u.version);
    let (part, dmg) = (dir.join(format!("{name}.part")), dir.join(&name));
    std::fs::write(&part, &bytes).map_err(|e| format!("cannot save download: {e}"))?;
    let digest = Sha256::digest(std::fs::read(&part).map_err(|e| e.to_string())?);
    if format!("{digest:x}") != u.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("Download is corrupt (checksum mismatch)".into());
    }
    std::fs::rename(&part, &dmg).map_err(|e| format!("cannot save download: {e}"))?;
    Ok(dmg)
}

/// The Available state for a feed body, if it holds a strictly newer, well-formed Release.
fn announce(current: &str, json: &str, platform: Platform) -> Option<State> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    if parse_version(&version)? <= parse_version(current)? {
        return None;
    }
    let notes = v["body"].as_str().unwrap_or_default().to_string();
    let url = v["html_url"].as_str()?.to_string();
    // Contract rule 1: the tag is exactly `v<ver>`; anything else is announce-only.
    let asset = (tag == format!("v{version}")).then(|| contract_asset(&version, &v, platform)).flatten();
    Some(match asset {
        Some((asset_url, sha256)) => State::Updatable(Update { version, notes, url, asset_url, sha256 }),
        None => State::Available { version, notes, url },
    })
}

/// The single asset for `platform` (download URL, lowercase hex digest) if the Release meets the contract.
fn contract_asset(version: &str, release: &serde_json::Value, platform: Platform) -> Option<(String, String)> {
    let want = platform.asset_name(version);
    let mut matching = release["assets"].as_array()?.iter().filter(|a| a["name"].as_str() == Some(want.as_str()));
    let asset = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    let hex = asset["digest"].as_str()?.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((asset["browser_download_url"].as_str()?.to_string(), hex.to_ascii_lowercase()))
}

/// True when the announced Release is not newer than the skipped version.
fn is_skipped(state: &State, skipped: Option<&str>) -> bool {
    let version = match state {
        State::Available { version, .. } => version,
        State::Updatable(u) => &u.version,
        _ => return false,
    };
    match skipped.and_then(parse_version) {
        Some(skip) => parse_version(version).is_some_and(|v| v <= skip),
        None => false,
    }
}

/// `major.minor.patch`, numbers only; anything else (e.g. a prerelease suffix) is unparseable.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

struct GithubFeed;
impl ReleaseFeed for GithubFeed {
    fn latest(&self) -> Option<String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .build()
            .into();
        agent
            .get(LATEST_URL)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "MacMischer")
            .call()
            .ok()?
            .body_mut()
            .read_to_string()
            .ok()
    }
}

struct HttpDmgs;
impl DmgSource for HttpDmgs {
    fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(300)))
            .build()
            .into();
        let mut resp = agent
            .get(url)
            .header("User-Agent", "MacMischer")
            .call()
            .map_err(|e| format!("Download failed: {e}"))?;
        resp.body_mut()
            .with_config()
            .limit(200 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| format!("Download failed: {e}"))
    }
}

/// The `.app` the running executable lives in, if any.
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = exe.ancestors().nth(3)?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

struct SystemInstaller;
impl SystemInstaller {
    fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
        let out = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("{cmd}: {}", String::from_utf8_lossy(&out.stderr).trim()))
        }
    }
}
impl Installer for SystemInstaller {
    fn mount(&self, dmg: &Path) -> Result<PathBuf, String> {
        let dir = std::env::temp_dir().join(format!("MacMischer-mount-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let args = ["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"];
        let mut a: Vec<&std::ffi::OsStr> = args.iter().map(|s| s.as_ref()).collect();
        a.extend([dir.as_os_str(), dmg.as_os_str()]);
        match Self::run("hdiutil", &a) {
            Ok(()) => Ok(dir),
            Err(e) => {
                let _ = std::fs::remove_dir(&dir);
                Err(e)
            }
        }
    }
    fn unmount(&self, mount: &Path) {
        let _ = Self::run("hdiutil", &["detach".as_ref(), "-force".as_ref(), mount.as_os_str()]);
        let _ = std::fs::remove_dir(mount);
    }
    fn copy(&self, from: &Path, to: &Path) -> Result<(), String> {
        Self::run("ditto", &[from.as_os_str(), to.as_os_str()])
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| format!("cannot move {}: {e}", from.display()))
    }
    fn remove(&self, path: &Path) -> Result<(), String> {
        std::fs::remove_dir_all(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))
    }
    fn strip_quarantine(&self, path: &Path) -> Result<(), String> {
        Self::run("xattr", &["-dr".as_ref(), "com.apple.quarantine".as_ref(), path.as_os_str()])
    }
    fn relaunch(&self, path: &Path) -> Result<(), String> {
        Self::run("open", &["-n".as_ref(), path.as_os_str()])
    }
    fn volume_read_only(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return true };
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            unsafe { libc::statvfs(c.as_ptr(), &mut st) != 0 || st.f_flag & libc::ST_RDONLY != 0 }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            true
        }
    }
}

struct SystemBrowser;
impl Browser for SystemBrowser {
    fn open(&self, url: &str) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(url).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FakeFeed(Option<String>);
    impl ReleaseFeed for FakeFeed {
        fn latest(&self) -> Option<String> {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct FakeBrowser(Mutex<Vec<String>>);
    impl Browser for FakeBrowser {
        fn open(&self, url: &str) {
            self.0.lock().unwrap().push(url.to_string());
        }
    }

    fn release(tag: &str) -> String {
        format!(r#"{{"tag_name":"{tag}","body":"Fixes","html_url":"https://example.test/r/{tag}"}}"#)
    }

    fn launched(current: &str, feed: Option<String>) -> Updater {
        let mut u = Updater::new(current, Arc::new(FakeFeed(feed)), Arc::new(FakeBrowser::default()), true);
        u.check_at_launch();
        u.wait_for_background_work();
        u
    }

    #[test]
    fn strictly_newer_release_is_announced_with_version_and_notes() {
        let mut u = launched("0.1.1", Some(release("v0.2.0")));
        assert_eq!(
            u.state(),
            &State::Available {
                version: "0.2.0".into(),
                notes: "Fixes".into(),
                url: "https://example.test/r/v0.2.0".into()
            }
        );
    }

    #[test]
    fn same_older_or_unparseable_version_shows_no_banner() {
        for tag in ["v0.1.1", "v0.1.0", "v0.0.9", "v0.1.2-rc1", "nightly", "v1.2", ""] {
            let mut u = launched("0.1.1", Some(release(tag)));
            assert_eq!(u.state(), &State::Idle, "tag {tag:?}");
        }
        let mut u = launched("not-a-version", Some(release("v9.9.9")));
        assert_eq!(u.state(), &State::Idle);
    }

    #[test]
    fn newer_minor_and_major_beat_larger_patch_numbers() {
        assert!(matches!(launched("0.9.9", Some(release("v0.10.0"))).state(), State::Available { .. }));
        assert!(matches!(launched("0.9.9", Some(release("v1.0.0"))).state(), State::Available { .. }));
    }

    #[test]
    fn offline_http_error_or_malformed_json_stays_silent() {
        for feed in [None, Some("<html>502</html>".to_string()), Some("{}".to_string()), Some(r#"{"tag_name":"v9.0.0"}"#.to_string())] {
            let mut u = launched("0.1.1", feed);
            assert_eq!(u.state(), &State::Idle);
        }
    }

    #[test]
    fn later_dismisses_the_banner_and_the_check_is_not_repeated() {
        let mut u = launched("0.1.1", Some(release("v0.2.0")));
        u.later();
        assert_eq!(u.state(), &State::Idle);
        u.check_at_launch();
        u.wait_for_background_work();
        assert_eq!(u.state(), &State::Idle);
    }

    #[test]
    fn open_release_page_opens_the_release_url_and_keeps_the_banner() {
        let browser = Arc::new(FakeBrowser::default());
        let mut u = Updater::new("0.1.1", Arc::new(FakeFeed(Some(release("v0.2.0")))), browser.clone(), true);
        u.check_at_launch();
        u.wait_for_background_work();
        u.open_release_page();
        assert_eq!(*browser.0.lock().unwrap(), vec!["https://example.test/r/v0.2.0".to_string()]);
        assert!(matches!(u.state(), State::Available { .. }));
    }

    #[test]
    fn open_release_page_without_a_banner_opens_nothing() {
        let browser = Arc::new(FakeBrowser::default());
        let mut u = Updater::new("0.1.1", Arc::new(FakeFeed(None)), browser.clone(), true);
        u.open_release_page();
        assert!(browser.0.lock().unwrap().is_empty());
    }

    #[test]
    fn unsupported_platform_never_checks_or_shows() {
        struct CountingFeed(Mutex<u32>);
        impl ReleaseFeed for CountingFeed {
            fn latest(&self) -> Option<String> {
                *self.0.lock().unwrap() += 1;
                Some(release("v9.0.0"))
            }
        }
        let feed = Arc::new(CountingFeed(Mutex::new(0)));
        let mut u = Updater::new("0.1.1", feed.clone(), Arc::new(FakeBrowser::default()), false);
        u.check_at_launch();
        u.wait_for_background_work();
        assert_eq!(u.state(), &State::Idle);
        assert_eq!(*feed.0.lock().unwrap(), 0);
    }

    #[test]
    fn check_does_not_block_the_caller() {
        struct SlowFeed(Mutex<std::sync::mpsc::Receiver<()>>);
        impl ReleaseFeed for SlowFeed {
            fn latest(&self) -> Option<String> {
                self.0.lock().unwrap().recv().ok();
                Some(release("v0.2.0"))
            }
        }
        let (go, wait) = std::sync::mpsc::channel();
        let mut u = Updater::new("0.1.1", Arc::new(SlowFeed(Mutex::new(wait))), Arc::new(FakeBrowser::default()), true);
        u.check_at_launch(); // returns although the feed is still waiting
        assert_eq!(u.state(), &State::Idle);
        go.send(()).unwrap();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Available { .. }));
    }

    // ---- Update now: download + verify ----

    const DMG: &[u8] = b"fake dmg bytes";
    const DMG_SHA: &str = "39eedc0e383479401621887eea4090532d00e64d6c524a19a9aaab3414ac771b";
    const DMG_URL: &str = "https://example.test/dl/MacMischer-0.2.0-macos-arm64.dmg";

    struct FakeDmgs {
        body: Mutex<Result<Vec<u8>, String>>,
        hits: Mutex<u32>,
    }
    impl FakeDmgs {
        fn new(body: Result<Vec<u8>, String>) -> Arc<FakeDmgs> {
            Arc::new(FakeDmgs { body: Mutex::new(body), hits: Mutex::new(0) })
        }
    }
    impl DmgSource for FakeDmgs {
        fn download(&self, url: &str) -> Result<Vec<u8>, String> {
            assert_eq!(url, DMG_URL);
            *self.hits.lock().unwrap() += 1;
            self.body.lock().unwrap().clone()
        }
    }

    fn asset(name: &str, digest: Option<&str>) -> String {
        let digest = digest.map_or("null".to_string(), |d| format!("\"{d}\""));
        format!(r#"{{"name":"{name}","digest":{digest},"browser_download_url":"https://example.test/dl/{name}"}}"#)
    }

    fn release_with(tag: &str, assets: &[String]) -> String {
        format!(
            r#"{{"tag_name":"{tag}","body":"Fixes","html_url":"https://example.test/r/{tag}","assets":[{}]}}"#,
            assets.join(",")
        )
    }

    fn good_asset() -> String {
        asset("MacMischer-0.2.0-macos-arm64.dmg", Some(&format!("sha256:{DMG_SHA}")))
    }

    fn good_release() -> String {
        release_with("v0.2.0", &[good_asset()])
    }

    fn with_dmgs(feed: String, dmgs: Arc<FakeDmgs>, dir: &std::path::Path) -> Updater {
        let mut u = Updater::new("0.1.1", Arc::new(FakeFeed(Some(feed))), Arc::new(FakeBrowser::default()), true)
            .with_downloads(dmgs, dir.to_path_buf());
        u.check_at_launch();
        u.wait_for_background_work();
        u
    }

    #[test]
    fn contract_conforming_release_offers_update_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Ok(DMG.to_vec())), dir.path());
        let State::Updatable(up) = u.state().clone() else { panic!("{:?}", u.state()) };
        assert_eq!((up.version.as_str(), up.notes.as_str()), ("0.2.0", "Fixes"));
        assert_eq!(up.url, "https://example.test/r/v0.2.0");
    }

    #[test]
    fn matching_digest_leaves_a_verified_dmg_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Ok(DMG.to_vec())), dir.path());
        u.update_now();
        assert!(matches!(u.state(), State::Downloading(_) | State::Verifying(_) | State::Ready { .. }));
        u.wait_for_background_work();
        let State::Ready { update, dmg } = u.state().clone() else { panic!("{:?}", u.state()) };
        assert_eq!(update.version, "0.2.0");
        assert_eq!(std::fs::read(&dmg).unwrap(), DMG);
        assert!(dmg.starts_with(dir.path()));
    }

    #[test]
    fn download_runs_in_the_background_and_shows_downloading() {
        struct Gated(Mutex<std::sync::mpsc::Receiver<()>>);
        impl DmgSource for Gated {
            fn download(&self, _: &str) -> Result<Vec<u8>, String> {
                self.0.lock().unwrap().recv().ok();
                Ok(DMG.to_vec())
            }
        }
        let (go, wait) = std::sync::mpsc::channel();
        let dir = tempfile::tempdir().unwrap();
        let mut u = Updater::new("0.1.1", Arc::new(FakeFeed(Some(good_release()))), Arc::new(FakeBrowser::default()), true)
            .with_downloads(Arc::new(Gated(Mutex::new(wait))), dir.path().to_path_buf());
        u.check_at_launch();
        u.wait_for_background_work();
        u.update_now(); // returns although the download is still waiting
        assert!(matches!(u.state(), State::Downloading(_)));
        go.send(()).unwrap();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Ready { .. }));
    }

    #[test]
    fn digest_mismatch_deletes_the_file_and_shows_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Ok(b"tampered".to_vec())), dir.path());
        u.update_now();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Failed { .. }));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn network_failure_shows_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("offline".into())), dir.path());
        u.update_now();
        u.wait_for_background_work();
        let State::Failed { error, .. } = u.state().clone() else { panic!("{:?}", u.state()) };
        assert!(error.contains("offline"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn try_again_restarts_the_download_from_the_error_state() {
        let dir = tempfile::tempdir().unwrap();
        let dmgs = FakeDmgs::new(Err("offline".into()));
        let mut u = with_dmgs(good_release(), dmgs.clone(), dir.path());
        u.update_now();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Failed { .. }));
        *dmgs.body.lock().unwrap() = Ok(DMG.to_vec());
        u.update_now(); // Try again
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Ready { .. }));
        assert_eq!(*dmgs.hits.lock().unwrap(), 2);
    }

    #[test]
    fn update_now_is_ignored_unless_an_update_is_offered_or_failed() {
        let dir = tempfile::tempdir().unwrap();
        let dmgs = FakeDmgs::new(Ok(DMG.to_vec()));
        let mut u = with_dmgs(good_release(), dmgs.clone(), dir.path());
        u.update_now();
        u.wait_for_background_work();
        u.update_now(); // already Ready
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Ready { .. }));
        assert_eq!(*dmgs.hits.lock().unwrap(), 1);
        let mut idle = launched("0.1.1", None);
        idle.update_now();
        assert_eq!(idle.state(), &State::Idle);
    }

    #[test]
    fn releases_breaking_the_contract_are_announce_only() {
        let name = "MacMischer-0.2.0-macos-arm64.dmg";
        let cases: Vec<(&str, String)> = vec![
            ("no assets", release_with("v0.2.0", &[])),
            ("wrong name", release_with("v0.2.0", &[asset("MacMischer.dmg", Some(&format!("sha256:{DMG_SHA}")))])),
            ("asset version differs from tag", release_with("v0.2.0", &[asset("MacMischer-0.3.0-macos-arm64.dmg", Some(&format!("sha256:{DMG_SHA}")))])),
            ("missing digest", release_with("v0.2.0", &[asset(name, None)])),
            ("digest not sha256", release_with("v0.2.0", &[asset(name, Some("md5:abcd"))])),
            ("digest not hex", release_with("v0.2.0", &[asset(name, Some(&format!("sha256:{}", "z".repeat(64))))])),
            ("digest wrong length", release_with("v0.2.0", &[asset(name, Some("sha256:abcd"))])),
            ("two matching assets", release_with("v0.2.0", &[good_asset(), good_asset()])),
            ("no assets field", release("v0.2.0")),
            ("tag without v prefix", release_with("0.2.0", &[good_asset()])),
            ("tag with doubled v prefix", release_with("vv0.2.0", &[good_asset()])),
            ("tag newer than the asset's version", release_with("v0.3.0", &[good_asset()])),
        ];
        for (why, feed) in cases {
            let dir = tempfile::tempdir().unwrap();
            let dmgs = FakeDmgs::new(Ok(DMG.to_vec()));
            let mut u = with_dmgs(feed, dmgs.clone(), dir.path());
            assert!(matches!(u.state(), State::Available { .. }), "{why}: {:?}", u.state());
            u.update_now();
            u.wait_for_background_work();
            assert!(matches!(u.state(), State::Available { .. }), "{why}");
            assert_eq!(*dmgs.hits.lock().unwrap(), 0, "{why}");
        }
    }

    #[test]
    fn state_predicates_and_banner_view() {
        let up = Update { version: "0.2.0".into(), notes: "n".into(), url: "u".into(), asset_url: "a".into(), sha256: "s".into() };
        assert!(State::Idle.banner().is_none());
        let offered = State::Updatable(up.clone());
        assert_eq!(offered.banner().unwrap().text, "is available");
        assert!(offered.is_dismissible() && offered.is_skippable() && !offered.should_quit());
        let failed = State::Failed { update: up.clone(), error: "boom".into() };
        assert_eq!(failed.banner().unwrap().error.as_deref(), Some("boom"));
        assert!(failed.is_dismissible() && !failed.is_skippable());
        assert!(!State::Downloading(up.clone()).is_dismissible());
        assert!(State::Relaunching(up).should_quit());
    }

    #[test]
    fn later_dismisses_an_offered_or_failed_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("x".into())), dir.path());
        u.later();
        assert_eq!(u.state(), &State::Idle);
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("x".into())), dir.path());
        u.update_now();
        u.wait_for_background_work();
        u.later();
        assert_eq!(u.state(), &State::Idle);
    }

    #[derive(Default)]
    struct Saved(Mutex<Vec<UpdatePrefs>>);

    fn with_prefs(prefs: UpdatePrefs, feed: Arc<dyn ReleaseFeed>) -> (Updater, Arc<Saved>) {
        let saved = Arc::new(Saved::default());
        let s = saved.clone();
        let u = Updater::new("0.1.1", feed, Arc::new(FakeBrowser::default()), true)
            .with_prefs(prefs, Arc::new(move |p: &UpdatePrefs| s.0.lock().unwrap().push(p.clone())));
        (u, saved)
    }

    fn launch(u: &mut Updater) {
        u.check_at_launch();
        u.wait_for_background_work();
    }

    #[test]
    fn update_prefs_default_to_checking_with_nothing_skipped() {
        assert_eq!(UpdatePrefs::default(), UpdatePrefs { check_at_launch: true, skipped_version: None });
    }

    #[test]
    fn check_at_launch_off_makes_no_network_call_and_shows_no_banner() {
        struct CountingFeed(Mutex<u32>);
        impl ReleaseFeed for CountingFeed {
            fn latest(&self) -> Option<String> {
                *self.0.lock().unwrap() += 1;
                Some(release("v9.0.0"))
            }
        }
        let feed = Arc::new(CountingFeed(Mutex::new(0)));
        let prefs = UpdatePrefs { check_at_launch: false, ..Default::default() };
        let (mut u, _) = with_prefs(prefs, feed.clone());
        launch(&mut u);
        assert_eq!(u.state(), &State::Idle);
        assert_eq!(*feed.0.lock().unwrap(), 0);
    }

    #[test]
    fn turning_the_toggle_off_is_written_to_prefs() {
        let (mut u, saved) = with_prefs(UpdatePrefs::default(), Arc::new(FakeFeed(None)));
        u.set_check_at_launch(false);
        let last = saved.0.lock().unwrap().last().cloned().unwrap();
        assert!(!last.check_at_launch);
    }

    #[test]
    fn skip_this_version_hides_the_banner_and_is_remembered_across_launches() {
        let feed = Arc::new(FakeFeed(Some(release("v0.2.0"))));
        let (mut u, saved) = with_prefs(UpdatePrefs::default(), feed.clone());
        launch(&mut u);
        u.skip_this_version();
        assert_eq!(u.state(), &State::Idle);
        let written = saved.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(written.skipped_version.as_deref(), Some("0.2.0"));
        // Restart with the written preferences.
        let (mut u2, _) = with_prefs(written, feed);
        launch(&mut u2);
        assert_eq!(u2.state(), &State::Idle);
    }

    #[test]
    fn a_release_newer_than_the_skipped_one_is_offered_again() {
        let prefs = UpdatePrefs { skipped_version: Some("0.2.0".into()), ..Default::default() };
        let (mut u, _) = with_prefs(prefs.clone(), Arc::new(FakeFeed(Some(release("v0.2.1")))));
        launch(&mut u);
        assert!(matches!(u.state(), State::Available { version, .. } if version == "0.2.1"));
        let (mut older, _) = with_prefs(prefs, Arc::new(FakeFeed(Some(release("v0.1.5")))));
        launch(&mut older);
        assert_eq!(older.state(), &State::Idle);
    }

    #[test]
    fn skip_without_a_banner_writes_nothing() {
        let (mut u, saved) = with_prefs(UpdatePrefs::default(), Arc::new(FakeFeed(None)));
        launch(&mut u);
        u.skip_this_version();
        assert!(saved.0.lock().unwrap().is_empty());
    }

    // ---- Update now: install in place and relaunch ----

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn version_of(bundle: &std::path::Path) -> Option<String> {
        std::fs::read_to_string(bundle.join("Contents/version.txt")).ok()
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dest = to.join(e.file_name());
            if e.path().is_dir() {
                copy_tree(&e.path(), &dest);
            } else {
                std::fs::copy(e.path(), dest).unwrap();
            }
        }
    }

    /// Stands in for hdiutil/ditto/xattr/open. Moves and removals are real, on a temp directory;
    /// `fail` names the one step that must fail: mount, copy, aside, place, strip or relaunch.
    struct FakeInstaller {
        volume: PathBuf,
        target: PathBuf,
        fail: Mutex<Option<&'static str>>,
        read_only_volume: bool,
        log: Mutex<Vec<String>>,
    }
    impl FakeInstaller {
        fn step(&self, name: &'static str, detail: String) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("{name} {detail}"));
            if *self.fail.lock().unwrap() == Some(name) {
                Err(format!("forced {name} failure"))
            } else {
                Ok(())
            }
        }
        fn did(&self, name: &str) -> Vec<String> {
            let prefix = format!("{name} ");
            self.log.lock().unwrap().iter().filter_map(|l| l.strip_prefix(&prefix).map(String::from)).collect()
        }
    }
    impl Installer for FakeInstaller {
        fn mount(&self, dmg: &std::path::Path) -> Result<PathBuf, String> {
            assert!(dmg.is_file(), "the verified DMG is what gets mounted");
            self.step("mount", dmg.display().to_string())?;
            Ok(self.volume.clone())
        }
        fn unmount(&self, _: &std::path::Path) {
            let _ = self.step("unmount", String::new());
        }
        fn copy(&self, from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
            copy_tree(from, to);
            self.step("copy", to.display().to_string())
        }
        fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
            let name = if from == self.target {
                "aside"
            } else if to == self.target && from.extension().is_some_and(|e| e == "new") {
                "place"
            } else {
                "restore"
            };
            self.step(name, to.display().to_string())?;
            std::fs::rename(from, to).map_err(|e| e.to_string())
        }
        fn remove(&self, path: &std::path::Path) -> Result<(), String> {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
        fn strip_quarantine(&self, path: &std::path::Path) -> Result<(), String> {
            self.step("strip", path.display().to_string())
        }
        fn relaunch(&self, path: &std::path::Path) -> Result<(), String> {
            self.step("relaunch", path.display().to_string())
        }
        fn volume_read_only(&self, _: &std::path::Path) -> bool {
            self.read_only_volume
        }
    }

    struct Rig {
        _tmp: tempfile::TempDir,
        apps: PathBuf,
        target: PathBuf,
        installer: Arc<FakeInstaller>,
        updater: Updater,
    }

    fn rig_with(fail: Option<&'static str>, read_only_volume: bool, target_override: Option<Option<PathBuf>>) -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let apps = tmp.path().join("Applications");
        let target = apps.join("MacMischer.app");
        write(&target.join("Contents/version.txt"), "0.1.1");
        let volume = tmp.path().join("volume");
        write(&volume.join("MacMischer.app/Contents/version.txt"), "0.2.0");
        let installer =
            Arc::new(FakeInstaller { volume, target: target.clone(), fail: Mutex::new(fail), read_only_volume, log: Mutex::default() });
        let mut updater = Updater::new("0.1.1", Arc::new(FakeFeed(Some(good_release()))), Arc::new(FakeBrowser::default()), true)
            .with_downloads(FakeDmgs::new(Ok(DMG.to_vec())), tmp.path().join("dl"))
            .with_install(target_override.unwrap_or(Some(target.clone())), installer.clone());
        updater.check_at_launch();
        updater.wait_for_background_work();
        Rig { _tmp: tmp, apps, target, installer, updater }
    }

    fn rig(fail: Option<&'static str>) -> Rig {
        rig_with(fail, false, None)
    }

    #[test]
    fn update_now_installs_the_new_bundle_strips_quarantine_and_relaunches() {
        let mut r = rig(None);
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert!(matches!(r.updater.state(), State::Relaunching(u) if u.version == "0.2.0"));
        assert_eq!(version_of(&r.target).as_deref(), Some("0.2.0"));
        assert_eq!(version_of(&r.apps.join("MacMischer.app.old")).as_deref(), Some("0.1.1"));
        assert_eq!(r.installer.did("strip"), vec![r.target.display().to_string()]);
        assert_eq!(r.installer.did("relaunch"), vec![r.target.display().to_string()]);
        assert_eq!(r.installer.did("unmount").len(), 1);
        assert!(!r.apps.join("MacMischer.app.new").exists(), "no temporary copy is left behind");
    }

    /// The original bundle is back, intact, and no helper copies are left in the folder.
    fn assert_original_restored(r: &Rig) {
        assert_eq!(version_of(&r.target).as_deref(), Some("0.1.1"));
        let mut names: Vec<_> =
            std::fs::read_dir(&r.apps).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["MacMischer.app"]);
    }

    fn assert_failed_with(r: &mut Rig, step: &str) {
        let State::Failed { update, error } = r.updater.state().clone() else { panic!("{:?}", r.updater.state()) };
        assert_eq!(update.version, "0.2.0");
        assert!(error.contains(&format!("forced {step} failure")), "{error}");
        assert!(r.installer.did("relaunch").is_empty() || step == "relaunch");
    }

    #[test]
    fn a_failure_at_each_step_restores_the_original_bundle_and_reports_an_error() {
        for step in ["mount", "copy", "aside", "place", "strip", "relaunch"] {
            let mut r = rig(Some(step));
            r.updater.update_now();
            r.updater.wait_for_background_work();
            assert_failed_with(&mut r, step);
            assert_original_restored(&r);
        }
    }

    #[test]
    fn the_mount_is_released_even_when_copying_fails() {
        let mut r = rig(Some("copy"));
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert_eq!(r.installer.did("unmount").len(), 1);
    }

    #[test]
    fn the_old_bundle_is_only_moved_aside_after_the_new_one_is_fully_copied() {
        let mut r = rig(Some("copy"));
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert!(r.installer.did("aside").is_empty());
        assert_eq!(version_of(&r.target).as_deref(), Some("0.1.1"));
    }

    #[test]
    fn try_again_after_a_failed_install_can_succeed() {
        let mut r = rig(Some("strip"));
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert!(matches!(r.updater.state(), State::Failed { .. }));
        *r.installer.fail.lock().unwrap() = None;
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert!(matches!(r.updater.state(), State::Relaunching(_)));
        assert_eq!(version_of(&r.target).as_deref(), Some("0.2.0"));
    }

    #[test]
    fn a_leftover_old_bundle_is_removed_on_the_next_launch() {
        let r = rig(None);
        let old = r.apps.join("MacMischer.app.old");
        write(&old.join("Contents/version.txt"), "0.0.9");
        r.updater.cleanup_old_bundle();
        assert!(!old.exists());
        assert_eq!(version_of(&r.target).as_deref(), Some("0.1.1"), "the installed bundle is untouched");
    }

    #[test]
    fn cleanup_with_nothing_left_over_does_nothing() {
        let r = rig(None);
        r.updater.cleanup_old_bundle();
        assert_eq!(version_of(&r.target).as_deref(), Some("0.1.1"));
    }

    #[test]
    fn a_stale_old_bundle_does_not_block_the_next_update() {
        let mut r = rig(None);
        write(&r.apps.join("MacMischer.app.old/Contents/version.txt"), "0.0.9");
        r.updater.update_now();
        r.updater.wait_for_background_work();
        assert!(matches!(r.updater.state(), State::Relaunching(_)));
        assert_eq!(version_of(&r.apps.join("MacMischer.app.old")).as_deref(), Some("0.1.1"));
    }

    // ---- degraded: Open release page instead of Update now ----

    fn assert_announce_only(r: &mut Rig) {
        assert!(matches!(r.updater.state(), State::Available { version, .. } if version == "0.2.0"), "{:?}", r.updater.state());
        r.updater.update_now(); // there is no Update now to press
        r.updater.wait_for_background_work();
        assert!(matches!(r.updater.state(), State::Available { .. }));
        assert!(r.installer.did("mount").is_empty());
    }

    #[test]
    fn not_running_from_an_app_bundle_is_announce_only() {
        let mut r = rig_with(None, false, Some(None));
        assert_announce_only(&mut r);
    }

    #[test]
    fn a_path_that_is_not_an_app_bundle_is_announce_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = rig_with(None, false, Some(Some(tmp.path().join("target/debug/macmischer"))));
        assert_announce_only(&mut r);
    }

    #[test]
    fn a_read_only_volume_is_announce_only() {
        let mut r = rig_with(None, true, None);
        assert_announce_only(&mut r);
    }

    #[test]
    fn an_app_translocation_path_is_announce_only() {
        let tmp = tempfile::tempdir().unwrap();
        let translocated = tmp.path().join("AppTranslocation/ABC-123/d/MacMischer.app");
        write(&translocated.join("Contents/version.txt"), "0.1.1");
        let mut r = rig_with(None, false, Some(Some(translocated)));
        assert_announce_only(&mut r);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_we_cannot_write_to_is_announce_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        write(&locked.join("MacMischer.app/Contents/version.txt"), "0.1.1");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable_anyway = std::fs::File::create(locked.join("probe")).is_ok(); // running as root
        let mut r = rig_with(None, false, Some(Some(locked.join("MacMischer.app"))));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !writable_anyway {
            assert_announce_only(&mut r);
        }
    }

    // ---- Windows / Linux ----

    fn platform_release(platform: Platform) -> String {
        release_with(
            "v0.2.0",
            &[good_asset(), asset(&platform.asset_name("0.2.0"), Some(&format!("sha256:{DMG_SHA}")))],
        )
    }

    #[test]
    fn each_platform_picks_only_its_own_asset() {
        for p in [Platform::MacosArm64, Platform::WindowsX64, Platform::LinuxX64] {
            let json = release_with("v0.2.0", &[asset(&p.asset_name("0.2.0"), Some(&format!("sha256:{DMG_SHA}")))]);
            for q in [Platform::MacosArm64, Platform::WindowsX64, Platform::LinuxX64] {
                let state = announce("0.1.1", &json, q).unwrap();
                assert_eq!(matches!(state, State::Updatable(_)), p == q, "asset for {p:?} seen from {q:?}");
            }
        }
        assert_eq!(Platform::WindowsX64.asset_name("1.2.3"), "MacMischer-1.2.3-windows-x64-setup.exe");
        assert_eq!(Platform::LinuxX64.asset_name("1.2.3"), "MacMischer-1.2.3-linux-x86_64.AppImage");
    }

    struct AnyUrlDmgs(Vec<u8>);
    impl DmgSource for AnyUrlDmgs {
        fn download(&self, _: &str) -> Result<Vec<u8>, String> {
            Ok(self.0.clone())
        }
    }

    struct FakePackage(Mutex<Vec<PathBuf>>, Result<(), String>);
    impl PackageInstaller for FakePackage {
        fn install(&self, package: &Path) -> Result<(), String> {
            self.0.lock().unwrap().push(package.to_path_buf());
            self.1.clone()
        }
    }

    fn package_updater(platform: Platform, pkg: Arc<FakePackage>, dir: &Path) -> Updater {
        Updater::new("0.1.1", Arc::new(FakeFeed(Some(platform_release(platform)))), Arc::new(FakeBrowser::default()), true)
            .with_platform(platform)
            .with_package_install(Some(pkg))
            .with_downloads(Arc::new(AnyUrlDmgs(DMG.to_vec())), dir.to_path_buf())
    }

    #[test]
    fn package_update_downloads_verifies_installs_and_asks_the_app_to_quit() {
        for p in [Platform::WindowsX64, Platform::LinuxX64] {
            let dir = tempfile::tempdir().unwrap();
            let pkg = Arc::new(FakePackage(Mutex::new(vec![]), Ok(())));
            let mut u = package_updater(p, pkg.clone(), dir.path());
            u.check_at_launch();
            u.wait_for_background_work();
            u.update_now();
            u.wait_for_background_work();
            assert!(matches!(u.state(), State::Relaunching(_)), "{p:?}: {:?}", u.state());
            assert_eq!(*pkg.0.lock().unwrap(), vec![dir.path().join(p.asset_name("0.2.0"))]);
        }
    }

    #[test]
    fn package_install_failure_is_reported_with_try_again() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = Arc::new(FakePackage(Mutex::new(vec![]), Err("nope".into())));
        let mut u = package_updater(Platform::LinuxX64, pkg, dir.path());
        u.check_at_launch();
        u.wait_for_background_work();
        u.update_now();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Failed { error, .. } if error == "nope"));
    }

    #[test]
    fn package_update_is_announce_only_when_it_cannot_be_installed_in_place() {
        let mut u = Updater::new(
            "0.1.1",
            Arc::new(FakeFeed(Some(platform_release(Platform::LinuxX64)))),
            Arc::new(FakeBrowser::default()),
            true,
        )
        .with_platform(Platform::LinuxX64)
        .with_package_install(None);
        u.check_at_launch();
        u.wait_for_background_work();
        assert!(matches!(u.state(), State::Available { .. }));
    }
}

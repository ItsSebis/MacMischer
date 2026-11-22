//! The Updater: checks GitHub for a newer Release at launch and reports its state for the banner.
//! An Update is not an Action; this module is independent of the Console and the mixer.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
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
    /// Download or verification failed; Try again is `update_now()`.
    Failed { update: Update, error: String },
}

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
        }
    }

    /// Replace where DMG bytes come from and where they are stored.
    pub fn with_downloads(mut self, dmgs: Arc<dyn DmgSource>, dir: PathBuf) -> Updater {
        self.dmgs = dmgs;
        self.downloads = dir;
        self
    }

    /// The real thing: GitHub feed, system browser, macOS arm64 only.
    pub fn production(repaint: impl Fn() + Send + Sync + 'static) -> Updater {
        let mut u = Updater::new(
            env!("CARGO_PKG_VERSION"),
            Arc::new(GithubFeed),
            Arc::new(SystemBrowser),
            cfg!(all(target_os = "macos", target_arch = "aarch64")),
        );
        u.on_change = Arc::new(repaint);
        u
    }

    /// Start the one check of this launch on a plain thread; never blocks the caller.
    pub fn check_at_launch(&mut self) {
        if !self.enabled || self.checked {
            return;
        }
        let (tx, rx) = channel();
        let (feed, current, notify) = (self.feed.clone(), self.current.clone(), self.on_change.clone());
        std::thread::spawn(move || {
            if let Some(found) = feed.latest().and_then(|json| announce(&current, &json)) {
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
    fn settle(&mut self) {
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
        if matches!(self.state, State::Available { .. } | State::Updatable(_) | State::Failed { .. }) {
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
        std::thread::spawn(move || {
            let send = |s: State| {
                let _ = tx.send(s);
                notify();
            };
            match fetch_verified(&*dmgs, &dir, &job, &|| send(State::Verifying(job.clone()))) {
                Ok(dmg) => send(State::Ready { update: job.clone(), dmg }),
                Err(error) => send(State::Failed { update: job.clone(), error }),
            }
        });
        self.state = State::Downloading(update);
        self.job = Some(rx);
    }
}

/// Download, check the digest, and leave only a verified file in `dir`.
fn fetch_verified(
    dmgs: &dyn DmgSource,
    dir: &std::path::Path,
    u: &Update,
    verifying: &dyn Fn(),
) -> Result<PathBuf, String> {
    let bytes = dmgs.download(&u.asset_url)?;
    verifying();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot prepare download folder: {e}"))?;
    let name = format!("MacMischer-{}-macos-arm64.dmg", u.version);
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
fn announce(current: &str, json: &str) -> Option<State> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = v["tag_name"].as_str()?.trim_start_matches('v').to_string();
    if parse_version(&version)? <= parse_version(current)? {
        return None;
    }
    let notes = v["body"].as_str().unwrap_or_default().to_string();
    let url = v["html_url"].as_str()?.to_string();
    Some(match contract_asset(&version, &v) {
        Some((asset_url, sha256)) => State::Updatable(Update { version, notes, url, asset_url, sha256 }),
        None => State::Available { version, notes, url },
    })
}

/// The single DMG asset (download URL, lowercase hex digest) if the Release meets the contract.
fn contract_asset(version: &str, release: &serde_json::Value) -> Option<(String, String)> {
    let want = format!("MacMischer-{version}-macos-arm64.dmg");
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

struct SystemBrowser;
impl Browser for SystemBrowser {
    fn open(&self, url: &str) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(url).spawn();
        #[cfg(not(target_os = "macos"))]
        let _ = url;
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
        u.settle();
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
        u.settle();
        assert_eq!(u.state(), &State::Idle);
    }

    #[test]
    fn open_release_page_opens_the_release_url_and_keeps_the_banner() {
        let browser = Arc::new(FakeBrowser::default());
        let mut u = Updater::new("0.1.1", Arc::new(FakeFeed(Some(release("v0.2.0")))), browser.clone(), true);
        u.check_at_launch();
        u.settle();
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
        u.settle();
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
        u.settle();
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
        u.settle();
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
        u.settle();
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
        u.settle();
        u.update_now(); // returns although the download is still waiting
        assert!(matches!(u.state(), State::Downloading(_)));
        go.send(()).unwrap();
        u.settle();
        assert!(matches!(u.state(), State::Ready { .. }));
    }

    #[test]
    fn digest_mismatch_deletes_the_file_and_shows_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Ok(b"tampered".to_vec())), dir.path());
        u.update_now();
        u.settle();
        assert!(matches!(u.state(), State::Failed { .. }));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn network_failure_shows_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("offline".into())), dir.path());
        u.update_now();
        u.settle();
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
        u.settle();
        assert!(matches!(u.state(), State::Failed { .. }));
        *dmgs.body.lock().unwrap() = Ok(DMG.to_vec());
        u.update_now(); // Try again
        u.settle();
        assert!(matches!(u.state(), State::Ready { .. }));
        assert_eq!(*dmgs.hits.lock().unwrap(), 2);
    }

    #[test]
    fn update_now_is_ignored_unless_an_update_is_offered_or_failed() {
        let dir = tempfile::tempdir().unwrap();
        let dmgs = FakeDmgs::new(Ok(DMG.to_vec()));
        let mut u = with_dmgs(good_release(), dmgs.clone(), dir.path());
        u.update_now();
        u.settle();
        u.update_now(); // already Ready
        u.settle();
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
        ];
        for (why, feed) in cases {
            let dir = tempfile::tempdir().unwrap();
            let dmgs = FakeDmgs::new(Ok(DMG.to_vec()));
            let mut u = with_dmgs(feed, dmgs.clone(), dir.path());
            assert!(matches!(u.state(), State::Available { .. }), "{why}: {:?}", u.state());
            u.update_now();
            u.settle();
            assert!(matches!(u.state(), State::Available { .. }), "{why}");
            assert_eq!(*dmgs.hits.lock().unwrap(), 0, "{why}");
        }
    }

    #[test]
    fn later_dismisses_an_offered_or_failed_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("x".into())), dir.path());
        u.later();
        assert_eq!(u.state(), &State::Idle);
        let mut u = with_dmgs(good_release(), FakeDmgs::new(Err("x".into())), dir.path());
        u.update_now();
        u.settle();
        u.later();
        assert_eq!(u.state(), &State::Idle);
    }
}

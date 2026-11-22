//! The Updater: checks GitHub for a newer Release at launch and reports its state for the banner.
//! An Update is not an Action; this module is independent of the Console and the mixer.

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

/// What the banner renders. Later tickets add downloading / installing / failed / announce-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Available { version: String, notes: String, url: String },
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
    prefs: UpdatePrefs,
    save_prefs: SavePrefs,
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
            prefs: UpdatePrefs::default(),
            save_prefs: Arc::new(|_| {}),
        }
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
        if let State::Available { version, .. } = &self.state {
            self.prefs.skipped_version = Some(version.clone());
            self.state = State::Idle;
            (self.save_prefs)(&self.prefs);
        }
    }

    /// The real thing: GitHub feed, system browser, macOS arm64 only.
    pub fn production(repaint: impl Fn() + Send + Sync + 'static) -> Updater {
        let prefs = crate::prefs::Prefs::load().updates;
        let save: SavePrefs = Arc::new(|u: &UpdatePrefs| {
            let mut p = crate::prefs::Prefs::load();
            p.updates = u.clone();
            p.save();
        });
        let mut u = Updater::new(
            env!("CARGO_PKG_VERSION"),
            Arc::new(GithubFeed),
            Arc::new(SystemBrowser),
            cfg!(all(target_os = "macos", target_arch = "aarch64")),
        );
        u.on_change = Arc::new(repaint);
        u.with_prefs(prefs, save)
    }

    /// Start the one check of this launch on a plain thread; never blocks the caller.
    pub fn check_at_launch(&mut self) {
        if !self.enabled || !self.prefs.check_at_launch || self.checked {
            return;
        }
        let (tx, rx) = channel();
        let (feed, current, notify) = (self.feed.clone(), self.current.clone(), self.on_change.clone());
        let skipped = self.prefs.skipped_version.clone();
        std::thread::spawn(move || {
            let found = feed
                .latest()
                .and_then(|json| announce(&current, &json))
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
    }

    /// "Later": hide the banner for this launch (the check is not repeated).
    pub fn later(&mut self) {
        self.state();
        if matches!(self.state, State::Available { .. }) {
            self.state = State::Idle;
        }
    }

    /// "Open release page": show the Release in the browser; the banner stays.
    pub fn open_release_page(&mut self) {
        if let State::Available { url, .. } = self.state().clone() {
            self.browser.open(&url);
        }
    }
}

/// The Available state for a feed body, if it holds a strictly newer, well-formed Release.
fn announce(current: &str, json: &str) -> Option<State> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = v["tag_name"].as_str()?.trim_start_matches('v').to_string();
    if parse_version(&version)? <= parse_version(current)? {
        return None;
    }
    Some(State::Available {
        version,
        notes: v["body"].as_str().unwrap_or_default().to_string(),
        url: v["html_url"].as_str()?.to_string(),
    })
}

/// True when the announced Release is not newer than the skipped version.
fn is_skipped(state: &State, skipped: Option<&str>) -> bool {
    match (state, skipped.and_then(parse_version)) {
        (State::Available { version, .. }, Some(skip)) => parse_version(version).is_some_and(|v| v <= skip),
        _ => false,
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
        u.settle();
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
}

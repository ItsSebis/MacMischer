//! Installing an Update on Windows (Inno Setup program) and Linux (AppImage). macOS swaps its
//! `.app` bundle in `updater.rs`. Everything here compiles on every platform so it can be tested
//! anywhere; only the production constructors are platform-specific.
#![allow(dead_code)] // each platform only constructs its own installer

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::updater::{sibling, PackageInstaller};

type Run = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// Runs the downloaded setup program silently and lets it replace and restart the app.
pub struct SetupInstaller {
    run: Run,
}

impl SetupInstaller {
    pub fn new(run: impl Fn(&Path) -> Result<(), String> + Send + Sync + 'static) -> Self {
        SetupInstaller { run: Box::new(run) }
    }
}

impl PackageInstaller for SetupInstaller {
    fn install(&self, package: &Path) -> Result<(), String> {
        (self.run)(package)
    }
}

/// Replaces the running AppImage file with the downloaded one and starts it.
pub struct AppImageInstaller {
    target: PathBuf,
    launch: Run,
}

impl AppImageInstaller {
    pub fn new(target: PathBuf, launch: impl Fn(&Path) -> Result<(), String> + Send + Sync + 'static) -> Self {
        AppImageInstaller { target, launch: Box::new(launch) }
    }
}

impl PackageInstaller for AppImageInstaller {
    /// Never deletes the installed AppImage before the new one is in place; on failure it is back.
    fn install(&self, package: &Path) -> Result<(), String> {
        let mut magic = [0u8; 4];
        std::io::Read::read_exact(&mut std::fs::File::open(package).map_err(|e| e.to_string())?, &mut magic)
            .map_err(|e| format!("Cannot read the downloaded file: {e}"))?;
        if &magic != b"\x7fELF" {
            return Err("The downloaded file is not an AppImage".into());
        }
        let (fresh, old) = (sibling(&self.target, ".new"), sibling(&self.target, ".old"));
        let _ = std::fs::remove_file(&fresh);
        std::fs::copy(package, &fresh).map_err(|e| format!("Cannot copy the new version: {e}"))?;
        let ready = make_executable(&fresh);
        let aside = ready.and_then(|()| {
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&self.target, &old).map_err(|e| format!("Cannot move the installed app aside: {e}"))
        });
        if let Err(e) = aside {
            let _ = std::fs::remove_file(&fresh);
            return Err(e);
        }
        let placed = std::fs::rename(&fresh, &self.target).map_err(|e| format!("Cannot place the new version: {e}"));
        let finished = placed.clone().and_then(|()| (self.launch)(&self.target).map_err(|e| format!("Cannot start the new version: {e}")));
        let Err(e) = finished else { return Ok(()) };
        if placed.is_ok() {
            let _ = std::fs::remove_file(&self.target);
        } else {
            let _ = std::fs::remove_file(&fresh);
        }
        match std::fs::rename(&old, &self.target) {
            Ok(()) => Err(format!("{e}. The installed version was left unchanged.")),
            Err(r) => Err(format!("{e}. Restoring the old version also failed: {r}. It is at {}", old.display())),
        }
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(sibling(&self.target, ".old"));
    }
}

fn make_executable(p: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("Cannot make it executable: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        Ok(())
    }
}

fn writable_dir(dir: &Path) -> bool {
    tempfile::Builder::new().prefix(".mm-write-probe").tempfile_in(dir).is_ok()
}

/// Linux: only when running from an AppImage (`$APPIMAGE`) in a folder we can write to.
pub fn appimage_installer() -> Option<Arc<dyn PackageInstaller>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let target = PathBuf::from(std::env::var_os("APPIMAGE")?);
    writable_dir(target.parent()?).then(|| {
        Arc::new(AppImageInstaller::new(target, |p| {
            // The runtime sets these itself; stale values from this instance must not leak.
            std::process::Command::new(p)
                .env_remove("APPIMAGE")
                .env_remove("APPDIR")
                .env_remove("ARGV0")
                .env_remove("OWD")
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        })) as Arc<dyn PackageInstaller>
    })
}

/// Windows: only when this exe was put there by our setup program (its uninstaller sits beside it).
pub fn windows_installer() -> Option<Arc<dyn PackageInstaller>> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().ok()?;
        exe.parent()?.join("unins000.exe").exists().then(|| {
            Arc::new(SetupInstaller::new(|setup| {
                use std::os::windows::process::CommandExt;
                // DETACHED_PROCESS | CREATE_NO_WINDOW: the setup must outlive this app, which it
                // closes and (via the script's silent [Run] entry) starts again.
                std::process::Command::new(setup)
                    .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
                    .creation_flags(0x0000_0008 | 0x0800_0000)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| format!("Cannot start the installer: {e}"))
            })) as Arc<dyn PackageInstaller>
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn elf(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, [b"\x7fELF".as_slice(), body].concat()).unwrap();
        p
    }

    #[test]
    fn appimage_swap_replaces_and_launches_the_new_file() {
        let d = tempfile::tempdir().unwrap();
        let target = elf(d.path(), "MacMischer.AppImage", b"old");
        let pkg = elf(d.path(), "dl.AppImage", b"new");
        let launched = Arc::new(Mutex::new(Vec::new()));
        let l = launched.clone();
        let inst = AppImageInstaller::new(target.clone(), move |p| {
            l.lock().unwrap().push(p.to_path_buf());
            Ok(())
        });
        inst.install(&pkg).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), [b"\x7fELF".as_slice(), b"new"].concat());
        assert_eq!(*launched.lock().unwrap(), vec![target.clone()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o111, 0o111);
        }
        assert!(sibling(&target, ".old").exists());
        inst.cleanup();
        assert!(!sibling(&target, ".old").exists());
    }

    #[test]
    fn appimage_swap_rolls_back_when_the_launch_fails() {
        let d = tempfile::tempdir().unwrap();
        let target = elf(d.path(), "MacMischer.AppImage", b"old");
        let pkg = elf(d.path(), "dl.AppImage", b"new");
        let inst = AppImageInstaller::new(target.clone(), |_| Err("boom".into()));
        let err = inst.install(&pkg).unwrap_err();
        assert!(err.contains("boom") && err.contains("left unchanged"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), [b"\x7fELF".as_slice(), b"old"].concat());
        assert!(!sibling(&target, ".new").exists());
    }

    #[test]
    fn appimage_swap_rejects_a_file_that_is_not_an_elf() {
        let d = tempfile::tempdir().unwrap();
        let target = elf(d.path(), "MacMischer.AppImage", b"old");
        let pkg = d.path().join("dl");
        std::fs::write(&pkg, b"<html>").unwrap();
        let inst = AppImageInstaller::new(target.clone(), |_| Ok(()));
        assert!(inst.install(&pkg).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), [b"\x7fELF".as_slice(), b"old"].concat());
    }

    #[test]
    fn setup_installer_passes_the_package_to_the_runner() {
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        let inst = SetupInstaller::new(move |p| {
            *s.lock().unwrap() = Some(p.to_path_buf());
            Ok(())
        });
        inst.install(Path::new("setup.exe")).unwrap();
        assert_eq!(seen.lock().unwrap().as_deref(), Some(Path::new("setup.exe")));
    }
}

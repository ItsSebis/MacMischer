//! Background mode: closing the window hides it and the app keeps running (and serving the web
//! remote) from a menu-bar / system-tray icon until "Quit" is chosen there.
//!
//! macOS and Windows only; on other platforms `Tray` is an inert stub and closing the window quits.

use eframe::egui;

/// Whether a window-close request should hide the window instead of quitting.
///
/// Only while the web remote is enabled (otherwise nothing useful would keep running), the
/// preference allows it, and the close is not a deliberate quit (tray Quit, updater handover).
pub fn hide_on_close(remote_enabled: bool, quit_on_close: bool, quitting: bool) -> bool {
    remote_enabled && !quit_on_close && !quitting
}

/// Text of the read-only status line in the tray menu.
pub fn status_text(status: &xair_remote::RemoteStatus, clients: usize) -> String {
    use xair_remote::RemoteStatus;
    match status {
        RemoteStatus::Running { port } => format!("Remote: running on :{port} · {clients} client{}", if clients == 1 { "" } else { "s" }),
        RemoteStatus::Error(e) => format!("Remote: {e}"),
        RemoteStatus::Off => "Remote: off".to_string(),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use imp::Tray;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use stub::Tray;

/// What the operator asked for through the tray this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrayEvents {
    pub toggle_window: bool,
    pub show_window: bool,
    pub quit: bool,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub {
    use super::*;
    #[derive(Default)]
    pub struct Tray;
    impl Tray {
        pub fn new(_ctx: &egui::Context) -> Tray {
            Tray
        }
        pub fn supported(&self) -> bool {
            false
        }
        pub fn poll(&mut self) -> TrayEvents {
            TrayEvents::default()
        }
        pub fn set_status(&mut self, _text: &str) {}
        pub fn set_window_visible(&mut self, _visible: bool) {}
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use super::*;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        icon: Option<TrayIcon>,
        toggle: MenuItem,
        status: MenuItem,
        quit: MenuItem,
        last_status: String,
        menu_rx: std::sync::mpsc::Receiver<MenuEvent>,
        icon_rx: std::sync::mpsc::Receiver<TrayIconEvent>,
    }

    impl Tray {
        /// Must be called on the main thread after the event loop started (the eframe creator).
        pub fn new(ctx: &egui::Context) -> Tray {
            // Wake the UI when the operator clicks the tray, even if the window is hidden.
            // (A handler replaces the crate's own channels, so forward into ours.)
            let (menu_tx, menu_rx) = std::sync::mpsc::channel::<MenuEvent>();
            let (icon_tx, icon_rx) = std::sync::mpsc::channel::<TrayIconEvent>();
            let c = ctx.clone();
            MenuEvent::set_event_handler(Some(std::sync::Arc::new(move |e| {
                let _ = menu_tx.send(e);
                c.request_repaint();
            })));
            let c = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |e| {
                let _ = icon_tx.send(e);
                c.request_repaint();
            }));

            let toggle = MenuItem::new("Hide MacMischer", true, None);
            let status = MenuItem::new("Remote: off", false, None);
            let quit = MenuItem::new("Quit MacMischer", true, None);
            let menu = Menu::new();
            let built = menu
                .append_items(&[&toggle, &PredefinedMenuItem::separator(), &status, &PredefinedMenuItem::separator(), &quit])
                .map_err(|e| e.to_string())
                .and_then(|_| {
                    let rgba = include_bytes!("../../../assets/icon/icon-256.rgba");
                    let img = image::RgbaImage::from_raw(256, 256, rgba.to_vec()).ok_or("bad icon")?;
                    let small = image::imageops::resize(&img, 44, 44, image::imageops::FilterType::Lanczos3);
                    let icon = Icon::from_rgba(small.into_raw(), 44, 44).map_err(|e| e.to_string())?;
                    TrayIconBuilder::new()
                        .with_tooltip("MacMischer")
                        .with_icon(icon)
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false)
                        .build()
                        .map_err(|e| e.to_string())
                });
            let icon = match built {
                Ok(i) => Some(i),
                Err(e) => {
                    tracing::warn!("tray icon unavailable, closing the window will quit: {e}");
                    None
                }
            };
            Tray { icon, toggle, status, quit, last_status: String::new(), menu_rx, icon_rx }
        }

        /// False when the icon could not be created: the window must then never be hidden.
        pub fn supported(&self) -> bool {
            self.icon.is_some()
        }

        pub fn poll(&mut self) -> TrayEvents {
            let mut ev = TrayEvents::default();
            while let Ok(e) = self.menu_rx.try_recv() {
                if e.id == *self.quit.id() {
                    ev.quit = true;
                } else if e.id == *self.toggle.id() {
                    ev.toggle_window = true;
                }
            }
            while let Ok(e) = self.icon_rx.try_recv() {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    ev.show_window = true;
                }
            }
            ev
        }

        pub fn set_status(&mut self, text: &str) {
            if self.last_status != text {
                self.status.set_text(text);
                self.last_status = text.to_string();
            }
        }

        pub fn set_window_visible(&mut self, visible: bool) {
            self.toggle.set_text(if visible { "Hide MacMischer" } else { "Show MacMischer" });
        }
    }
}

/// macOS: drop the Dock icon while the window is hidden (menu-bar only) and bring it back on show.
#[cfg(target_os = "macos")]
pub fn set_dock_icon(visible: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    });
    if visible {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon(_visible: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_only_when_remote_on_and_pref_allows_and_not_quitting() {
        assert!(hide_on_close(true, false, false));
        assert!(!hide_on_close(false, false, false), "remote off: close quits");
        assert!(!hide_on_close(true, true, false), "preference off: close quits");
        assert!(!hide_on_close(true, false, true), "deliberate quit (tray, updater) must close");
    }

    #[test]
    fn status_line() {
        use xair_remote::RemoteStatus;
        assert_eq!(status_text(&RemoteStatus::Running { port: 8080 }, 1), "Remote: running on :8080 · 1 client");
        assert_eq!(status_text(&RemoteStatus::Running { port: 8080 }, 3), "Remote: running on :8080 · 3 clients");
        assert_eq!(status_text(&RemoteStatus::Off, 0), "Remote: off");
    }
}

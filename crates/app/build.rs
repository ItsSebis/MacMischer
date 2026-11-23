//! Embeds the app icon into the Windows exe (Explorer, taskbar, shortcuts).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/icon.ico");
        res.compile().expect("embed the Windows icon");
    }
}

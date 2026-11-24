fn main() {
    // The taskbar, Alt-Tab and Explorer take the icon from the exe, not from the window.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("packaging/windows.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
    println!("cargo:rerun-if-changed=assets/parally.ico");
}

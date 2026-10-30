fn main() {
    // Embed the app icon (Explorer / Alt+Tab / taskbar) on Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/lazykasten.rc", embed_resource::NONE);
    }
    println!("cargo:rerun-if-changed=assets/lazykasten.rc");
    println!("cargo:rerun-if-changed=assets/lazykasten.ico");
}

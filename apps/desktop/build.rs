//! Embeds the app icon in the Windows executable (Explorer, taskbar, Alt-Tab).
//! Other platforms take it from the bundle (M8), so this is a no-op there.

fn main() {
    println!("cargo:rerun-if-changed=assets/brand/magi.rc");
    println!("cargo:rerun-if-changed=assets/brand/magi.ico");
    // Not `#[cfg]`: a build script's cfg is the host's, the target is what counts.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/brand/magi.rc", embed_resource::NONE)
            .manifest_optional()
            .ok();
    }
}

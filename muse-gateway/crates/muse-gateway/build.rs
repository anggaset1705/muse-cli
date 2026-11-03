//! Embed the Windows application manifest (Common Controls v6) into
//! `muse-gateway.exe` so the login window's owner-drawn buttons render
//! with the modern themed look instead of classic comctl32 v5 bevels.
//!
//! Best-effort by design: on a machine without a resource compiler
//! (`rc.exe` on Windows, `llvm-rc` when cross-compiling from Linux)
//! the build still succeeds and the sidecar `muse-gateway.exe.manifest`
//! next to the exe provides the same manifest at runtime — copy
//! `muse-gateway.manifest` alongside the exe during install as
//! `muse-gateway.exe.manifest` (see that file's header comment).

fn main() {
    println!("cargo:rerun-if-changed=muse-gateway.rc");
    println!("cargo:rerun-if-changed=muse-gateway.manifest");
    if std::env::var("TARGET")
        .map(|t| t.contains("windows"))
        .unwrap_or(false)
    {
        // `manifest_optional`: a missing resource compiler is a warning,
        // not a build failure; the sidecar .manifest covers that case.
        let _ = embed_resource::compile("muse-gateway.rc", embed_resource::NONE)
            .manifest_optional();
    }
}

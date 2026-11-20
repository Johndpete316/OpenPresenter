//! Stamps the build with the git commit, so the log, crash reports and Settings say
//! exactly which build is installed. On Windows, also embeds the program icon.

fn main() {
    let commit = std::process::Command::new("git")
        .args(["describe", "--always", "--dirty", "--exclude", "*"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=PP6_BUILD={commit}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    // Icon resource (ID 1): Explorer, the taskbar and the windows use it.
    // Cross-compiling from Linux needs `llvm-rc`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/pp6.ico");
        if let Err(e) = embed_resource::compile_for("assets/pp6.rc", ["pp6"], embed_resource::NONE)
            .manifest_required()
        {
            panic!("couldn't embed the program icon: {e}");
        }
    }
}

//! Stamps the build with the git commit, so the log, crash reports and Settings say
//! exactly which build is installed.

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
}

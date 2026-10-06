//! Bakes the git short hash into the build (`ARTY_GIT_HASH`, shown in Help > About
//! and in crash reports). Without git or a repository it is "unknown".

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    let hash = std::env::var("ARTY_GIT_HASH").ok().or_else(|| git(&["rev-parse", "--short=8", "HEAD"])).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=ARTY_GIT_HASH={hash}");
    println!("cargo:rerun-if-env-changed=ARTY_GIT_HASH");
    // A commit moves HEAD (or the branch reflog, which a worktree keeps for itself).
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}

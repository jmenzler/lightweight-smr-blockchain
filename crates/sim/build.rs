//! Stamps the commit and source-tree state the binary is built from into `SIM_BUILD_COMMIT` / `SIM_BUILD_DIRTY`.
//!
//! `SIM_BUILD_COMMIT_OVERRIDE` / `SIM_BUILD_DIRTY_OVERRIDE` replace the git-derived values; the checked-in lab
//! wasm is built with fixed overrides so that a rebuild of the same sources is byte-identical.

use std::path::Path;
use std::process::Command;

const SOURCE_PATHS: [&str; 4] = ["crates/protocol", "crates/sim", "Cargo.toml", "Cargo.lock"];

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = Path::new(&manifest);

    let top = git(manifest, &["rev-parse", "--show-toplevel"]);
    let commit = git(manifest, &["rev-parse", "HEAD"]);
    let dirty = top.as_deref().is_none_or(|top| {
        let mut args = vec!["status", "--porcelain", "--"];
        args.extend(SOURCE_PATHS);
        git(Path::new(top), &args).is_none_or(|s| !s.is_empty())
    });
    println!("cargo:rerun-if-env-changed=SIM_BUILD_COMMIT_OVERRIDE");
    println!("cargo:rerun-if-env-changed=SIM_BUILD_DIRTY_OVERRIDE");
    let commit = std::env::var("SIM_BUILD_COMMIT_OVERRIDE")
        .ok()
        .or(commit)
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = std::env::var("SIM_BUILD_DIRTY_OVERRIDE").map_or(dirty, |v| {
        v.parse::<bool>()
            .expect("SIM_BUILD_DIRTY_OVERRIDE must be `true` or `false`")
    });
    println!("cargo:rustc-env=SIM_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=SIM_BUILD_DIRTY={dirty}");

    // Without a repository there is nothing to track; cargo's default rerun rule applies.
    let (Some(top), Some(git_dir), Some(common_dir)) = (
        top,
        git(manifest, &["rev-parse", "--absolute-git-dir"]),
        git(
            manifest,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ),
    ) else {
        return;
    };
    for p in [
        format!("{git_dir}/HEAD"),
        format!("{git_dir}/index"),
        format!("{common_dir}/packed-refs"),
        format!("{common_dir}/refs"),
    ] {
        println!("cargo:rerun-if-changed={p}");
    }
    for p in SOURCE_PATHS {
        println!("cargo:rerun-if-changed={top}/{p}");
    }
}

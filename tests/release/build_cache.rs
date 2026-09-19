//! The release build script's cache contract, driven through the real script.
//!
//! A brama release compiles about 400 crates plus skarbiec's own. Until
//! 2026-09-19 `src/release/build.sh` pointed cargo at a directory it had just
//! deleted, overriding the per-product cache Stado hands every build job, so
//! every release paid for the whole graph and `~/.stado/build-cache/brama`
//! never existed on any builder. The only symptom was a slow release: the run
//! register for 0.4.41 reads "compiled 263 crates (~96% of the previous run)".
//!
//! The script now refuses a cargo root inside the job's own output tree. This
//! test drives that refusal through the real script with a real version, a
//! real platform and a real source tree, and reads the exact sentence and
//! exit status a builder would see. The success path is a full compile and is
//! exercised by the release itself, not from here.

use std::path::{Path, PathBuf};
use std::process::Command;

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One output tree per case, under the package's own build directory.
fn output_dir(case: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("release-build-cache-{case}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create the case output directory");
    root
}

fn run(output: &Path, cargo_target_dir: &Path) -> std::process::Output {
    let source = source_dir();
    Command::new("bash")
        .arg(source.join("src/release/build.sh"))
        .current_dir(&source)
        .env("WISENT_SOURCE_DIR", &source)
        .env("WISENT_OUTPUT_DIR", output)
        .env("WISENT_PLATFORM", platform())
        .env("WISENT_VERSION", env!("CARGO_PKG_VERSION"))
        .env("CARGO_TARGET_DIR", cargo_target_dir)
        .output()
        .expect("run the release build script")
}

#[test]
fn the_build_refuses_a_cargo_root_inside_the_job_it_deletes() {
    let output = output_dir("inside");
    let inside = output.join(".build");
    let result = run(&output, &inside);

    let said = String::from_utf8_lossy(&result.stderr);
    assert_eq!(
        result.status.code(),
        Some(65),
        "refusal exit status; stderr was: {said}"
    );
    assert!(
        said.contains(&format!(
            "the builder handed CARGO_TARGET_DIR {} inside this job output {}; \
             compiled dependencies there are deleted with the job",
            inside.display(),
            output.display()
        )),
        "refusal sentence; stderr was: {said}"
    );
    assert!(
        !output.join("stage").exists(),
        "a refused build stages nothing"
    );
}

#[test]
fn the_refusal_removes_nothing_a_builder_already_compiled() {
    let output = output_dir("outside");
    let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("release-build-cache-outside-cargo");
    std::fs::create_dir_all(cache.join("brama/release")).expect("create the cache");
    let sentinel = cache.join("brama/release/.sentinel");
    std::fs::write(&sentinel, b"kept").expect("write the sentinel");

    let refused = run(&output, &output.join(".build"));

    assert_eq!(refused.status.code(), Some(65));
    assert_eq!(
        std::fs::read(&sentinel).expect("the cache still exists"),
        b"kept",
        "a refused build must not touch a cache outside the job"
    );
}

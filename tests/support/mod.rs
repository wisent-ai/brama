//! One test-owned directory, the sibling binaries a story drives, and the
//! isolated Skarbiec vault in `skarbiec/`.
//!
//! What is isolated in this suite is the data, never the component: a story
//! runs the real sibling binaries against a vault the fixture creates with
//! `skarbiec init`, under its own `HOME` and `GNUPGHOME`, seeded by real
//! `skarbiec set-json` writes. A scripted stand-in for another Wisent
//! product cannot go out of date, and so can never tell us when Brama and
//! Skarbiec disagree.
// Every suite includes this module and uses a different part of it: the
// dead-code and unused-import allowances are what let one fixture serve
// all of them without each suite importing a different subset.
#![allow(dead_code, unused_imports)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

mod skarbiec;

pub use skarbiec::SkarbiecVault;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// Characters of the story name kept in a fixture directory's name, and the
/// mask applied to the nanosecond stamp. Both keep the name short: GnuPG
/// binds agent sockets below the fixture root and macOS caps `sun_path` at
/// 104 bytes.
const STORY_STEM: usize = 4;
const STAMP_MASK: u128 = 0xffff_ffff;

/// GnuPG needs the short product-owned root because macOS limits Unix socket paths to 104 bytes.
pub(crate) fn temp_base() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME is required");
    PathBuf::from(home).join(".brama").join("test-runs")
}

/// A name no two fixtures can share, even in one process at one instant:
/// the story, this process, the clock and a counter.
pub(crate) fn fixture_name(story: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let stem: String = story
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(STORY_STEM)
        .collect();
    format!(
        "{stem}{:x}{:x}{sequence:x}",
        std::process::id(),
        nonce & STAMP_MASK
    )
}

pub struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub fn new(story: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(fixture_name(story));
        fs::create_dir_all(&path).expect("create Brama test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Resolve one sibling product's binary in the fleet's fixed order: the
/// product's own environment variable, then `PATH`, then `~/.stado/bin`.
///
/// When none holds it the test fails here, naming what is missing. There is no
/// fourth step: a scripted stand-in, an `#[ignore]` or a silent skip would
/// make this suite go green precisely when the dependency is absent.
pub fn sibling_binary(variable: &str, name: &str) -> PathBuf {
    if let Some(declared) = std::env::var_os(variable) {
        if !declared.is_empty() {
            let path = PathBuf::from(&declared);
            assert!(
                path.is_file(),
                "{variable} names {}, which is not a file: point it at the real {name} binary",
                path.display()
            );
            return path;
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let home = std::env::var_os("HOME").expect("HOME is required");
    let installed = PathBuf::from(home).join(".stado").join("bin").join(name);
    if installed.is_file() {
        return installed;
    }
    panic!(
        "the real `{name}` binary is required and was not found: set {variable} to it, put it on \
         PATH, or install it at ~/.stado/bin/{name} -- this test drives the real product and has \
         no stand-in for it"
    );
}

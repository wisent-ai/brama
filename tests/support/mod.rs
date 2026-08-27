use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub fn new(story: &str) -> Self {
        let home = std::env::var_os("HOME").expect("HOME is required");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = PathBuf::from(home)
            .join(".stado/work/brama-tests")
            .join(format!("{story}-{}-{nonce}", std::process::id()));
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

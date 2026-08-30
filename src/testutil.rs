//! A throwaway sysfs tree, so the readers can be tested without unplugging
//! anything.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct FakeRoot(PathBuf);

impl FakeRoot {
    /// Builds one directory per supply, each holding the given attribute files.
    pub fn new(supplies: &[(&str, &[(&str, &str)])]) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join(format!("power-module-test-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (name, attrs) in supplies {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            for (attr, value) in *attrs {
                fs::write(dir.join(attr), format!("{value}\n")).unwrap();
            }
        }
        fs::create_dir_all(&root).unwrap();
        FakeRoot(root)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for FakeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

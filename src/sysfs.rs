//! Shared plumbing for reading the sysfs power supply class.
//!
//! Every power supply is a directory under [`SYSFS_ROOT`] holding one
//! single-value file per attribute: `type`, `status`, `capacity`, and so on.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the kernel publishes the power supply class.
pub const SYSFS_ROOT: &str = "/sys/class/power_supply";

#[derive(Debug)]
pub enum Error {
    Read { path: PathBuf, source: io::Error },
    BadValue { path: PathBuf, raw: String },
    /// No mains or USB supply at all — a desktop, or a VM.
    NoAcAdapter,
    /// Asked for battery detail on a machine that has none.
    NoBattery,
    /// `--adapter` named something that is not there.
    UnknownSupply(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Error::BadValue { path, raw } => {
                write!(f, "{}: unexpected value {raw:?}", path.display())
            }
            Error::NoAcAdapter => write!(f, "no mains or USB power supply found under {SYSFS_ROOT}"),
            Error::NoBattery => write!(f, "no battery found under {SYSFS_ROOT}"),
            Error::UnknownSupply(name) => write!(f, "no power supply named {name:?}"),
        }
    }
}

/// The name the kernel gave a supply, i.e. its directory name.
pub fn name_of(dir: &Path) -> String {
    dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

pub fn read_trimmed(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .map_err(|source| Error::Read { path: path.to_path_buf(), source })
}

/// Read one attribute of a supply, or `None` if it is absent or unreadable.
///
/// Absent attributes are the normal case rather than a failure: which of
/// `energy_*` and `charge_*` a battery publishes depends on its driver, and a
/// supply can disappear mid-read when a dock is unplugged.
pub fn attr(dir: &Path, name: &str) -> Option<String> {
    read_trimmed(&dir.join(name)).ok()
}

/// Read a numeric attribute. Currents are reported signed by some drivers, so
/// callers that want a magnitude take the absolute value themselves.
pub fn attr_i64(dir: &Path, name: &str) -> Option<i64> {
    attr(dir, name)?.parse().ok()
}

/// Every supply directory, in stable order.
pub fn supply_dirs(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|source| Error::Read { path: root.to_path_buf(), source })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    Ok(dirs)
}

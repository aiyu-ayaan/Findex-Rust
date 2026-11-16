//! Findex core: models, natural sort, history, formatting and fuzzy matching.
//! No UI and no OS calls, so it is testable and benchmarkable headless.

pub mod entry;
pub mod format;
pub mod fuzzy;
pub mod history;
pub mod sort;

pub use entry::{Entry, FLAG_HIDDEN, FLAG_SYMLINK, Kind};
pub use fuzzy::Fuzzy;
pub use history::History;
pub use sort::{Sort, SortKey, natural_cmp, sort_order};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation flag shared between the UI and workers.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    /// Known block device that is not mounted yet.
    Unmounted,
}

/// A volume shown under "This PC".
#[derive(Clone, Debug)]
pub struct Drive {
    pub label: String,
    /// Mount point (or drive root on Windows). Empty when unmounted.
    pub path: PathBuf,
    /// Block device (Linux) used to mount or eject.
    pub device: Option<String>,
    pub kind: DriveKind,
    /// Bytes. 0 when unknown (probe timed out or not mounted).
    pub total: u64,
    pub free: u64,
}

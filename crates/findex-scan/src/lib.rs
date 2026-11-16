//! Recursive fuzzy search.
//!
//! Walks breadth-first so matches near the starting folder arrive first. Each level is split across
//! worker threads; each worker lists directories with the platform's fast reader and scores names with
//! `nucleo`. Results stream to the caller in batches. Symlinked folders and pseudo filesystems are never
//! entered, so loops are impossible.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};

use findex_core::{Cancel, Entry, FLAG_SYMLINK, Fuzzy};

/// Linux pseudo filesystems: huge or infinite, never useful for file search.
const SKIP: &[&str] = &["/proc", "/sys", "/dev", "/run", "/snap"];

pub struct Hit {
    /// Entry with `parent` set to its folder.
    pub entry: Entry,
    pub score: u32,
}

pub struct Options {
    pub include_hidden: bool,
    /// Stop after this many matches.
    pub max_results: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            include_hidden: false,
            max_results: 20_000,
        }
    }
}

/// Searches `root` and its subfolders for names fuzzy-matching `query`. Blocks until done or cancelled.
pub fn search(
    root: &Path,
    query: &str,
    opts: &Options,
    cancel: &Cancel,
    sink: &mut dyn FnMut(Vec<Hit>),
) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let found = AtomicUsize::new(0);
    let mut level = vec![root.to_path_buf()];
    while !level.is_empty() && !cancel.is_cancelled() {
        let next = Mutex::new(Vec::new());
        let cursor = AtomicUsize::new(0);
        let (tx, rx) = mpsc::channel::<Vec<Hit>>();
        std::thread::scope(|s| {
            for _ in 0..threads.min(level.len()) {
                let tx = tx.clone();
                let (level, next, cursor, found) = (&level, &next, &cursor, &found);
                s.spawn(move || {
                    let mut fuzzy = Fuzzy::new(query);
                    loop {
                        let i = cursor.fetch_add(1, Relaxed);
                        if i >= level.len()
                            || cancel.is_cancelled()
                            || found.load(Relaxed) >= opts.max_results
                        {
                            break;
                        }
                        let dir = &level[i];
                        let parent: Arc<Path> = Arc::from(dir.as_path());
                        let (mut hits, mut subdirs) = (Vec::new(), Vec::new());
                        let _ = findex_platform::list_dir(dir, cancel, &mut |chunk| {
                            for mut e in chunk {
                                if e.is_hidden() && !opts.include_hidden {
                                    continue;
                                }
                                if e.is_dir() && e.flags & FLAG_SYMLINK == 0 {
                                    let p = dir.join(e.os_name());
                                    if !SKIP.iter().any(|s| p == Path::new(s)) {
                                        subdirs.push(p);
                                    }
                                }
                                if let Some(score) = fuzzy.score(&e.name) {
                                    e.parent = Some(parent.clone());
                                    hits.push(Hit { entry: e, score });
                                }
                            }
                        });
                        next.lock().unwrap().extend(subdirs);
                        if !hits.is_empty() {
                            found.fetch_add(hits.len(), Relaxed);
                            let _ = tx.send(hits);
                        }
                    }
                });
            }
            drop(tx);
            for hits in rx {
                sink(hits);
            }
        });
        if found.load(Relaxed) >= opts.max_results {
            break;
        }
        level = next.into_inner().unwrap();
    }
}

/// Convenience for tests and the CLI: collect every hit.
pub fn search_all(root: &Path, query: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    search(
        root,
        query,
        &Options::default(),
        &Cancel::default(),
        &mut |hits| out.extend(hits.into_iter().map(|h| h.entry.path_in(root))),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_fuzzy_matches_and_skips_hidden() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("a/b/c")).unwrap();
        std::fs::create_dir_all(t.path().join(".hidden")).unwrap();
        std::fs::write(t.path().join("a/b/c/foo_bar.txt"), b"").unwrap();
        std::fs::write(t.path().join(".hidden/foo_bar.txt"), b"").unwrap();
        std::fs::write(t.path().join("a/zzz.txt"), b"").unwrap();
        let hits = search_all(t.path(), "fobr");
        assert_eq!(hits, vec![t.path().join("a/b/c/foo_bar.txt")]);
    }
}

//! File operations engine. Every copy, move, trash and delete is a [`Job`] reported in the one transfer panel.
//!
//! One worker per destination device runs its jobs in order, so a copy to a USB stick does not
//! wait behind a copy on the SSD, and two writes to the same disk never compete.
//! shortcut: the device is the filesystem id (`st_dev` / drive letter), not the physical disk;
//! two partitions of one disk get separate queues. Upgrade with spike S4.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Trash,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Cancelled,
    Failed(String),
}

/// Live state of one job, shared between the worker and the UI.
#[derive(Debug)]
pub struct Job {
    pub id: u64,
    pub kind: JobKind,
    pub sources: Vec<PathBuf>,
    pub dest: Option<PathBuf>,
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
    pub total_items: AtomicU64,
    pub done_items: AtomicU64,
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
    pub status: Mutex<Status>,
    /// Paths created at the destination's top level (to select them after a paste).
    pub created: Mutex<Vec<PathBuf>>,
}

impl Job {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status(),
            Status::Done | Status::Cancelled | Status::Failed(_)
        )
    }
    fn set_status(&self, s: Status) {
        *self.status.lock().unwrap() = s;
    }
    /// Blocks while paused. Returns false once cancelled. Pausing never loses the byte offset.
    fn keep_going(&self) -> bool {
        while self.paused.load(Relaxed) && !self.cancelled.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        !self.cancelled.load(Relaxed)
    }
}

pub struct Queue {
    /// One sender per device, each with its own worker thread.
    workers: Mutex<HashMap<u64, Sender<Arc<Job>>>>,
    on_finished: Arc<dyn Fn(&Job) + Send + Sync>,
    jobs: Arc<Mutex<Vec<Arc<Job>>>>,
    next_id: AtomicU64,
}

fn worker(on_finished: Arc<dyn Fn(&Job) + Send + Sync>, device: u64) -> Sender<Arc<Job>> {
    let (tx, rx) = channel::<Arc<Job>>();
    std::thread::Builder::new()
        .name(format!("findex-ops-{device}"))
        .spawn(move || {
            for job in rx {
                if !job.cancelled.load(Relaxed) {
                    job.set_status(Status::Running);
                    let r = run(&job);
                    job.set_status(match r {
                        _ if job.cancelled.load(Relaxed) => Status::Cancelled,
                        Ok(()) => Status::Done,
                        Err(e) => Status::Failed(e.to_string()),
                    });
                } else {
                    job.set_status(Status::Cancelled);
                }
                on_finished(&job);
            }
        })
        .expect("spawn ops worker");
    tx
}

impl Queue {
    /// `on_finished` runs on a worker thread after each job ends.
    pub fn new(on_finished: impl Fn(&Job) + Send + Sync + 'static) -> Queue {
        Queue {
            workers: Default::default(),
            on_finished: Arc::new(on_finished),
            jobs: Default::default(),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn submit(&self, kind: JobKind, sources: Vec<PathBuf>, dest: Option<PathBuf>) -> Arc<Job> {
        let job = Arc::new(Job {
            id: self.next_id.fetch_add(1, Relaxed),
            kind,
            sources,
            dest,
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            total_items: AtomicU64::new(0),
            done_items: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            status: Mutex::new(Status::Queued),
            created: Mutex::new(Vec::new()),
        });
        self.jobs.lock().unwrap().push(job.clone());
        // Writes land on the destination; trash and delete work on the source's device.
        let device = findex_platform::device_id(
            job.dest
                .as_deref()
                .or(job.sources.first().map(|p| p.as_path()))
                .unwrap_or(Path::new("/")),
        );
        let mut workers = self.workers.lock().unwrap();
        let tx = workers
            .entry(device)
            .or_insert_with(|| worker(self.on_finished.clone(), device));
        let _ = tx.send(job.clone());
        job
    }

    pub fn jobs(&self) -> Vec<Arc<Job>> {
        self.jobs.lock().unwrap().clone()
    }

    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap().retain(|j| !j.is_finished());
    }
}

fn run(job: &Job) -> io::Result<()> {
    match job.kind {
        JobKind::Copy | JobKind::Move => {
            let dest = job
                .dest
                .as_deref()
                .ok_or_else(|| io::Error::other("no destination"))?;
            let mut plan = Vec::new();
            for s in &job.sources {
                if dest.starts_with(s) && s.is_dir() {
                    return Err(io::Error::other(format!(
                        "Cannot put {} inside itself",
                        s.display()
                    )));
                }
                if job.kind == JobKind::Move && s.parent() == Some(dest) {
                    continue; // moving into the folder it is already in
                }
                let (bytes, items) = measure(s);
                job.total_bytes.fetch_add(bytes, Relaxed);
                job.total_items.fetch_add(items, Relaxed);
                plan.push((s.clone(), bytes, items));
            }
            for (src, bytes, items) in plan {
                if !job.keep_going() {
                    break;
                }
                let dst = unique_target(dest, &src);
                if job.kind == JobKind::Move {
                    match std::fs::rename(&src, &dst) {
                        Ok(()) => {
                            // Same volume: instant, count the whole source as done.
                            job.done_bytes.fetch_add(bytes, Relaxed);
                            job.done_items.fetch_add(items, Relaxed);
                            job.created.lock().unwrap().push(dst);
                            continue;
                        }
                        Err(e) if e.kind() != io::ErrorKind::CrossesDevices => return Err(e),
                        Err(_) => {} // different volume: copy then delete
                    }
                }
                copy_tree(&src, &dst, job)?;
                job.created.lock().unwrap().push(dst);
                if job.kind == JobKind::Move && job.keep_going() {
                    remove(&src)?;
                }
            }
            Ok(())
        }
        JobKind::Trash => {
            job.total_items.store(job.sources.len() as u64, Relaxed);
            for s in &job.sources {
                if !job.keep_going() {
                    break;
                }
                trash::delete(s).map_err(|e| io::Error::other(e.to_string()))?;
                job.done_items.fetch_add(1, Relaxed);
            }
            Ok(())
        }
        JobKind::Delete => {
            job.total_items.store(job.sources.len() as u64, Relaxed);
            for s in &job.sources {
                if !job.keep_going() {
                    break;
                }
                remove(s)?;
                job.done_items.fetch_add(1, Relaxed);
            }
            Ok(())
        }
    }
}

/// (bytes, items) under `p`. Never follows symlinks.
fn measure(p: &Path) -> (u64, u64) {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return (0, 0);
    };
    let (mut bytes, mut items) = (if m.is_file() { m.len() } else { 0 }, 1);
    if m.is_dir() {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            let (b, n) = measure(&e.path());
            bytes += b;
            items += n;
        }
    }
    (bytes, items)
}

/// Copies a file, folder tree or symlink. Symlinks are recreated, never followed.
fn copy_tree(src: &Path, dst: &Path, job: &Job) -> io::Result<()> {
    if !job.keep_going() {
        return Ok(());
    }
    let m = std::fs::symlink_metadata(src)?;
    if m.file_type().is_symlink() {
        findex_platform::copy_symlink(src, dst)?;
    } else if m.is_dir() {
        std::fs::create_dir(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()), job)?;
        }
    } else {
        let base = job.done_bytes.load(Relaxed);
        findex_platform::copy_file(src, dst, &mut |n| {
            job.done_bytes.store(base + n, Relaxed);
            job.keep_going()
        })?;
    }
    job.done_items.fetch_add(1, Relaxed);
    Ok(())
}

/// Removes a file or a whole tree without following symlinks (`remove_dir_all` does not traverse them).
fn remove(p: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(p)?.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// Destination for `src` inside `dir`. On a name clash keeps both, Explorer style:
/// "a.txt" -> "a - Copy.txt" -> "a - Copy (2).txt". Never overwrites.
pub fn unique_target(dir: &Path, src: &Path) -> PathBuf {
    let name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let first = dir.join(&name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let is_dir = src.is_dir();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !is_dir && !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (1..)
        .map(|i| {
            if i == 1 {
                dir.join(format!("{stem} - Copy{ext}"))
            } else {
                dir.join(format!("{stem} - Copy ({i}){ext}"))
            }
        })
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(job: &Job) {
        while !job.is_finished() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn copy_tree_keeps_both_and_move_works() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("inner/f.txt"), b"hello").unwrap();
        std::fs::write(t.path().join("a.txt"), b"x").unwrap();
        let q = Queue::new(|_| {});

        // copy a folder into its own parent: becomes "src - Copy"
        let j = q.submit(
            JobKind::Copy,
            vec![src.clone()],
            Some(t.path().to_path_buf()),
        );
        wait(&j);
        assert_eq!(j.status(), Status::Done);
        assert_eq!(
            std::fs::read(t.path().join("src - Copy/inner/f.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(j.done_bytes.load(Relaxed), 5);

        let j = q.submit(
            JobKind::Copy,
            vec![t.path().join("a.txt")],
            Some(t.path().to_path_buf()),
        );
        wait(&j);
        assert!(t.path().join("a - Copy.txt").exists());

        let dst = t.path().join("dst");
        std::fs::create_dir(&dst).unwrap();
        let j = q.submit(
            JobKind::Move,
            vec![t.path().join("a.txt")],
            Some(dst.clone()),
        );
        wait(&j);
        assert!(dst.join("a.txt").exists() && !t.path().join("a.txt").exists());

        // a folder cannot go inside itself
        let j = q.submit(JobKind::Copy, vec![src.clone()], Some(src.join("inner")));
        wait(&j);
        assert!(matches!(j.status(), Status::Failed(_)));

        let j = q.submit(JobKind::Delete, vec![src.clone()], None);
        wait(&j);
        assert!(!src.exists());
    }
}

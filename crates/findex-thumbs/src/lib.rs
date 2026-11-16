//! Thumbnails.
//!
//! Cache layout follows the freedesktop Thumbnail Managing Standard (`normal/<md5(uri)>.png` with
//! `Thumb::URI` and `Thumb::MTime`), so on Linux Findex shares thumbnails with Nautilus, Dolphin and others.
//! Requests are served newest-first, which keeps the rows currently on screen at the front of the queue.
//! shortcut: images only (via `image`); video, PDF and audio covers need external decoders (M4).

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};

use md5::{Digest, Md5};

/// freedesktop "normal" size.
pub const SIZE: u32 = 128;
const MAX_FILE_BYTES: u64 = 80 << 20;

pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `file://` URI as glib's `g_filename_to_uri` writes it, which the cache key depends on.
pub fn file_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    let bytes = path.as_os_str().as_encoded_bytes();
    for &b in bytes {
        let keep = b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b);
        if keep {
            s.push(b as char);
        } else if cfg!(windows) && b == b'\\' {
            s.push('/');
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn cache_path(cache_dir: &Path, uri: &str) -> PathBuf {
    let hash = Md5::digest(uri.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    cache_dir.join("normal").join(format!("{hex}.png"))
}

fn read_cached(path: &Path, mtime: i64) -> Option<Thumb> {
    let mut dec = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let fresh = reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .any(|t| t.keyword == "Thumb::MTime" && t.text.trim().parse::<i64>().ok() == Some(mtime));
    if !fresh {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Thumb { width: info.width, height: info.height, rgba })
}

fn write_cached(path: &Path, uri: &str, mtime: i64, t: &Thumb) -> std::io::Result<()> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    // Write to a temp name then rename, so readers never see a half-written file.
    let tmp = dir.join(format!(".findex-{}.png", std::process::id()));
    {
        let mut enc = png::Encoder::new(BufWriter::new(File::create(&tmp)?), t.width, t.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Thumb::URI".into(), uri.into()).map_err(std::io::Error::other)?;
        enc.add_text_chunk("Thumb::MTime".into(), mtime.to_string()).map_err(std::io::Error::other)?;
        enc.add_text_chunk("Software".into(), "Findex".into()).map_err(std::io::Error::other)?;
        enc.write_header().map_err(std::io::Error::other)?.write_image_data(&t.rgba).map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, path)
}

fn decode(path: &Path) -> Option<Thumb> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(512 << 20);
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    reader.limits(limits);
    let img = reader.decode().ok()?.thumbnail(SIZE, SIZE).to_rgba8();
    Some(Thumb { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

/// Cached thumbnail, or a freshly made one (then cached). `None` for unsupported or broken files.
pub fn thumbnail(path: &Path, mtime: i64, cache_dir: &Path) -> Option<Thumb> {
    let uri = file_uri(path);
    let cached = cache_path(cache_dir, &uri);
    if let Some(t) = read_cached(&cached, mtime) {
        return Some(t);
    }
    let t = decode(path)?;
    let _ = write_cached(&cached, &uri, mtime, &t);
    Some(t)
}

/// True for extensions this crate can decode.
pub fn supported(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff")
}

struct Request {
    generation: u64,
    key: u64,
    path: PathBuf,
    mtime: i64,
}

/// Pending requests (popped from the end) plus a (generation, key) set to drop duplicates.
type Pending = (Vec<Request>, HashSet<(u64, u64)>);

#[derive(Default)]
struct Shared {
    queue: Mutex<Pending>,
    wake: Condvar,
    generation: AtomicU64,
}

/// Worker pool. `on_ready(generation, key, thumb)` runs on a worker thread.
#[derive(Clone)]
pub struct Service(Arc<Shared>);

impl Service {
    pub fn new(workers: usize, cache_dir: PathBuf, on_ready: impl Fn(u64, u64, Thumb) + Send + Sync + 'static) -> Service {
        let shared = Arc::new(Shared::default());
        let on_ready = Arc::new(on_ready);
        for i in 0..workers.max(1) {
            let (shared, on_ready, cache_dir) = (shared.clone(), on_ready.clone(), cache_dir.clone());
            std::thread::Builder::new()
                .name(format!("findex-thumb-{i}"))
                .spawn(move || loop {
                    let req = {
                        let mut q = shared.queue.lock().unwrap();
                        loop {
                            if let Some(r) = q.0.pop() {
                                q.1.remove(&(r.generation, r.key));
                                break r;
                            }
                            q = shared.wake.wait(q).unwrap();
                        }
                    };
                    if req.generation != shared.generation.load(Relaxed) {
                        continue; // the user navigated away
                    }
                    if let Some(t) = thumbnail(&req.path, req.mtime, &cache_dir) {
                        on_ready(req.generation, req.key, t);
                    }
                })
                .expect("spawn thumbnail worker");
        }
        Service(shared)
    }

    /// Drops queued work for older generations (call on navigation).
    pub fn set_generation(&self, generation: u64) {
        self.0.generation.store(generation, Relaxed);
        let mut q = self.0.queue.lock().unwrap();
        q.0.retain(|r| r.generation == generation);
        q.1.retain(|(g, _)| *g == generation);
    }

    /// Queues a request; newest requests run first. Duplicates are ignored.
    pub fn request(&self, generation: u64, key: u64, path: PathBuf, mtime: i64) {
        let mut q = self.0.queue.lock().unwrap();
        if q.1.insert((generation, key)) {
            q.0.push(Request { generation, key, path, mtime });
            self.0.wake.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_escaping_matches_glib() {
        assert_eq!(file_uri(Path::new("/home/a b/ü.png")), "file:///home/a%20b/%C3%BC.png");
    }

    #[test]
    fn makes_and_reuses_cached_thumbnail() {
        let t = tempfile::tempdir().unwrap();
        let img = t.path().join("p.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255])).save(&img).unwrap();
        let cache = t.path().join("cache");
        let a = thumbnail(&img, 42, &cache).unwrap();
        assert_eq!((a.width, a.height), (128, 64));
        let cached = cache_path(&cache, &file_uri(&img));
        assert!(read_cached(&cached, 42).is_some());
        assert!(read_cached(&cached, 43).is_none(), "stale mtime must miss");
    }
}

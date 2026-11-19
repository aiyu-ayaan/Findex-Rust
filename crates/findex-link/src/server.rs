//! The Link server: HTTPS on the LAN, serving this device's files to paired peers.
//!
//! Every route except `/v1/pair/*` needs `Authorization: Bearer <token>` issued at pairing.
//! Paths on the wire are virtual: `/<root name>/<relative path>`. They never contain `..`,
//! and a resolved path must stay inside its root after symlinks are followed.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};

use crate::http::{self, Head};
use crate::store::{Peer, Store};
use crate::{DEFAULT_PORT, Identity, io_err, sas};

/// What the server tells the app.
#[derive(Clone, Debug)]
pub enum Event {
    /// A peer sent a file; it is saved at this path.
    Received {
        from: String,
        path: PathBuf,
    },
    /// A peer sent text (a link, a note, clipboard contents).
    Text {
        from: String,
        text: String,
    },
    Paired(Peer),
    Unpaired(String),
}

/// A pairing request waiting for the user. Show `code`; the user accepts only when the other screen shows the same.
#[derive(Clone, Debug)]
pub struct PairPrompt {
    pub name: String,
    pub platform: String,
    pub code: String,
}

/// Total and free bytes of the volume holding a path.
pub type SpaceFn = dyn Fn(&Path) -> Option<(u64, u64)> + Send + Sync;

pub struct Config {
    pub identity: Arc<Identity>,
    pub name: String,
    pub platform: String,
    pub version: String,
    pub store: Arc<Store>,
    /// Exposed roots: (name on the wire, folder).
    pub roots: Vec<(String, PathBuf)>,
    /// Where files pushed with `/v1/send` land.
    pub inbox: PathBuf,
    pub space: Box<SpaceFn>,
    pub events: Box<dyn Fn(Event) + Send + Sync>,
    /// Blocks until the user accepts (true) or declines; the caller should time out after about a minute.
    pub approve: Box<dyn Fn(PairPrompt) -> bool + Send + Sync>,
}

struct Pending {
    client_id: String,
    name: String,
    platform: String,
    port: u16,
    commit: Vec<u8>,
    nonce: [u8; 32],
    at: Instant,
}

struct Shared {
    cfg: Config,
    pending: Mutex<HashMap<String, Pending>>,
    /// One pairing prompt at a time.
    pairing: Mutex<()>,
}

pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Binds `DEFAULT_PORT` (or any free port when it is taken) on all interfaces and serves on worker threads.
    pub fn start(cfg: Config) -> io::Result<Server> {
        let listener = TcpListener::bind(("0.0.0.0", DEFAULT_PORT))
            .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))?;
        Server::start_on(cfg, listener)
    }

    pub fn start_on(cfg: Config, listener: TcpListener) -> io::Result<Server> {
        let port = listener.local_addr()?.port();
        let tls = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| io_err(e.to_string()))?
                .with_no_client_auth()
                .with_single_cert(vec![cfg.identity.cert.clone()], cfg.identity.key())
                .map_err(|e| io_err(e.to_string()))?,
        );
        let shared = Arc::new(Shared {
            cfg,
            pending: Mutex::new(HashMap::new()),
            pairing: Mutex::new(()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("findex-link-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = conn else { continue };
                    let (tls, shared) = (tls.clone(), shared.clone());
                    let _ = std::thread::Builder::new()
                        .name("findex-link-conn".into())
                        .spawn(move || {
                            let _ = serve(stream, tls, &shared);
                        });
                }
            })?;
        Ok(Server { port, stop })
    }

    pub fn stop(&self) {
        if !self.stop.swap(true, Ordering::Relaxed) {
            // Wake the accept loop so it sees the flag.
            let _ = TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], self.port)),
                Duration::from_millis(500),
            );
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

type Tls = StreamOwned<ServerConnection, TcpStream>;

/// An error the client sees as `{"error": code}` with an HTTP status.
struct ApiError(u16, &'static str);

type ApiResult = Result<(), ApiError>;

impl From<io::Error> for ApiError {
    fn from(e: io::Error) -> ApiError {
        match e.kind() {
            io::ErrorKind::NotFound => ApiError(404, "not_found"),
            io::ErrorKind::PermissionDenied => ApiError(403, "access_denied"),
            io::ErrorKind::AlreadyExists => ApiError(409, "exists"),
            _ => ApiError(500, "io"),
        }
    }
}

fn serve(stream: TcpStream, tls: Arc<ServerConfig>, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(60)))?;
    let remote = stream.peer_addr()?.ip().to_string();
    let conn = ServerConnection::new(tls).map_err(|e| io_err(e.to_string()))?;
    let mut reader = BufReader::new(StreamOwned::new(conn, stream));
    let Some(head) = http::read_head(&mut reader)? else {
        return Ok(());
    };
    let result = route(&head, &mut reader, &remote, shared);
    let tls = reader.get_mut();
    if let Err(ApiError(status, code)) = result {
        let body = json!({ "error": code }).to_string();
        http::write_head(
            tls,
            status,
            &[("Content-Type", "application/json".into())],
            body.len() as u64,
        )?;
        tls.write_all(body.as_bytes())?;
    }
    tls.flush()?;
    tls.conn.send_close_notify();
    tls.flush()
}

fn send_json(r: &mut BufReader<Tls>, v: Value) -> ApiResult {
    let body = v.to_string();
    let w = r.get_mut();
    http::write_head(
        w,
        200,
        &[("Content-Type", "application/json".into())],
        body.len() as u64,
    )?;
    w.write_all(body.as_bytes())?;
    Ok(())
}

fn read_body(head: &Head, r: &mut BufReader<Tls>, max: u64) -> Result<Vec<u8>, ApiError> {
    let len = head.content_length().ok_or(ApiError(411, "length"))?;
    if len > max {
        return Err(ApiError(413, "too_large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(body)
}

fn read_json(head: &Head, r: &mut BufReader<Tls>) -> Result<Value, ApiError> {
    serde_json::from_slice(&read_body(head, r, 64 * 1024)?).map_err(|_| ApiError(400, "json"))
}

fn route(head: &Head, r: &mut BufReader<Tls>, remote: &str, s: &Shared) -> ApiResult {
    let m = head.first.as_str();
    match (m, head.second.as_str()) {
        ("POST", "/v1/pair/start") => return pair_start(head, r, s),
        ("POST", "/v1/pair/finish") => return pair_finish(head, r, remote, s),
        _ => {}
    }
    let token = head
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError(401, "unauthorized"))?;
    let peer = s
        .cfg
        .store
        .by_token(token.trim())
        .ok_or(ApiError(401, "unauthorized"))?;
    s.cfg.store.touch(&peer.id, remote);
    let path = || head.param("path").ok_or(ApiError(400, "path"));
    match (m, head.second.as_str()) {
        ("GET", "/v1/info") => info(r, s),
        ("GET", "/v1/list") => list(r, s, path()?),
        ("GET", "/v1/stat") => {
            let p = resolve(s, path()?)?;
            let name = p.file_name().map(|n| n.to_string_lossy().to_string());
            send_json(r, item_json(&name.unwrap_or_default(), &fs::metadata(&p)?))
        }
        ("GET", "/v1/file") => get_file(head, r, s, path()?),
        ("PUT", "/v1/file") => {
            let target = resolve_new(s, path()?)?;
            receive(head, r, &target, true)?;
            // A copy into the inbox root is a send, announced like `/v1/send`.
            if target.parent().is_some_and(|d| same_file(d, &s.cfg.inbox)) {
                (s.cfg.events)(Event::Received {
                    from: peer.name.clone(),
                    path: target.clone(),
                });
            }
            send_json(r, json!({ "ok": true }))
        }
        ("POST", "/v1/mkdir") => {
            fs::create_dir(resolve_new(s, path()?)?)?;
            send_json(r, json!({ "ok": true }))
        }
        ("POST", "/v1/delete") => {
            let p = resolve(s, path()?)?;
            if s.cfg.roots.iter().any(|(_, root)| same_file(root, &p)) {
                return Err(ApiError(403, "access_denied"));
            }
            // Recycle bin only; a peer can never delete permanently.
            trash::delete(&p).map_err(|_| ApiError(500, "trash"))?;
            send_json(r, json!({ "ok": true }))
        }
        ("POST", "/v1/rename") => {
            let from = resolve(s, path()?)?;
            let to = resolve_new(s, head.param("to").ok_or(ApiError(400, "to"))?)?;
            if to.symlink_metadata().is_ok() {
                return Err(ApiError(409, "exists"));
            }
            fs::rename(from, to)?;
            send_json(r, json!({ "ok": true }))
        }
        ("POST", "/v1/send") => {
            let name = valid_name(head.param("name").ok_or(ApiError(400, "name"))?)?;
            fs::create_dir_all(&s.cfg.inbox)?;
            let target = unique(&s.cfg.inbox, name);
            receive(head, r, &target, false)?;
            (s.cfg.events)(Event::Received {
                from: peer.name.clone(),
                path: target.clone(),
            });
            let saved = target.file_name().unwrap_or_default().to_string_lossy();
            send_json(r, json!({ "saved": saved }))
        }
        ("POST", "/v1/text") => {
            let text = String::from_utf8(read_body(head, r, 1024 * 1024)?)
                .map_err(|_| ApiError(400, "utf8"))?;
            (s.cfg.events)(Event::Text {
                from: peer.name.clone(),
                text,
            });
            send_json(r, json!({ "ok": true }))
        }
        ("POST", "/v1/unpair") => {
            s.cfg.store.remove(&peer.id)?;
            (s.cfg.events)(Event::Unpaired(peer.id.clone()));
            send_json(r, json!({ "ok": true }))
        }
        _ => Err(ApiError(404, "no_route")),
    }
}

// ---------------- pairing ----------------

fn pair_start(head: &Head, r: &mut BufReader<Tls>, s: &Shared) -> ApiResult {
    let v = read_json(head, r)?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let client_id = field("id").ok_or(ApiError(400, "id"))?;
    let commit = field("commit")
        .and_then(|c| sas::unhex(&c))
        .filter(|c| c.len() == 32)
        .ok_or(ApiError(400, "commit"))?;
    if sas::unhex(&client_id).is_none_or(|b| b.len() != 32) {
        return Err(ApiError(400, "id"));
    }
    let nonce = sas::random32();
    let session = sas::token();
    let mut pending = s.pending.lock().unwrap();
    pending.retain(|_, p| p.at.elapsed() < Duration::from_secs(120));
    if pending.len() >= 16 {
        return Err(ApiError(429, "busy"));
    }
    pending.insert(
        session.clone(),
        Pending {
            client_id,
            name: field("name").unwrap_or_else(|| "Unknown device".into()),
            platform: field("platform").unwrap_or_default(),
            port: v.get("port").and_then(Value::as_u64).unwrap_or(0) as u16,
            commit,
            nonce,
            at: Instant::now(),
        },
    );
    drop(pending);
    send_json(
        r,
        json!({"session": session, "nonce": sas::hex(&nonce), "id": s.cfg.identity.id(),
               "name": s.cfg.name, "platform": s.cfg.platform}),
    )
}

fn pair_finish(head: &Head, r: &mut BufReader<Tls>, remote: &str, s: &Shared) -> ApiResult {
    let v = read_json(head, r)?;
    let field = |k: &str| v.get(k).and_then(Value::as_str);
    let session = field("session").ok_or(ApiError(400, "session"))?;
    let p = s
        .pending
        .lock()
        .unwrap()
        .remove(session)
        .ok_or(ApiError(404, "session"))?;
    let nonce = field("nonce")
        .and_then(sas::unhex)
        .ok_or(ApiError(400, "nonce"))?;
    let token_out = field("token")
        .filter(|t| t.len() >= 32)
        .ok_or(ApiError(400, "token"))?;
    if sas::sha256(&nonce) != p.commit[..] {
        return Err(ApiError(403, "commit"));
    }
    let client_fp = sas::unhex(&p.client_id).ok_or(ApiError(400, "id"))?;
    let code = sas::code(&s.cfg.identity.fingerprint, &client_fp, &nonce, &p.nonce);
    let _one = s.pairing.lock().unwrap();
    let accepted = (s.cfg.approve)(PairPrompt {
        name: p.name.clone(),
        platform: p.platform.clone(),
        code,
    });
    if !accepted {
        return Err(ApiError(403, "declined"));
    }
    let peer = Peer {
        id: p.client_id,
        name: p.name,
        platform: p.platform,
        host: remote.to_string(),
        port: p.port,
        token_out: token_out.to_string(),
        token_in: sas::token(),
    };
    s.cfg.store.upsert(peer.clone())?;
    (s.cfg.events)(Event::Paired(peer.clone()));
    send_json(r, json!({ "token": peer.token_in }))
}

// ---------------- files ----------------

fn info(r: &mut BufReader<Tls>, s: &Shared) -> ApiResult {
    let roots: Vec<Value> = s
        .cfg
        .roots
        .iter()
        .map(|(name, path)| {
            let (total, free) = (s.cfg.space)(path).unwrap_or((0, 0));
            json!({"name": name, "total": total, "free": free})
        })
        .collect();
    send_json(
        r,
        json!({"id": s.cfg.identity.id(), "name": s.cfg.name, "platform": s.cfg.platform,
               "version": s.cfg.version, "roots": roots}),
    )
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

fn item_json(name: &str, meta: &fs::Metadata) -> Value {
    json!({"name": name, "dir": meta.is_dir(), "size": if meta.is_dir() { 0 } else { meta.len() },
           "mtime": mtime_ms(meta)})
}

fn list(r: &mut BufReader<Tls>, s: &Shared, vpath: &str) -> ApiResult {
    let items: Vec<Value> = if vpath.trim_matches('/').is_empty() {
        s.cfg
            .roots
            .iter()
            .map(|(name, _)| json!({"name": name, "dir": true, "size": 0, "mtime": 0}))
            .collect()
    } else {
        let dir = resolve(s, vpath)?;
        fs::read_dir(dir)?
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().into_string().ok()?;
                // Follows symlinks for the kind; resolve() still refuses ones that leave the root.
                let meta = fs::metadata(e.path()).ok()?;
                Some(item_json(&name, &meta))
            })
            .collect()
    };
    send_json(r, json!({ "items": items }))
}

fn get_file(head: &Head, r: &mut BufReader<Tls>, s: &Shared, vpath: &str) -> ApiResult {
    let p = resolve(s, vpath)?;
    let mut f = File::open(&p)?;
    let size = f.metadata()?.len();
    if f.metadata()?.is_dir() {
        return Err(ApiError(400, "is_dir"));
    }
    let mut headers = vec![
        ("Content-Type", "application/octet-stream".to_string()),
        ("Accept-Ranges", "bytes".to_string()),
    ];
    let (status, start, len) = match head.header("range") {
        Some(range) => {
            let (a, b) = http::parse_range(range, size).ok_or(ApiError(416, "range"))?;
            headers.push(("Content-Range", format!("bytes {a}-{b}/{size}")));
            (206, a, b - a + 1)
        }
        None => (200, 0, size),
    };
    f.seek(SeekFrom::Start(start))?;
    let w = r.get_mut();
    http::write_head(w, status, &headers, len)?;
    http::copy_exact(&mut f, w, len, &mut |_| true)?;
    Ok(())
}

/// Streams the request body (`Content-Length` or chunked) into `target` through a hidden part file;
/// the part is removed on failure.
fn receive(head: &Head, r: &mut BufReader<Tls>, target: &Path, overwrite: bool) -> ApiResult {
    let chunked = head
        .header("transfer-encoding")
        .is_some_and(|t| t.eq_ignore_ascii_case("chunked"));
    let len = head.content_length();
    if len.is_none() && !chunked {
        return Err(ApiError(411, "length"));
    }
    if !overwrite && target.symlink_metadata().is_ok() {
        return Err(ApiError(409, "exists"));
    }
    if target.is_dir() {
        return Err(ApiError(409, "is_dir"));
    }
    let dir = target.parent().ok_or(ApiError(400, "path"))?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let part = dir.join(format!(".{name}.findex-part"));
    let result = (|| -> io::Result<()> {
        let mut out = File::create(&part)?;
        match len {
            Some(len) if !chunked => http::copy_exact(r, &mut out, len, &mut |_| true)?,
            _ => {
                io::copy(&mut http::Chunked::new(&mut *r), &mut out)?;
            }
        }
        out.sync_all()?;
        fs::rename(&part, target)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(())
}

fn valid_name(name: &str) -> Result<&str, ApiError> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '\0'));
    if bad {
        Err(ApiError(400, "name"))
    } else {
        Ok(name)
    }
}

/// `name`, or `name (2).ext`, `name (3).ext`… whichever is free in `dir`.
fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if first.symlink_metadata().is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| p.symlink_metadata().is_err())
        .expect("a free name")
}

/// Splits a wire path into its root folder and the checked relative components.
fn split(s: &Shared, vpath: &str) -> Result<(PathBuf, PathBuf), ApiError> {
    let mut parts = vpath.split('/').filter(|p| !p.is_empty());
    let root_name = parts.next().ok_or(ApiError(400, "path"))?;
    let root = s
        .cfg
        .roots
        .iter()
        .find(|(n, _)| n == root_name)
        .map(|(_, p)| p.clone())
        .ok_or(ApiError(404, "not_found"))?;
    let mut rel = PathBuf::new();
    for part in parts {
        valid_name(part)?;
        let c = Path::new(part).components().collect::<Vec<_>>();
        if c.len() != 1 || !matches!(c[0], Component::Normal(_)) {
            return Err(ApiError(400, "path"));
        }
        rel.push(part);
    }
    Ok((root, rel))
}

fn inside(root: &Path, p: &Path) -> Result<(), ApiError> {
    let root = root.canonicalize()?;
    if p.canonicalize()?.starts_with(&root) {
        Ok(())
    } else {
        Err(ApiError(403, "access_denied"))
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// An existing path inside a root.
fn resolve(s: &Shared, vpath: &str) -> Result<PathBuf, ApiError> {
    let (root, rel) = split(s, vpath)?;
    let p = root.join(rel);
    inside(&root, &p)?;
    Ok(p)
}

/// A path to create: its parent must exist inside a root, the name itself is not followed.
fn resolve_new(s: &Shared, vpath: &str) -> Result<PathBuf, ApiError> {
    let (root, rel) = split(s, vpath)?;
    let name = rel.file_name().ok_or(ApiError(400, "path"))?.to_owned();
    let parent = root.join(rel.parent().unwrap_or(Path::new("")));
    inside(&root, &parent)?;
    Ok(parent.join(name))
}

//! Findex Link in the desktop app: the server and discovery, paired phones as a virtual folder in
//! tabs (`findex-phones/<phone>/<root>/…`), and copies between this PC and a phone.
//!
//! Every network call runs on a worker thread and posts its result back, like local I/O.

use std::io::{self, Read};
use std::net::UdpSocket;
use std::path::Component;
use std::sync::Arc;
use std::sync::mpsc;

use findex_core::FLAG_HIDDEN;
use findex_link::client::{Me, pair};
use findex_link::discovery::{Discovery, Found};
use findex_link::server::{Config, Event, PairPrompt, Server};
use findex_link::{Client, DEFAULT_PORT, Identity, Info, Peer, Store};

use super::*;

/// First component of every phone path. It is relative, so it can never clash with a real (absolute) folder.
pub const PHONES: &str = "findex-phones";
const PLATFORM: &str = std::env::consts::OS;

#[derive(Default)]
pub struct LinkState {
    identity: Option<Arc<Identity>>,
    store: Option<Arc<Store>>,
    server: Option<Server>,
    discovery: Option<Discovery>,
    found: Vec<Found>,
    infos: HashMap<String, Info>,
    /// The open Link dialog (kind) and where its answer goes.
    answer: Option<(i32, mpsc::Sender<bool>)>,
    /// This PC phone tiles in display order: (phone name, root name).
    tiles: Vec<(String, String)>,
    /// Phones waiting for the Unpair confirmation (dialog 9).
    unpairing: Vec<String>,
}

pub fn phones_root() -> PathBuf {
    PathBuf::from(PHONES)
}

/// `findex-phones/<phone>/<rest…>` as (phone, "/rest…"); None for a local path.
pub fn split(path: &Path) -> Option<(Option<String>, String)> {
    let mut c = path.components();
    match c.next()? {
        Component::Normal(n) if n == PHONES => {}
        _ => return None,
    }
    let name = |x: Component| x.as_os_str().to_string_lossy().into_owned();
    let phone = c.next().map(name);
    let rest: Vec<String> = c.map(name).collect();
    Some((phone, format!("/{}", rest.join("/"))))
}

pub fn is_remote(path: &Path) -> bool {
    split(path).is_some()
}

fn friendly(phone: &str, e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "{phone} refused the connection. Pair it again from Findex on the phone (Computers)."
        ),
        io::ErrorKind::NotFound => "That item no longer exists on the phone.".into(),
        io::ErrorKind::AlreadyExists => "An item with that name already exists.".into(),
        io::ErrorKind::TimedOut
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::WouldBlock => format!(
            "Couldn't reach {phone}. In Findex on the phone, open Computers and turn on Let computers connect, on the same network."
        ),
        _ => format!("{phone}: {e}"),
    }
}

/// What a listing worker needs to list a phone folder off the UI thread.
pub struct RemoteList {
    phones: Vec<String>,
    target: Option<(String, Client, String)>,
}

impl RemoteList {
    pub fn list(&self) -> Result<Vec<Entry>, String> {
        let Some((phone, client, vpath)) = &self.target else {
            return Ok(self
                .phones
                .iter()
                .map(|n| Entry::new(n.into(), true, 0, 0, 0))
                .collect());
        };
        let items = client.list(vpath).map_err(|e| friendly(phone, &e))?;
        Ok(items
            .into_iter()
            .map(|i| {
                let flags = if i.name.starts_with('.') {
                    FLAG_HIDDEN
                } else {
                    0
                };
                Entry::new(i.name.into(), i.dir, i.size, i.mtime / 1000, flags)
            })
            .collect())
    }
}

/// This computer's name as phones show it.
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Findex PC".into())
}

/// The LAN address other devices would use (no packet is sent: UDP connect only picks a route).
fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// `host`, `host:port` or `[v6]:port`.
fn parse_address(text: &str) -> Option<(String, u16)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(rest) = t.strip_prefix('[') {
        let (host, port) = rest.split_once(']')?;
        let port = port
            .strip_prefix(':')
            .map_or(Some(DEFAULT_PORT), |p| p.parse().ok())?;
        return Some((host.to_string(), port));
    }
    match t.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((t.to_string(), DEFAULT_PORT)),
    }
}

/// `name`, `name (2).ext`, … : the first that `taken` says is free.
fn free_name(name: &str, taken: &mut dyn FnMut(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|c| !taken(c))
        .expect("a free name")
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// Progress shown in the toast, at most four times a second.
struct Progress {
    verb: &'static str,
    target: String,
    done: u64,
    total: u64,
    last: Instant,
}

impl Progress {
    fn add(&mut self, n: u64, name: &str) {
        self.done += n;
        if self.last.elapsed() > Duration::from_millis(250) {
            self.last = Instant::now();
            let pct = (self.done * 100).checked_div(self.total).unwrap_or(0);
            let text = format!("{} {name} to {}… {pct}%", self.verb, self.target);
            post(move |app| app.toast(text, false));
        }
    }
}

/// Total bytes under local paths (files and folders, symlinks not followed).
fn local_size(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p)
            .map(|d| d.flatten().map(|e| local_size(&e.path())).sum())
            .unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

fn upload(c: &Client, src: &Path, dir: &str, prog: &mut Progress) -> io::Result<()> {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = join(
        dir,
        &free_name(&name, &mut |n| c.stat(&join(dir, n)).is_ok()),
    );
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() {
        c.mkdir(&target)?;
        for e in std::fs::read_dir(src)? {
            upload(c, &e?.path(), &target, prog)?;
        }
    } else if meta.is_file() {
        let mut f = std::fs::File::open(src)?;
        let len = meta.len();
        let mut counted = Counted {
            inner: &mut f,
            on: &mut |n| prog.add(n, &name),
        };
        c.upload(&target, &mut counted, len)?;
    }
    Ok(())
}

fn download(c: &Client, src: &str, dest: &Path, prog: &mut Progress) -> io::Result<()> {
    let name = src.rsplit('/').next().unwrap_or("item").to_string();
    let target = dest.join(free_name(&name, &mut |n| {
        dest.join(n).symlink_metadata().is_ok()
    }));
    let item = c.stat(src)?;
    if item.dir {
        std::fs::create_dir(&target)?;
        for child in c.list(src)? {
            download(c, &join(src, &child.name), &target, prog)?;
        }
        return Ok(());
    }
    let part = dest.join(format!(".{name}.findex-part"));
    let r = (|| {
        let mut out = std::fs::File::create(&part)?;
        let mut last = 0;
        c.download(src, &mut out, &mut |done| {
            prog.add(done - last, &name);
            last = done;
            true
        })?;
        std::fs::rename(&part, &target)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

/// Counts bytes read through it.
struct Counted<'a> {
    inner: &'a mut dyn Read,
    on: &'a mut dyn FnMut(u64),
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        (self.on)(n as u64);
        Ok(n)
    }
}

impl App {
    fn link_store(&self) -> Option<Arc<Store>> {
        self.link.store.clone()
    }

    fn peers(&self) -> Vec<Peer> {
        self.link_store().map(|s| s.all()).unwrap_or_default()
    }

    fn peer_named(&self, name: &str) -> Option<Peer> {
        self.peers().into_iter().find(|p| p.name == name)
    }

    pub(super) fn link_peer_names(&self) -> Vec<String> {
        self.peers().into_iter().map(|p| p.name).collect()
    }

    pub(super) fn link_on(&self) -> bool {
        self.settings.link_enabled && self.link.store.is_some()
    }

    /// Starts (or stops, when the setting is off) the server and discovery.
    pub(super) fn link_start(&mut self) {
        self.link.discovery = None;
        if let Some(s) = self.link.server.take() {
            s.stop();
        }
        let Some(dir) = Identity::default_dir() else {
            return;
        };
        let store = self
            .link
            .store
            .get_or_insert_with(|| Arc::new(Store::open(Some(dir.join("peers.json")))))
            .clone();
        self.link_sync();
        if !self.settings.link_enabled {
            return;
        }
        std::thread::spawn(move || {
            let r = (|| -> io::Result<(Arc<Identity>, Server)> {
                let identity = Arc::new(Identity::load_or_create(&dir)?);
                let server = Server::start(server_config(identity.clone(), store))?;
                Ok((identity, server))
            })();
            post(move |app| match r {
                Ok((identity, server))
                    if app.settings.link_enabled && app.link.server.is_none() =>
                {
                    let port = server.port;
                    let disc =
                        Discovery::start(&identity.id(), &device_name(), PLATFORM, port, |f| {
                            post(move |app| app.link_found(f))
                        });
                    app.link.discovery = disc.ok();
                    app.link.identity = Some(identity);
                    app.link.server = Some(server);
                }
                Ok(_) => {}
                Err(e) => app.toast(format!("Findex Link could not start: {e}"), false),
            });
        });
    }

    /// Nav pane, This PC tiles and the open phone folder after the list of phones changed.
    pub(super) fn link_sync(&mut self) {
        self.ui.set_phone_count(if self.link_on() {
            self.peers().len() as i32
        } else {
            0
        });
        self.sync_nav();
        self.link_tiles();
        self.link_refresh_infos();
        if self.tab().path.as_deref() == Some(Path::new(PHONES)) {
            self.load();
        }
    }

    fn link_found(&mut self, found: Vec<Found>) {
        // A paired phone that moved to another address: follow it.
        if let Some(store) = self.link_store() {
            for f in &found {
                if let Some(mut p) = store
                    .all()
                    .into_iter()
                    .find(|p| p.id.starts_with(&f.id) && (p.host != f.host || p.port != f.port))
                {
                    p.host = f.host.clone();
                    p.port = f.port;
                    let _ = store.upsert(p);
                }
            }
        }
        self.link.found = found;
    }

    /// Asks every paired phone for its storage (This PC tiles); offline phones drop out.
    pub(super) fn link_refresh_infos(&mut self) {
        if !self.link_on() {
            return;
        }
        for peer in self.peers() {
            std::thread::spawn(move || {
                let info = Client::for_peer(&peer).info().ok();
                post(move |app| {
                    match info {
                        Some(i) => app.link.infos.insert(peer.id.clone(), i),
                        None => app.link.infos.remove(&peer.id),
                    };
                    app.link_tiles();
                });
            });
        }
    }

    fn link_tiles(&mut self) {
        let mut tiles = Vec::new();
        let mut data = Vec::new();
        if self.link_on() {
            for peer in self.peers() {
                let Some(info) = self.link.infos.get(&peer.id) else {
                    continue;
                };
                for r in &info.roots {
                    let usage = if r.total > 0 {
                        1.0 - r.free as f32 / r.total as f32
                    } else {
                        -1.0
                    };
                    let space = (r.total > 0).then(|| {
                        format!("{} free of {}", format::size(r.free), format::size(r.total))
                    });
                    data.push(DriveData {
                        label: r.name.clone().into(),
                        kind: 5,
                        usage,
                        detail: [Some(peer.name.clone()), space]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ")
                            .into(),
                        low_space: r.total > 0 && (r.free as f64) < r.total as f64 * 0.1,
                        can_eject: false,
                    });
                    tiles.push((peer.name.clone(), r.name.clone()));
                }
            }
        }
        self.link.tiles = tiles;
        self.ui.set_phone_drives(ModelRc::new(VecModel::from(data)));
    }

    pub(super) fn phone_drive_open(&mut self, i: usize) {
        if let Some((phone, root)) = self.link.tiles.get(i).cloned() {
            self.navigate(Some(phones_root().join(phone).join(root)), true);
        }
    }

    /// The listing source for a phone path, prepared on the UI thread.
    pub(super) fn remote_list(&self, path: &Path) -> Option<RemoteList> {
        let (phone, vpath) = split(path)?;
        let phones = self.peers().into_iter().map(|p| p.name).collect();
        let target = match phone {
            None => None,
            Some(name) => {
                let peer = self.peer_named(&name);
                Some((name.clone(), peer.as_ref().map(Client::for_peer)?, vpath))
            }
        };
        Some(RemoteList { phones, target })
    }

    /// The client and wire path for a phone path; toasts when the phone is not paired.
    fn remote(&mut self, path: &Path) -> Option<(String, Client, String)> {
        let (phone, vpath) = split(path)?;
        let phone = phone?;
        match self.peer_named(&phone) {
            Some(p) => Some((phone, Client::for_peer(&p), vpath)),
            None => {
                self.toast(format!("{phone} is not paired any more."), false);
                None
            }
        }
    }

    /// Opens a phone file: download to the cache, then the default app.
    pub(super) fn link_open(&mut self, path: PathBuf) {
        let Some((phone, client, vpath)) = self.remote(&path) else {
            return;
        };
        let name = vpath.rsplit('/').next().unwrap_or("file").to_string();
        let Some(dir) = dirs::cache_dir().map(|d| d.join("findex").join("phone").join(&phone))
        else {
            return;
        };
        self.toast(format!("Opening {name}…"), false);
        std::thread::spawn(move || {
            let target = dir.join(&name);
            let r = (|| {
                std::fs::create_dir_all(&dir)?;
                let mut out = std::fs::File::create(&target)?;
                client.download(&vpath, &mut out, &mut |_| true)?;
                findex_platform::open_default(&target)
            })();
            post(move |app| match r {
                Ok(()) => app.toast(String::new(), false),
                Err(e) => app.toast(friendly(&phone, &e), false),
            });
        });
    }

    /// Copy or move between this PC and a phone (either direction).
    pub(super) fn link_paste(&mut self, sources: Vec<PathBuf>, dest: PathBuf, cut: bool) {
        let store = self.link_store();
        let Some(store) = store else { return };
        let peer = |p: &Path| -> Option<(Client, String, String)> {
            let (phone, vpath) = split(p)?;
            let phone = phone?;
            let peer = store.all().into_iter().find(|x| x.name == phone)?;
            Some((Client::for_peer(&peer), vpath, phone))
        };
        let dest_remote = peer(&dest);
        if is_remote(&dest) && dest_remote.is_none() {
            return self.toast("Open a folder on the phone to paste there.".into(), false);
        }
        let mut plan = Vec::new();
        for s in sources {
            match (peer(&s), &dest_remote) {
                (Some(_), Some(_)) => {
                    return self.toast(
                        "Copying from one phone folder to another isn't supported yet. Copy to this PC first."
                            .into(),
                        false,
                    );
                }
                (src, _) => plan.push((s, src)),
            }
        }
        let n = plan.len();
        let verb = if cut { "Moving" } else { "Copying" };
        std::thread::spawn(move || {
            let target_name = dest_remote
                .as_ref()
                .map(|(_, _, phone)| phone.clone())
                .unwrap_or_else(|| {
                    dest.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dest.display().to_string())
                });
            let mut prog = Progress {
                verb,
                target: target_name.clone(),
                done: 0,
                total: 0,
                last: Instant::now(),
            };
            prog.total = plan
                .iter()
                .map(|(s, r)| match r {
                    None => local_size(s),
                    Some((c, v, _)) => c.stat(v).map_or(0, |i| i.size),
                })
                .sum();
            let mut result = Ok(());
            for (src, remote_src) in &plan {
                let r = match (remote_src, &dest_remote) {
                    (None, Some((c, dir, _))) => upload(c, src, dir, &mut prog).and_then(|_| {
                        if cut {
                            trash::delete(src).map_err(|e| io::Error::other(e.to_string()))
                        } else {
                            Ok(())
                        }
                    }),
                    (Some((c, v, _)), None) => download(c, v, &dest, &mut prog)
                        .and_then(|_| if cut { c.delete(v) } else { Ok(()) }),
                    _ => Ok(()),
                };
                if let Err(e) = r {
                    let phone = remote_src
                        .as_ref()
                        .or(dest_remote.as_ref())
                        .map(|(_, _, p)| p.clone())
                        .unwrap_or_default();
                    result = Err(friendly(&phone, &e));
                    break;
                }
            }
            post(move |app| {
                match result {
                    Ok(()) => app.toast(
                        format!(
                            "{} {n} item{} to {target_name}",
                            if cut { "Moved" } else { "Copied" },
                            if n == 1 { "" } else { "s" }
                        ),
                        false,
                    ),
                    Err(e) => app.toast(e, false),
                }
                app.load();
            });
        });
    }

    /// Phone items go to the phone's recycle bin; at the Phones root, deleting a phone unpairs it.
    pub(super) fn link_delete(&mut self, paths: Vec<PathBuf>) {
        if self.tab().path.as_deref() == Some(Path::new(PHONES)) {
            let names: Vec<String> = paths.iter().filter_map(|p| split(p)?.0).collect();
            if names.is_empty() {
                return;
            }
            self.ui.set_link_text(
                format!(
                    "Unpair {}? It can no longer browse this PC, and this PC can no longer open it. Pair again to reconnect.",
                    names.join(", ")
                )
                .into(),
            );
            self.link.unpairing = names;
            self.ui.invoke_show_link_dialog(9, SharedString::new());
            return;
        }
        let mut work = Vec::new();
        for p in &paths {
            if let Some(t) = self.remote(p) {
                work.push(t);
            }
        }
        std::thread::spawn(move || {
            let r = work
                .iter()
                .try_for_each(|(phone, c, v)| c.delete(v).map_err(|e| friendly(phone, &e)));
            post(move |app| {
                match r {
                    Ok(()) => app.toast("Moved to the phone's Recycle Bin".into(), false),
                    Err(e) => app.toast(e, false),
                }
                app.load();
            });
        });
    }

    pub(super) fn unpair_confirmed(&mut self) {
        let names = std::mem::take(&mut self.link.unpairing);
        self.unpair(names);
    }

    fn unpair(&mut self, names: Vec<String>) {
        let Some(store) = self.link_store() else {
            return;
        };
        let peers: Vec<Peer> = store
            .all()
            .into_iter()
            .filter(|p| names.contains(&p.name))
            .collect();
        std::thread::spawn(move || {
            for p in &peers {
                // Best effort: the phone may be offline; forgetting it here is what matters.
                let _ = Client::for_peer(p).unpair();
                let _ = store.remove(&p.id);
            }
            post(move |app| {
                app.toast(format!("Unpaired {}", names.join(", ")), false);
                app.link_sync();
            });
        });
    }

    pub(super) fn link_rename(&mut self, from: PathBuf, name: String) {
        let Some((phone, c, v)) = self.remote(&from) else {
            return;
        };
        let to = match v.rsplit_once('/') {
            Some((dir, _)) => join(dir, &name),
            None => return,
        };
        let select = OsString::from(&name);
        std::thread::spawn(move || {
            let r = c.rename(&v, &to);
            post(move |app| match r {
                Ok(()) => {
                    app.tab_mut().pending_select = vec![select];
                    app.load();
                }
                Err(e) => app.toast(friendly(&phone, &e), false),
            });
        });
    }

    pub(super) fn link_create(&mut self, dir: PathBuf, folder: bool) {
        let Some((phone, c, v)) = self.remote(&dir) else {
            return;
        };
        if v == "/" {
            return;
        }
        std::thread::spawn(move || {
            let (stem, ext) = if folder {
                ("New folder", "")
            } else {
                ("New Text Document", ".txt")
            };
            let name = free_name(&format!("{stem}{ext}"), &mut |n| {
                c.stat(&join(&v, n)).is_ok()
            });
            let target = join(&v, &name);
            let r = if folder {
                c.mkdir(&target)
            } else {
                c.upload(&target, &mut io::empty(), 0)
            };
            post(move |app| match r {
                Ok(()) => {
                    app.tab_mut().pending_select = vec![OsString::from(name)];
                    app.tab_mut().rename_after = true;
                    app.load();
                }
                Err(e) => app.toast(friendly(&phone, &e), false),
            });
        });
    }

    // ---------------- pairing ----------------

    pub(super) fn pair_phone(&mut self) {
        if !self.settings.link_enabled {
            return self.toast(
                "Turn on Phones in Options > Findex to pair a phone.".into(),
                false,
            );
        }
        let paired = self.peers();
        let nearby: Vec<&Found> = self
            .link
            .found
            .iter()
            .filter(|f| !paired.iter().any(|p| p.id.starts_with(&f.id)))
            .collect();
        let mut text = String::new();
        if nearby.is_empty() {
            text.push_str("No phone found nearby yet. ");
        } else {
            text.push_str("Found nearby: ");
            text.push_str(
                &nearby
                    .iter()
                    .map(|f| format!("{} ({})", f.name, f.host))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push_str(". ");
        }
        text.push_str("In Findex on the phone, open Computers to see its address. ");
        if let (Some(ip), Some(s)) = (local_ip(), &self.link.server) {
            text.push_str(&format!(
                "You can also pair from the phone: this PC is {ip}:{}.",
                s.port
            ));
        }
        let input = nearby
            .first()
            .map(|f| format!("{}:{}", f.host, f.port))
            .unwrap_or_default();
        self.ui.set_link_text(text.into());
        self.ui.invoke_show_link_dialog(5, input.into());
    }

    pub(super) fn pair_submit(&mut self, address: String) {
        let Some((host, port)) = parse_address(&address) else {
            return self.toast("Type the phone's address, like 192.168.1.20.".into(), false);
        };
        let (Some(identity), Some(store), Some(server)) = (
            self.link.identity.clone(),
            self.link_store(),
            self.link.server.as_ref(),
        ) else {
            return self.toast("Findex Link is still starting. Try again.".into(), false);
        };
        let my_port = server.port;
        self.toast(format!("Connecting to {host}…"), false);
        std::thread::spawn(move || {
            let name = device_name();
            let me = Me {
                identity: &identity,
                name: &name,
                platform: PLATFORM,
                port: my_port,
            };
            let r = pair(&host, port, &me, &mut |phone, code| {
                let (tx, rx) = mpsc::channel();
                let (phone, code) = (phone.to_string(), code.to_string());
                post(move |app| {
                    app.link.answer = Some((6, tx));
                    app.ui.set_link_text(
                        format!(
                            "Check that {phone} shows this code too, then choose Pair here and on the phone.\n\n{code}"
                        )
                        .into(),
                    );
                    app.ui.invoke_show_link_dialog(6, SharedString::new());
                    app.toast(String::new(), false);
                });
                let ok = rx.recv_timeout(Duration::from_secs(120)).unwrap_or(false);
                if ok {
                    post(move |app| app.toast("Waiting for the phone to accept…".into(), false));
                }
                ok
            });
            let r = r.and_then(|peer| store.upsert(peer.clone()).map(|_| peer));
            post(move |app| {
                app.ui.invoke_close_link_dialog(6);
                app.link.answer = None;
                match r {
                    Ok(peer) => {
                        app.toast(format!("Paired with {}", peer.name), false);
                        app.link_sync();
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        app.toast(String::new(), false)
                    }
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => app.toast(
                        "The phone declined, or the codes didn't match. Nothing was paired.".into(),
                        false,
                    ),
                    Err(e) => app.toast(format!("Couldn't pair: {e}"), false),
                }
            });
        });
    }

    /// A phone asks to pair (we are the server). The worker waits for [Self::link_answer].
    fn link_prompt(&mut self, p: PairPrompt, tx: mpsc::Sender<bool>) {
        let device = if p.platform.is_empty() {
            p.name.clone()
        } else {
            format!("{} ({})", p.name, p.platform)
        };
        self.ui.set_link_text(
            format!(
                "{device} wants to pair with this PC. Paired devices can browse your Home folder and send you files.\n\nPair only if the phone shows this code:\n\n{}",
                p.code
            )
            .into(),
        );
        self.link.answer = Some((7, tx));
        self.ui.invoke_show_link_dialog(7, SharedString::new());
    }

    pub(super) fn link_answer(&mut self, yes: bool) {
        if let Some((_, tx)) = self.link.answer.take() {
            let _ = tx.send(yes);
        }
    }

    fn link_event(&mut self, e: Event) {
        match e {
            Event::Received { from, path } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.toast(format!("{from} sent {name} · in Downloads/Findex"), false);
                if self.tab().path.as_deref() == path.parent() {
                    self.load();
                }
            }
            Event::Text { from, text } => {
                self.ui.invoke_copy_text(text.clone().into());
                let preview: String = text.chars().take(60).collect();
                self.toast(format!("{from} sent text (copied): {preview}"), false);
            }
            Event::Paired(p) => {
                self.toast(format!("Paired with {}", p.name), false);
                self.link_sync();
            }
            Event::Unpaired(_) => self.link_sync(),
        }
    }

    // ---------------- send to phone ----------------

    pub(super) fn send_to_phone(&mut self) {
        let peers = self.peers();
        match peers.as_slice() {
            [] => {}
            [one] => {
                let name = one.name.clone();
                self.send_to_phone_named(name)
            }
            many => {
                let names: Vec<String> = many.iter().map(|p| p.name.clone()).collect();
                self.ui
                    .set_link_text(format!("Paired phones: {}.", names.join(", ")).into());
                self.ui.invoke_show_link_dialog(8, names[0].clone().into());
            }
        }
    }

    pub(super) fn send_to_phone_named(&mut self, name: String) {
        let Some(peer) = self.peer_named(name.trim()) else {
            return self.toast(format!("No paired phone is called {name}."), false);
        };
        let files: Vec<PathBuf> = self
            .tab()
            .model
            .selected_paths()
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        if files.is_empty() {
            return self.toast(
                "Select files to send. To send a folder, copy it into the phone instead.".into(),
                false,
            );
        }
        let client = Client::for_peer(&peer);
        std::thread::spawn(move || {
            let mut prog = Progress {
                verb: "Sending",
                target: peer.name.clone(),
                done: 0,
                total: files.iter().map(|f| local_size(f)).sum(),
                last: Instant::now(),
            };
            let r = files.iter().try_for_each(|f| -> io::Result<()> {
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut file = std::fs::File::open(f)?;
                let len = file.metadata()?.len();
                let mut counted = Counted {
                    inner: &mut file,
                    on: &mut |n| prog.add(n, &name),
                };
                client.send_file(&name, &mut counted, len).map(drop)
            });
            let n = files.len();
            post(move |app| match r {
                Ok(()) => app.toast(
                    format!(
                        "Sent {n} file{} to {} (Download/Findex on the phone)",
                        if n == 1 { "" } else { "s" },
                        peer.name
                    ),
                    false,
                ),
                Err(e) => app.toast(friendly(&peer.name, &e), false),
            });
        });
    }
}

fn server_config(identity: Arc<Identity>, store: Arc<Store>) -> Config {
    let home = dirs::home_dir().unwrap_or_default();
    let inbox = dirs::download_dir()
        .unwrap_or_else(|| home.join("Downloads"))
        .join("Findex");
    let _ = std::fs::create_dir_all(&inbox);
    Config {
        identity,
        name: device_name(),
        platform: PLATFORM.into(),
        version: update::VERSION.into(),
        store,
        // shortcut: only Home (and the inbox) are exposed; add other drives when phones need them.
        roots: vec![("Home".into(), home), ("Received".into(), inbox.clone())],
        inbox,
        space: Box::new(|p| Some(findex_platform::volume_space(p))),
        events: Box::new(|e| post(move |app| app.link_event(e))),
        approve: Box::new(|prompt| {
            let (tx, rx) = mpsc::channel();
            post(move |app| app.link_prompt(prompt, tx));
            let ok = rx.recv_timeout(Duration::from_secs(60)).unwrap_or(false);
            post(|app| {
                app.ui.invoke_close_link_dialog(7);
                app.link.answer = None;
            });
            ok
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_phone_paths() {
        assert_eq!(split(Path::new("/home/a")), None);
        assert_eq!(split(Path::new(PHONES)), Some((None, "/".into())));
        assert_eq!(
            split(
                &phones_root()
                    .join("Pixel 8")
                    .join("Internal storage")
                    .join("DCIM")
            ),
            Some((Some("Pixel 8".into()), "/Internal storage/DCIM".into()))
        );
        assert_eq!(
            split(&phones_root().join("Pixel")),
            Some((Some("Pixel".into()), "/".into()))
        );
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(
            parse_address("10.0.0.2"),
            Some(("10.0.0.2".into(), DEFAULT_PORT))
        );
        assert_eq!(
            parse_address(" 10.0.0.2:5000 "),
            Some(("10.0.0.2".into(), 5000))
        );
        assert_eq!(parse_address("[fe80::1]:7"), Some(("fe80::1".into(), 7)));
        assert_eq!(
            parse_address("fe80::1"),
            Some(("fe80::1".into(), DEFAULT_PORT))
        );
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("h:x"), None);
    }

    #[test]
    fn picks_free_names() {
        let taken = ["a.txt", "a (2).txt"];
        assert_eq!(free_name("a.txt", &mut |n| taken.contains(&n)), "a (3).txt");
        assert_eq!(free_name("b", &mut |n| taken.contains(&n)), "b");
    }
}

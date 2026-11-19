//! Two Link endpoints in one process: pair over real TLS, then use every route.

use std::fs;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use findex_link::client::{Me, pair};
use findex_link::server::{Config, Event, Server};
use findex_link::{Client, Identity, Store};

struct Side {
    identity: Arc<Identity>,
    store: Arc<Store>,
    _dirs: tempfile::TempDir,
    root: std::path::PathBuf,
    inbox: std::path::PathBuf,
    events: Arc<Mutex<Vec<Event>>>,
    server: Server,
}

fn side(accept: bool, codes: Arc<Mutex<Vec<String>>>) -> Side {
    let dirs = tempfile::tempdir().unwrap();
    let identity = Arc::new(Identity::load_or_create(&dirs.path().join("id")).unwrap());
    let store = Arc::new(Store::open(None));
    let root = dirs.path().join("home");
    let inbox = dirs.path().join("inbox");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir_all(&inbox).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let ev = events.clone();
    let server = Server::start_on(
        Config {
            identity: identity.clone(),
            name: "Desk".into(),
            platform: "linux".into(),
            version: "test".into(),
            store: store.clone(),
            roots: vec![
                ("Home".into(), root.clone()),
                ("Received".into(), inbox.clone()),
            ],
            inbox: inbox.clone(),
            space: Box::new(|_| Some((100, 40))),
            events: Box::new(move |e| ev.lock().unwrap().push(e)),
            approve: Box::new(move |p| {
                codes.lock().unwrap().push(p.code);
                accept
            }),
        },
        TcpListener::bind("127.0.0.1:0").unwrap(),
    )
    .unwrap();
    Side {
        identity,
        store,
        _dirs: dirs,
        root,
        inbox,
        events,
        server,
    }
}

fn pair_with(server: &Side, phone: &Identity) -> std::io::Result<(findex_link::Peer, String)> {
    let mut client_code = String::new();
    let me = Me {
        identity: phone,
        name: "Phone",
        platform: "android",
        port: 1,
    };
    let r = pair("127.0.0.1", server.server.port, &me, &mut |name, code| {
        assert_eq!(name, "Desk");
        client_code = code.to_string();
        true
    });
    r.map(|p| {
        assert_eq!(p.id, server.identity.id());
        (p, client_code)
    })
}

#[test]
fn pair_then_use_every_route() {
    let codes: Arc<Mutex<Vec<String>>> = Arc::default();
    let desk = side(true, codes.clone());
    let phone_dir = tempfile::tempdir().unwrap();
    let phone = Identity::load_or_create(phone_dir.path()).unwrap();

    // Unpaired: everything but pairing is refused.
    let stranger = Client::for_peer(&findex_link::Peer {
        id: desk.identity.id(),
        name: String::new(),
        platform: String::new(),
        host: "127.0.0.1".into(),
        port: desk.server.port,
        token_out: "0".repeat(64),
        token_in: String::new(),
    });
    assert_eq!(
        stranger.list("/").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );

    let (peer, client_code) = pair_with(&desk, &phone).unwrap();
    // Both screens showed the same code.
    assert_eq!(codes.lock().unwrap().as_slice(), [client_code]);
    assert_eq!(
        desk.store.get(&phone.id()).unwrap().token_out,
        peer.token_in
    );

    let c = Client::for_peer(&peer);
    let info = c.info().unwrap();
    assert_eq!(info.roots[0].name, "Home");
    assert_eq!((info.roots[0].total, info.roots[0].free), (100, 40));

    let names: Vec<String> = c.list("/").unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["Home", "Received"]);

    // Chunked upload (unknown length), and a copy into the inbox root counts as a send.
    c.upload_stream("/Home/docs/chunked.txt", &mut &b"streamed body"[..])
        .unwrap();
    assert_eq!(
        fs::read(desk.root.join("docs/chunked.txt")).unwrap(),
        b"streamed body"
    );
    c.upload("/Received/r.txt", &mut &b"r"[..], 1).unwrap();
    assert!(
        desk.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Received { path, .. } if path.ends_with("r.txt")))
    );

    // Upload, list, stat, ranged read (streaming), full download.
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    c.upload("/Home/docs/a b.bin", &mut &data[..], data.len() as u64)
        .unwrap();
    assert_eq!(fs::read(desk.root.join("docs/a b.bin")).unwrap(), data);
    let items = c.list("/Home/docs").unwrap();
    let item = items.iter().find(|i| i.name == "a b.bin").unwrap();
    assert_eq!(item.size, data.len() as u64);
    assert_eq!(
        c.stat("/Home/docs/a b.bin").unwrap().size,
        data.len() as u64
    );
    let (mut body, len) = c.open("/Home/docs/a b.bin", 150_000).unwrap();
    assert_eq!(len, 50_000);
    let mut tail = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut tail).unwrap();
    assert_eq!(&tail[..], &data[150_000..]);
    let mut full = Vec::new();
    c.download("/Home/docs/a b.bin", &mut full, &mut |_| true)
        .unwrap();
    assert_eq!(full, data);

    // mkdir, rename, no overwrite on rename.
    c.mkdir("/Home/new").unwrap();
    c.rename("/Home/docs/a b.bin", "/Home/new/c.bin").unwrap();
    assert!(desk.root.join("new/c.bin").is_file());
    c.mkdir("/Home/docs2").unwrap();
    assert!(c.rename("/Home/docs2", "/Home/new").is_err());

    // Path escapes are refused.
    assert!(c.list("/Home/..").is_err());
    assert!(c.list("/Home/../..").is_err());
    assert!(c.list("/Nope").is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/", desk.root.join("escape")).unwrap();
        assert_eq!(
            c.list("/Home/escape").unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    // A root itself cannot be deleted.
    assert!(c.delete("/Home").is_err());

    // AirDrop-style push lands in the inbox with a unique name.
    assert_eq!(c.send_file("x.txt", &mut &b"one"[..], 3).unwrap(), "x.txt");
    assert_eq!(
        c.send_file("x.txt", &mut &b"two"[..], 3).unwrap(),
        "x (2).txt"
    );
    assert_eq!(fs::read(desk.inbox.join("x (2).txt")).unwrap(), b"two");
    assert!(c.send_file("../evil", &mut &b"z"[..], 1).is_err());

    c.send_text("hello").unwrap();
    let ev = desk.events.lock().unwrap();
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Text { text, .. } if text == "hello"))
    );
    assert!(
        ev.iter()
            .filter(|e| matches!(e, Event::Received { .. }))
            .count()
            == 3
    );
    drop(ev);

    // A pinned client refuses a different server (wrong certificate).
    let other = side(true, Arc::default());
    let mut wrong = peer.clone();
    wrong.port = other.server.port;
    assert!(Client::for_peer(&wrong).info().is_err());

    c.unpair().unwrap();
    assert!(desk.store.get(&phone.id()).is_none());
    assert!(c.info().is_err());
}

#[test]
fn declined_pairing_stores_nothing() {
    let desk = side(false, Arc::default());
    let dir = tempfile::tempdir().unwrap();
    let phone = Identity::load_or_create(dir.path()).unwrap();
    let err = pair_with(&desk, &phone).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(desk.store.all().is_empty());
}

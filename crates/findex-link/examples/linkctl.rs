//! Headless Findex Link endpoint for testing against a phone or emulator.
//!
//! State (identity, peers.json) lives in `$LINKCTL_DIR` (default `./linkctl-state`).
//!
//! ```text
//! linkctl serve DIR [PORT]     serve DIR as "Home" (auto-accepts pairing, prints the code and events)
//! linkctl pair HOST [PORT]     pair with a server (auto-confirms, prints the code)
//! linkctl info | ls PATH | stat PATH
//! linkctl get PATH OUT [OFFSET] | put LOCAL PATH | send LOCAL | text TEXT
//! linkctl mkdir PATH | rm PATH | mv PATH TO | unpair
//! ```
//! Commands after `pair` use the most recently stored peer.

use std::fs::File;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use findex_link::client::{Me, pair};
use findex_link::server::{Config, Server};
use findex_link::{Client, DEFAULT_PORT, Identity, Store};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir =
        PathBuf::from(std::env::var("LINKCTL_DIR").unwrap_or_else(|_| "linkctl-state".into()));
    let identity = Arc::new(Identity::load_or_create(&dir)?);
    let store = Arc::new(Store::open(Some(dir.join("peers.json"))));
    let arg = |i: usize| {
        args.get(i)
            .cloned()
            .ok_or_else(|| std::io::Error::other("missing argument"))
    };
    let peer = || {
        store
            .all()
            .pop()
            .ok_or_else(|| std::io::Error::other("not paired"))
    };
    match arg(0)?.as_str() {
        "serve" => {
            let root = PathBuf::from(arg(1)?);
            let port = args
                .get(2)
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            let inbox = dir.join("inbox");
            std::fs::create_dir_all(&inbox)?;
            let server = Server::start_on(
                Config {
                    identity: identity.clone(),
                    name: "linkctl".into(),
                    platform: "linux".into(),
                    version: "test".into(),
                    store: store.clone(),
                    roots: vec![("Home".into(), root), ("Received".into(), inbox.clone())],
                    inbox,
                    space: Box::new(|_| Some((1000, 400))),
                    events: Box::new(|e| println!("event: {e:?}")),
                    approve: Box::new(|p| {
                        println!(
                            "pair request from {} ({}) code {}",
                            p.name, p.platform, p.code
                        );
                        true
                    }),
                },
                TcpListener::bind(("0.0.0.0", port))?,
            )?;
            println!("serving on port {} as {}", server.port, identity.id());
            loop {
                std::thread::park();
            }
        }
        "pair" => {
            let port = args
                .get(2)
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            let me = Me {
                identity: &identity,
                name: "linkctl",
                platform: "linux",
                port: DEFAULT_PORT,
            };
            let p = pair(&arg(1)?, port, &me, &mut |name, code| {
                println!("pairing with {name}, code {code}");
                true
            })?;
            store.upsert(p.clone())?;
            println!("paired with {} ({})", p.name, p.id);
        }
        cmd => {
            let c = Client::for_peer(&peer()?);
            match cmd {
                "info" => println!("{:?}", c.info()?),
                "ls" => {
                    for i in c.list(&arg(1)?)? {
                        println!(
                            "{}\t{}\t{}\t{}",
                            if i.dir { "d" } else { "-" },
                            i.size,
                            i.mtime,
                            i.name
                        );
                    }
                }
                "stat" => println!("{:?}", c.stat(&arg(1)?)?),
                "get" => {
                    let offset: u64 = args.get(3).and_then(|o| o.parse().ok()).unwrap_or(0);
                    let (mut body, len) = c.open(&arg(1)?, offset)?;
                    let n = std::io::copy(&mut body, &mut File::create(arg(2)?)?)?;
                    println!("got {n} of {len} bytes from offset {offset}");
                }
                "put" => {
                    let mut f = File::open(arg(1)?)?;
                    let len = f.metadata()?.len();
                    c.upload(&arg(2)?, &mut f, len)?;
                    println!("uploaded {len} bytes");
                }
                "send" => {
                    let path = PathBuf::from(arg(1)?);
                    let mut f = File::open(&path)?;
                    let len = f.metadata()?.len();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    println!("saved as {}", c.send_file(&name, &mut f, len)?);
                }
                "text" => c.send_text(&arg(1)?)?,
                "mkdir" => c.mkdir(&arg(1)?)?,
                "rm" => c.delete(&arg(1)?)?,
                "mv" => c.rename(&arg(1)?, &arg(2)?)?,
                "unpair" => {
                    let p = peer()?;
                    c.unpair()?;
                    store.remove(&p.id)?;
                }
                other => return Err(std::io::Error::other(format!("unknown command {other}"))),
            }
            println!("ok");
        }
    }
    Ok(())
}

//! LAN discovery over mDNS / DNS-SD: service `_findex._tcp`, TXT `id` (first 16 hex of the
//! certificate fingerprint, like AirDrop's short hash), `name`, `platform`, `v`.
//! The full fingerprint is checked by TLS on connect; a TXT record is never trusted on its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::io_err;

pub const SERVICE: &str = "_findex._tcp.local.";
/// Length of the fingerprint prefix advertised in TXT `id`.
pub const ID_PREFIX: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Fingerprint prefix ([ID_PREFIX] hex chars).
    pub id: String,
    pub name: String,
    pub platform: String,
    pub host: String,
    pub port: u16,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    found: Arc<Mutex<HashMap<String, Found>>>,
    own: String,
}

impl Discovery {
    /// Advertises this device and browses for others. `on_change` runs on the mDNS thread with the current list.
    pub fn start(
        id: &str,
        name: &str,
        platform: &str,
        port: u16,
        on_change: impl Fn(Vec<Found>) + Send + 'static,
    ) -> std::io::Result<Discovery> {
        let daemon = ServiceDaemon::new().map_err(|e| io_err(e.to_string()))?;
        let short = &id[..ID_PREFIX.min(id.len())];
        let props = [
            ("id", short),
            ("name", name),
            ("platform", platform),
            ("v", "1"),
        ];
        let info = ServiceInfo::new(
            SERVICE,
            &format!("findex-{short}"),
            &format!("findex-{short}.local."),
            "",
            port,
            &props[..],
        )
        .map_err(|e| io_err(e.to_string()))?
        .enable_addr_auto();
        let own = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| io_err(e.to_string()))?;
        let rx = daemon.browse(SERVICE).map_err(|e| io_err(e.to_string()))?;
        let found: Arc<Mutex<HashMap<String, Found>>> = Arc::default();
        let (map, own2, short) = (found.clone(), own.clone(), short.to_string());
        std::thread::Builder::new()
            .name("findex-link-mdns".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    let changed = match event {
                        ServiceEvent::ServiceResolved(info) if info.get_fullname() != own2 => {
                            let txt =
                                |k: &str| info.get_property_val_str(k).unwrap_or("").to_string();
                            // Prefer IPv4: link-local IPv6 needs a scope id the client cannot carry.
                            let mut addrs: Vec<_> = info.get_addresses().iter().copied().collect();
                            addrs.sort_by_key(|a| !a.is_ipv4());
                            match addrs.first() {
                                Some(ip) if txt("id") != short => {
                                    map.lock().unwrap().insert(
                                        info.get_fullname().to_string(),
                                        Found {
                                            id: txt("id"),
                                            name: txt("name"),
                                            platform: txt("platform"),
                                            host: ip.to_string(),
                                            port: info.get_port(),
                                        },
                                    );
                                    true
                                }
                                _ => false,
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            map.lock().unwrap().remove(&fullname).is_some()
                        }
                        ServiceEvent::SearchStopped(_) => break,
                        _ => false,
                    };
                    if changed {
                        on_change(map.lock().unwrap().values().cloned().collect());
                    }
                }
            })?;
        Ok(Discovery { daemon, found, own })
    }

    pub fn found(&self) -> Vec<Found> {
        self.found.lock().unwrap().values().cloned().collect()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.own);
        let _ = self.daemon.shutdown();
    }
}

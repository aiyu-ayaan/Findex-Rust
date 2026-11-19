//! Findex Link: the secure channel between Findex on the desktop and Findex on Android.
//!
//! Design (see `development/docs/10-ecosystem.md`):
//! - Every device has a long-term self-signed EC P-256 identity certificate. Its SHA-256 is the device id.
//! - Devices find each other with mDNS (`_findex._tcp`), or by typed address when multicast is blocked.
//! - Everything runs as HTTP/1.1 over TLS. The client pins the server certificate after pairing.
//! - Pairing is numeric comparison with hash commitments (like Bluetooth SSP): both screens show
//!   the same 6-digit code, and each side issues a random bearer token for the other.

pub mod client;
pub mod discovery;
pub mod http;
pub mod identity;
pub mod qr;
pub mod sas;
pub mod server;
pub mod store;

pub use client::Client;
pub use identity::Identity;
pub use store::{Peer, Store};

/// Default TCP port; the server falls back to any free port when it is taken.
pub const DEFAULT_PORT: u16 = 47_100;
/// TLS server name every Findex identity certificate carries.
pub const SERVER_NAME: &str = "findex.local";

/// One file or folder as the API returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Modification time, Unix milliseconds. 0 when unknown.
    pub mtime: i64,
}

/// A storage root a device exposes, with capacity when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    pub total: u64,
    pub free: u64,
}

/// The answer to `GET /v1/info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub version: String,
    pub roots: Vec<Root>,
}

pub(crate) fn io_err(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::other(msg.into())
}

//! QR pairing. The PC shows `findex://pair?v=1&id=<fp>&h=<ip>,<ip>&p=<port>&s=<secret>&n=<name>`.
//!
//! Scanning it gives the phone the PC's full certificate fingerprint over the camera, an out-of-band
//! channel, so the phone pins the right PC from the first connection and no code needs comparing.
//! The one-time secret proves to the PC that the phone saw its screen, so the PC accepts without a
//! prompt. A wrong or expired secret falls back to the normal code comparison.

use crate::http::{decode, encode, parse_query};
use crate::sas;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairUri {
    /// Hex SHA-256 of the PC's certificate.
    pub id: String,
    /// Addresses to try in order.
    pub hosts: Vec<String>,
    pub port: u16,
    /// One-time pairing secret (hex).
    pub secret: String,
    pub name: String,
}

impl PairUri {
    pub fn to_uri(&self) -> String {
        format!(
            "findex://pair?v=1&id={}&h={}&p={}&s={}&n={}",
            self.id,
            self.hosts.join(","),
            self.port,
            self.secret,
            encode(&self.name)
        )
    }

    pub fn parse(text: &str) -> Option<PairUri> {
        let query = text.trim().strip_prefix("findex://pair?")?;
        let q = parse_query(query);
        let get = |k: &str| q.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        if get("v")? != "1" {
            return None;
        }
        let id = get("id")?.to_ascii_lowercase();
        let secret = get("s")?.to_ascii_lowercase();
        if sas::unhex(&id)?.len() != 32 || sas::unhex(&secret)?.len() < 16 {
            return None;
        }
        let hosts: Vec<String> = get("h")?
            .split(',')
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .collect();
        if hosts.is_empty() {
            return None;
        }
        Some(PairUri {
            id,
            hosts,
            port: get("p")?.parse().ok()?,
            secret,
            name: get("n").and_then(|n| decode(&n)).unwrap_or_default(),
        })
    }
}

/// This machine's private LAN IPv4 addresses (what a phone on the same Wi-Fi can reach).
pub fn lan_addresses() -> Vec<String> {
    let mut v: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() => Some(ip.to_string()),
            _ => None,
        })
        .collect();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let u = PairUri {
            id: "ab".repeat(32),
            hosts: vec!["192.168.1.5".into(), "10.0.0.2".into()],
            port: 47100,
            secret: "cd".repeat(16),
            name: "My PC & co".into(),
        };
        let text = u.to_uri();
        assert!(text.starts_with("findex://pair?v=1&id="));
        assert_eq!(PairUri::parse(&text), Some(u));
    }

    #[test]
    fn rejects_other_codes() {
        assert_eq!(PairUri::parse("https://example.com"), None);
        assert_eq!(PairUri::parse("findex://pair?v=2&id=ab"), None);
        let short = format!(
            "findex://pair?v=1&id=abcd&h=1.2.3.4&p=1&s={}",
            "cd".repeat(16)
        );
        assert_eq!(PairUri::parse(&short), None);
        let no_host = format!(
            "findex://pair?v=1&id={}&h=&p=1&s={}",
            "ab".repeat(32),
            "cd".repeat(16)
        );
        assert_eq!(PairUri::parse(&no_host), None);
    }
}

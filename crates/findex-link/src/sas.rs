//! Fingerprints, nonces and the 6-digit short authentication string shown while pairing.

use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

/// SHA-256 of a certificate's DER bytes: the device id.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    sha256(der)
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest(&SHA256, data).as_ref());
    out
}

pub fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    SystemRandom::new().fill(&mut b).expect("system random");
    b
}

/// A fresh 256-bit bearer token, hex encoded.
pub fn token() -> String {
    hex(&random32())
}

/// `SHA-256("findex-sas-v1" || server fp || client fp || client nonce || server nonce)`,
/// first 4 bytes big-endian, modulo 10^6, zero padded.
/// The client commits to its nonce before it sees the server's, so a man in the middle
/// cannot search for a certificate that makes both codes match (1 in 10^6 per attempt).
pub fn code(
    fp_server: &[u8],
    fp_client: &[u8],
    nonce_client: &[u8],
    nonce_server: &[u8],
) -> String {
    let mut buf = b"findex-sas-v1".to_vec();
    for part in [fp_server, fp_client, nonce_client, nonce_server] {
        buf.extend_from_slice(part);
    }
    let h = sha256(&buf);
    let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
    format!("{n:06}")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Constant-time string comparison for tokens.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared test vector; the Android unit test (`LinkCryptoTest`) checks the same values.
    #[test]
    fn code_matches_vector() {
        let code = code(&[0x11; 32], &[0x22; 32], &[0x33; 32], &[0x44; 32]);
        assert_eq!(code, VECTOR_CODE);
        assert_eq!(
            hex(&sha256(&[0x33; 32])),
            VECTOR_COMMIT,
            "commitment of the client nonce"
        );
    }

    const VECTOR_CODE: &str = "702489";
    const VECTOR_COMMIT: &str = "deb0e38ced1e41de6f92e70e80c418d2d356afaaa99e26f5939dbc7d3ef4772a";

    #[test]
    fn hex_round_trips() {
        assert_eq!(
            unhex(&hex(&[0, 1, 0xab, 0xff])).unwrap(),
            vec![0, 1, 0xab, 0xff]
        );
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
    }
}

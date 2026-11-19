//! The device's long-term identity: a self-signed EC P-256 certificate and its key,
//! kept in the config dir (`findex/link/`). Losing it means pairing again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::{SERVER_NAME, io_err, sas};

pub struct Identity {
    pub cert: CertificateDer<'static>,
    key: Vec<u8>,
    pub fingerprint: [u8; 32],
}

impl Identity {
    /// The default folder: `$XDG_CONFIG_HOME/findex/link` or `%APPDATA%\findex\link`.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("findex").join("link"))
    }

    pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
        let (cert_path, key_path) = (dir.join("identity.der"), dir.join("identity.key"));
        if let (Ok(cert), Ok(key)) = (fs::read(&cert_path), fs::read(&key_path)) {
            return Ok(Identity::from_parts(cert, key));
        }
        let key = rcgen::KeyPair::generate().map_err(|e| io_err(e.to_string()))?;
        let mut params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])
            .map_err(|e| io_err(e.to_string()))?;
        params.not_after = rcgen::date_time_ymd(2125, 1, 1);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Findex");
        let cert = params
            .self_signed(&key)
            .map_err(|e| io_err(e.to_string()))?;
        fs::create_dir_all(dir)?;
        write_private(&key_path, &key.serialize_der())?;
        fs::write(&cert_path, cert.der())?;
        Ok(Identity::from_parts(
            cert.der().to_vec(),
            key.serialize_der(),
        ))
    }

    fn from_parts(cert: Vec<u8>, key: Vec<u8>) -> Identity {
        Identity {
            fingerprint: sas::fingerprint(&cert),
            cert: CertificateDer::from(cert),
            key,
        }
    }

    /// Lowercase hex of [Self::fingerprint]; this is the device id on the wire.
    pub fn id(&self) -> String {
        sas::hex(&self.fingerprint)
    }

    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

/// Writes a secret readable by the owner only (Unix); on Windows the profile ACL applies.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    io::Write::write_all(&mut opts.open(path)?, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path()).unwrap();
        let b = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.id().len(), 64);
    }
}

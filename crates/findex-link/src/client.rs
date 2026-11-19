//! The Link client: talks to a peer's server over TLS, pinning its certificate.

use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::{Value, json};

use crate::http::{self, Head};
use crate::store::Peer;
use crate::{Identity, Info, Item, Root, SERVER_NAME, io_err, sas};

/// Accepts exactly one certificate (or, while pairing, any) but always checks the handshake signature,
/// so the server must hold the private key of the certificate it shows.
#[derive(Debug)]
struct PinVerifier {
    pin: Option<[u8; 32]>,
    seen: Mutex<Option<[u8; 32]>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = sas::fingerprint(end_entity);
        if self.pin.is_some_and(|pin| pin != fp) {
            return Err(rustls::Error::General(
                "certificate does not match the paired device".into(),
            ));
        }
        *self.seen.lock().unwrap() = Some(fp);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub type Tls = StreamOwned<ClientConnection, TcpStream>;

/// A response whose body is still on the wire.
pub struct Response {
    pub status: u16,
    pub head: Head,
    pub body: BufReader<Tls>,
}

impl Response {
    pub fn content_length(&self) -> Option<u64> {
        self.head.content_length()
    }

    fn json(mut self) -> io::Result<Value> {
        let mut buf = Vec::new();
        let len = self.content_length().unwrap_or(0).min(64 * 1024 * 1024);
        (&mut self.body).take(len).read_to_end(&mut buf)?;
        let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
        if (200..300).contains(&self.status) {
            Ok(v)
        } else {
            let code = v.get("error").and_then(Value::as_str).unwrap_or("error");
            Err(link_error(self.status, code))
        }
    }
}

/// An error answer from the peer, as an `io::Error` whose message is `"<status> <code>"`.
fn link_error(status: u16, code: &str) -> io::Error {
    let kind = match status {
        401 | 403 => io::ErrorKind::PermissionDenied,
        404 => io::ErrorKind::NotFound,
        409 => io::ErrorKind::AlreadyExists,
        408 => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("{status} {code}"))
}

pub enum Body<'a> {
    None,
    Bytes(&'a [u8]),
    Stream(&'a mut dyn Read, u64),
    /// Unknown length: sent with `Transfer-Encoding: chunked`.
    Chunked(&'a mut dyn Read),
}

pub struct Client {
    pub host: String,
    pub port: u16,
    pin: Option<[u8; 32]>,
    token: Option<String>,
    timeout: Duration,
}

impl Client {
    /// A client for a paired peer: pins its certificate and authenticates with its token.
    pub fn for_peer(peer: &Peer) -> Client {
        Client {
            host: peer.host.clone(),
            port: peer.port,
            pin: peer.fingerprint(),
            token: Some(peer.token_out.clone()),
            timeout: Duration::from_secs(30),
        }
    }

    /// Opens one TLS connection; returns it with the certificate fingerprint the server showed.
    fn connect(&self) -> io::Result<(Tls, [u8; 32])> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinVerifier {
            pin: self.pin,
            seen: Mutex::new(None),
            provider: provider.clone(),
        });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| io_err(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_no_client_auth();
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io_err("no address"))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        tcp.set_read_timeout(Some(self.timeout))?;
        tcp.set_write_timeout(Some(self.timeout))?;
        let name = ServerName::try_from(SERVER_NAME).map_err(|e| io_err(e.to_string()))?;
        let conn =
            ClientConnection::new(Arc::new(config), name).map_err(|e| io_err(e.to_string()))?;
        let mut tls = StreamOwned::new(conn, tcp);
        while tls.conn.is_handshaking() {
            tls.conn
                .complete_io(&mut tls.sock)
                .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
        }
        let seen = verifier
            .seen
            .lock()
            .unwrap()
            .ok_or_else(|| io_err("no certificate"))?;
        Ok((tls, seen))
    }

    fn send(
        &self,
        method: &str,
        target: &str,
        headers: &[(&str, String)],
        body: Body,
    ) -> io::Result<(Response, [u8; 32])> {
        let (mut tls, seen) = self.connect()?;
        let length = match &body {
            Body::None => "Content-Length: 0".to_string(),
            Body::Bytes(b) => format!("Content-Length: {}", b.len()),
            Body::Stream(_, n) => format!("Content-Length: {n}"),
            Body::Chunked(_) => "Transfer-Encoding: chunked".to_string(),
        };
        let mut head = format!(
            "{method} {target} HTTP/1.1\r\nHost: {SERVER_NAME}\r\n{length}\r\nConnection: close\r\n"
        );
        if let Some(t) = &self.token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        tls.write_all(head.as_bytes())?;
        match body {
            Body::None => {}
            Body::Bytes(b) => tls.write_all(b)?,
            Body::Stream(r, n) => http::copy_exact(r, &mut tls, n, &mut |_| true)?,
            Body::Chunked(r) => {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = r.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    tls.write_all(format!("{n:x}\r\n").as_bytes())?;
                    tls.write_all(&buf[..n])?;
                    tls.write_all(b"\r\n")?;
                }
                tls.write_all(b"0\r\n\r\n")?;
            }
        }
        tls.flush()?;
        let mut reader = BufReader::new(tls);
        let h = http::read_head(&mut reader)?.ok_or_else(|| io_err("no response"))?;
        let status = h.first.parse().map_err(|_| io_err("bad status"))?;
        Ok((
            Response {
                status,
                head: h,
                body: reader,
            },
            seen,
        ))
    }

    fn call(&self, method: &str, route: &str, path: &str, extra: &str) -> io::Result<Value> {
        let target = format!("{route}?path={}{extra}", http::encode(path));
        self.send(method, &target, &[], Body::None)?.0.json()
    }

    pub fn info(&self) -> io::Result<Info> {
        let v = self.send("GET", "/v1/info", &[], Body::None)?.0.json()?;
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Ok(Info {
            id: s(&v, "id"),
            name: s(&v, "name"),
            platform: s(&v, "platform"),
            version: s(&v, "version"),
            roots: v
                .get("roots")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|r| Root {
                            name: s(r, "name"),
                            total: n(r, "total"),
                            free: n(r, "free"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    pub fn list(&self, path: &str) -> io::Result<Vec<Item>> {
        let v = self.call("GET", "/v1/list", path, "")?;
        Ok(v.get("items")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(item).collect())
            .unwrap_or_default())
    }

    pub fn stat(&self, path: &str) -> io::Result<Item> {
        item(&self.call("GET", "/v1/stat", path, "")?).ok_or_else(|| io_err("bad item"))
    }

    /// Opens a file for reading from `offset` (a range request, so media can seek). Returns (body, bytes to read).
    pub fn open(&self, path: &str, offset: u64) -> io::Result<(BufReader<Tls>, u64)> {
        let target = format!("/v1/file?path={}", http::encode(path));
        let range = [("Range", format!("bytes={offset}-"))];
        let headers: &[(&str, String)] = if offset > 0 { &range } else { &[] };
        let (resp, _) = self.send("GET", &target, headers, Body::None)?;
        if !matches!(resp.status, 200 | 206) {
            return resp.json().map(|_| unreachable!());
        }
        let len = resp.content_length().ok_or_else(|| io_err("no length"))?;
        Ok((resp.body, len))
    }

    /// Copies a remote file to `out`; `progress` gets bytes done and returns false to cancel.
    pub fn download(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> io::Result<u64> {
        let (mut body, len) = self.open(path, 0)?;
        http::copy_exact(&mut body, out, len, progress)?;
        Ok(len)
    }

    /// Creates or replaces a file at `path`.
    pub fn upload(&self, path: &str, data: &mut dyn Read, len: u64) -> io::Result<()> {
        let target = format!("/v1/file?path={}", http::encode(path));
        self.send("PUT", &target, &[], Body::Stream(data, len))?
            .0
            .json()
            .map(drop)
    }

    /// Like [Self::upload] for a stream of unknown length (chunked).
    pub fn upload_stream(&self, path: &str, data: &mut dyn Read) -> io::Result<()> {
        let target = format!("/v1/file?path={}", http::encode(path));
        self.send("PUT", &target, &[], Body::Chunked(data))?
            .0
            .json()
            .map(drop)
    }

    /// Pushes a file to the peer's inbox (AirDrop-style). Returns the name it was saved under.
    pub fn send_file(&self, name: &str, data: &mut dyn Read, len: u64) -> io::Result<String> {
        let target = format!("/v1/send?name={}", http::encode(name));
        let v = self
            .send("POST", &target, &[], Body::Stream(data, len))?
            .0
            .json()?;
        Ok(v.get("saved")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string())
    }

    pub fn send_text(&self, text: &str) -> io::Result<()> {
        self.send("POST", "/v1/text", &[], Body::Bytes(text.as_bytes()))?
            .0
            .json()
            .map(drop)
    }

    pub fn mkdir(&self, path: &str) -> io::Result<()> {
        self.call("POST", "/v1/mkdir", path, "").map(drop)
    }

    pub fn delete(&self, path: &str) -> io::Result<()> {
        self.call("POST", "/v1/delete", path, "").map(drop)
    }

    pub fn rename(&self, path: &str, to: &str) -> io::Result<()> {
        let extra = format!("&to={}", http::encode(to));
        self.call("POST", "/v1/rename", path, &extra).map(drop)
    }

    pub fn unpair(&self) -> io::Result<()> {
        self.send("POST", "/v1/unpair", &[], Body::None)?
            .0
            .json()
            .map(drop)
    }
}

fn item(v: &Value) -> Option<Item> {
    Some(Item {
        name: v.get("name")?.as_str()?.to_string(),
        dir: v.get("dir")?.as_bool()?,
        size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
        mtime: v.get("mtime").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// Who we are, as sent while pairing.
pub struct Me<'a> {
    pub identity: &'a Identity,
    pub name: &'a str,
    pub platform: &'a str,
    /// Our own server port, so the peer can call back.
    pub port: u16,
}

/// Pairs with the server at `host:port`.
///
/// `confirm` gets the 6-digit code and blocks until the user says whether the other screen shows
/// the same code. The server then asks its user; this call waits for that answer (up to 90 s).
/// On success the returned peer is ready to store.
pub fn pair(
    host: &str,
    port: u16,
    me: &Me,
    confirm: &mut dyn FnMut(&str, &str) -> bool,
) -> io::Result<Peer> {
    let mut client = Client {
        host: host.to_string(),
        port,
        pin: None,
        token: None,
        timeout: Duration::from_secs(30),
    };
    let nonce_c = sas::random32();
    let start = json!({"id": me.identity.id(), "name": me.name, "platform": me.platform,
                       "port": me.port, "commit": sas::hex(&sas::sha256(&nonce_c))})
    .to_string();
    let (resp, seen) = client.send("POST", "/v1/pair/start", &[], Body::Bytes(start.as_bytes()))?;
    let v = resp.json()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    if s("id") != sas::hex(&seen) {
        return Err(io_err("the device identity does not match its certificate"));
    }
    let nonce_s = sas::unhex(&s("nonce")).ok_or_else(|| io_err("bad nonce"))?;
    let code = sas::code(&seen, &me.identity.fingerprint, &nonce_c, &nonce_s);
    if !confirm(&s("name"), &code) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    // From here on the certificate is pinned: the finish request goes to the same device.
    client.pin = Some(seen);
    client.timeout = Duration::from_secs(90);
    let token_in = sas::token();
    let finish = json!({"session": s("session"), "nonce": sas::hex(&nonce_c), "token": token_in})
        .to_string();
    let v = client
        .send(
            "POST",
            "/v1/pair/finish",
            &[],
            Body::Bytes(finish.as_bytes()),
        )?
        .0
        .json()?;
    let token_out = v
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| io_err("no token"))?
        .to_string();
    Ok(Peer {
        id: sas::hex(&seen),
        name: s("name"),
        platform: s("platform"),
        host: host.to_string(),
        port,
        token_out,
        token_in,
    })
}

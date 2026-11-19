//! Just enough HTTP/1.1 for the Link API: one request per connection, `Content-Length` bodies.

use std::io::{self, BufRead, Read, Write};

use crate::io_err;

const MAX_HEAD: usize = 16 * 1024;

pub struct Head {
    /// Request: method. Response: status code as text.
    pub first: String,
    /// Request: decoded path. Response: reason phrase.
    pub second: String,
    pub query: Vec<(String, String)>,
    /// Lowercase names.
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }
}

/// Reads a request or response head. `Ok(None)` when the peer closed before sending anything.
pub fn read_head(r: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            if lines.is_empty() {
                return Ok(None);
            }
            return Err(io_err("connection closed inside the header"));
        }
        total += n;
        if total > MAX_HEAD {
            return Err(io_err("header too large"));
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let mut it = lines.into_iter();
    let start = it.next().ok_or_else(|| io_err("empty head"))?;
    let mut parts = start.splitn(3, ' ');
    let (a, b, c) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let headers = it
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    if a.starts_with("HTTP/") {
        return Ok(Some(Head {
            first: b.to_string(),
            second: c.to_string(),
            query: Vec::new(),
            headers,
        }));
    }
    let (path, query) = b.split_once('?').unwrap_or((b, ""));
    Ok(Some(Head {
        first: a.to_string(),
        second: decode(path).ok_or_else(|| io_err("bad path encoding"))?,
        query: parse_query(query),
        headers,
    }))
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Some((decode(k)?, decode(v)?))
        })
        .collect()
}

/// Percent-encodes everything except unreserved characters (RFC 3986).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-decodes (`+` stays `+`). None for broken escapes or invalid UTF-8.
pub fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        416 => "Range Not Satisfiable",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

pub fn write_head(
    w: &mut impl Write,
    status: u16,
    headers: &[(&str, String)],
    len: u64,
) -> io::Result<()> {
    let mut s = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str(&format!(
        "Content-Length: {len}\r\nConnection: close\r\n\r\n"
    ));
    w.write_all(s.as_bytes())
}

/// Copies exactly `len` bytes, calling `progress` with the running total.
pub fn copy_exact(
    r: &mut (impl Read + ?Sized),
    w: &mut (impl Write + ?Sized),
    len: u64,
    progress: &mut dyn FnMut(u64) -> bool,
) -> io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    while done < len {
        let want = buf.len().min((len - done) as usize);
        let n = r.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "body cut short",
            ));
        }
        w.write_all(&buf[..n])?;
        done += n as u64;
        if !progress(done) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
    }
    Ok(())
}

/// Decodes a `Transfer-Encoding: chunked` body; reads end at the last (zero) chunk.
pub struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    pub fn new(inner: R) -> Chunked<R> {
        Chunked {
            inner,
            left: 0,
            done: false,
        }
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            if self.inner.read_line(&mut line)? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "chunk size"));
            }
            let size = line.trim().split(';').next().unwrap_or("");
            self.left = u64::from_str_radix(size, 16).map_err(|_| io_err("bad chunk size"))?;
            if self.left == 0 {
                // Trailer lines up to the blank one.
                loop {
                    let mut t = String::new();
                    if self.inner.read_line(&mut t)? == 0 || t.trim().is_empty() {
                        break;
                    }
                }
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "chunk body"));
        }
        self.left -= n as u64;
        if self.left == 0 {
            let mut crlf = String::new();
            self.inner.read_line(&mut crlf)?;
        }
        Ok(n)
    }
}

/// `bytes=a-b` / `bytes=a-` / `bytes=-n` against `size`: inclusive (start, end). None when unsatisfiable.
pub fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split(',').next()?.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size.checked_sub(1)?)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            size.checked_sub(1)?
        } else {
            b.parse::<u64>().ok()?.min(size.checked_sub(1)?)
        };
        (start, end)
    };
    (start <= end && end < size).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes() {
        let s = "/Internal storage/DCIM/a b+ü?&=.jpg";
        assert_eq!(decode(&encode(s)).unwrap(), s);
        assert!(decode("%zz").is_none());
    }

    #[test]
    fn parses_requests() {
        let raw =
            b"GET /v1/list?path=%2FHome%2Fa%20b HTTP/1.1\r\nHost: x\r\nRange: bytes=0-\r\n\r\n";
        let h = read_head(&mut &raw[..]).unwrap().unwrap();
        assert_eq!(h.first, "GET");
        assert_eq!(h.second, "/v1/list");
        assert_eq!(h.param("path"), Some("/Home/a b"));
        assert_eq!(h.header("range"), Some("bytes=0-"));
    }

    #[test]
    fn decodes_chunked_bodies() {
        let raw = b"5\r\nhello\r\n6;x=y\r\n world\r\n0\r\nTrailer: 1\r\n\r\nNEXT";
        let mut r = &raw[..];
        let mut out = String::new();
        Chunked::new(&mut r).read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello world");
        assert_eq!(r, b"NEXT");
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=2-4", 10), Some((2, 4)));
        assert_eq!(parse_range("bytes=5-100", 10), Some((5, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }
}

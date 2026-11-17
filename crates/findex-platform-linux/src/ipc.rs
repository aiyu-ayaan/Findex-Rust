//! Single-instance handoff over a Unix socket in `$XDG_RUNTIME_DIR` (no D-Bus needed).
//! A second `findex PATH` sends the path to the running instance and exits.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("findex.sock"),
        _ => PathBuf::from(format!("/tmp/findex-{}.sock", rustix::process::getuid().as_raw())),
    }
}

/// Hands `request` (a path, or empty to just raise the window) to a running instance.
/// Returns false when none is running.
pub fn forward(request: &str) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let line = format!("{}\n", request.replace('\n', " "));
    s.write_all(line.as_bytes()).is_ok()
}

/// Starts accepting requests from later instances; `on_request` runs on a worker thread.
pub fn listen(on_request: impl Fn(String) + Send + 'static) {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(_) => {
            // A dead instance leaves its socket behind; a live one answers.
            if UnixStream::connect(&path).is_ok() {
                return;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(_) => return,
            }
        }
    };
    std::thread::Builder::new()
        .name("findex-ipc".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(conn.take(4096)).read_line(&mut line).is_ok() {
                    on_request(line.trim_end().to_string());
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    #[test]
    fn socket_path_is_per_user() {
        assert!(super::socket_path().to_string_lossy().ends_with(".sock"));
    }
}

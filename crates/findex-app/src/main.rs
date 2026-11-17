//! Findex: a fast file explorer with the Windows 11 look, for Linux and Windows.

mod app;
mod model;
mod settings;
mod ui_test;
mod update;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

const HELP: &str = "Findex - file explorer

Usage: findex [PATH | URI]
       findex --set-default      make Findex the default file manager
       findex --repair           restore the previous default file manager
       findex --search ROOT QUERY  fuzzy-search from the command line (prints matches and timing)
       findex --version";

/// Accepts plain paths, `~`, and `file://` URIs (as passed by desktop launchers and D-Bus).
pub fn parse_location(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("file://") {
        let bytes = rest.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && let Ok(v) = u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                    16,
                )
            {
                out.push(v);
                i += 3;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        let decoded = String::from_utf8_lossy(&out).into_owned();
        // Windows: file:///C:/x -> C:/x
        let trimmed = if decoded.len() > 2 && decoded.as_bytes()[2] == b':' {
            &decoded[1..]
        } else {
            &decoded
        };
        return PathBuf::from(trimmed);
    }
    if let Some(rest) = s.strip_prefix('~')
        && let Some(h) = dirs::home_dir()
    {
        return h.join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(s)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => return println!("{HELP}"),
        Some("--version") => return println!("findex {}", env!("CARGO_PKG_VERSION")),
        Some(flag @ ("--set-default" | "--repair" | "--unset-default")) => {
            match findex_platform::set_default_file_manager(flag == "--set-default") {
                Ok(msg) => println!("{msg}"),
                Err(e) => {
                    eprintln!("findex: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Some("--search") => {
            let (Some(root), Some(q)) = (args.get(1), args.get(2)) else {
                return eprintln!("{HELP}");
            };
            let t = Instant::now();
            let hits = findex_scan::search_all(&parse_location(root), q);
            for h in hits.iter().take(20) {
                println!("{}", h.display());
            }
            return println!(
                "{} matches in {:.1} ms",
                hits.len(),
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
        _ => {}
    }
    let start = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(|a| parse_location(a))
        .map(|p| std::path::absolute(&p).unwrap_or(p));

    // Single instance: hand the request to a running Findex. Test runs keep their own process.
    let isolated = std::env::var_os("FINDEX_UI_TEST").is_some()
        || std::env::var_os("FINDEX_SNAPSHOT").is_some()
        || std::env::var_os("FINDEX_NO_IPC").is_some();
    // After a self-update the old process is still exiting; never hand the window to it.
    let relaunched = args.iter().any(|a| a == "--relaunched");
    if !isolated
        && !relaunched
        && findex_platform::forward_to_running(
            &start.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        )
    {
        return;
    }

    let ui = findex_ui::AppWindow::new().expect("create window");
    app::start(ui.clone_strong(), start);
    ui.show().expect("show window");
    if !isolated {
        findex_platform::listen_for_instances(|req| {
            slint::invoke_from_event_loop(move || app::with_app(|a| a.open_external(&req))).ok();
        });
    }
    // The native window exists now: request the OS backdrop (Mica on Windows 11).
    slint::Timer::single_shot(Duration::from_millis(0), || {
        app::with_app(|a| a.apply_window_effects())
    });
    // FINDEX_SNAPSHOT=out.png saves a screenshot after startup, for UI tests and docs.
    if let Ok(out) = std::env::var("FINDEX_SNAPSHOT") {
        let delay = std::env::var("FINDEX_SNAPSHOT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(delay), move || {
            if let Some(ui) = weak.upgrade() {
                if let Ok(buf) = ui.window().take_snapshot() {
                    let _ = image::save_buffer(
                        &out,
                        buf.as_bytes(),
                        buf.width(),
                        buf.height(),
                        image::ExtendedColorType::Rgba8,
                    );
                }
                if std::env::var("FINDEX_SNAPSHOT_EXIT").is_ok() {
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }
    if let Ok(script) = std::env::var("FINDEX_UI_TEST") {
        let out = std::env::var("FINDEX_UI_TEST_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("findex-ui-test"));
        ui_test::start(&ui, &script, out);
    }
    slint::run_event_loop_until_quit().expect("event loop");
    app::with_app(|a| a.save_session());
    app::with_app(|a| a.finish_relaunch());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_uris() {
        assert_eq!(
            parse_location("file:///home/a%20b"),
            PathBuf::from("/home/a b")
        );
        assert_eq!(parse_location("/tmp"), PathBuf::from("/tmp"));
    }
}

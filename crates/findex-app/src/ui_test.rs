//! Scripted end-to-end UI tests.
//!
//! `FINDEX_UI_TEST=script.txt FINDEX_UI_TEST_OUT=dir findex PATH` replays real key and pointer events
//! through the window's normal input path, checks UI state, saves snapshots, and exits with status 1 on
//! any failed check. Works the same on Linux and Windows CI.
//!
//! Commands (one per line, `#` comments):
//!   wait MS | key [ctrl+][shift+][alt+]NAME | type TEXT | click X Y | rclick X Y | dblclick X Y
//!   drag X1 Y1 X2 Y2 (left-button drag) | resize W H | snap NAME | quit
//!   tclick X Y | tsnap NAME   (same, in the transfer window)
//!   oclick X Y | osnap NAME   (same, in the options window)
//!   expect FIELD TEXT   (polls up to 4 s) where FIELD is status, selection, title, path, toast, view,
//!   tabs, rows, rows>=, crumbs, error, details
//!   wait-jobs           (until no transfer is running)

use std::time::{Duration, Instant};

use findex_ui::AppWindow;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, SharedString, Timer};

struct Runner {
    ui: slint::Weak<AppWindow>,
    lines: Vec<(usize, String)>,
    pos: usize,
    out: std::path::PathBuf,
    failures: usize,
    checks: usize,
    started: Option<Instant>,
}

pub fn start(ui: &AppWindow, script: &str, out: std::path::PathBuf) {
    let text = std::fs::read_to_string(script).unwrap_or_else(|e| panic!("read {script}: {e}"));
    let lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let _ = std::fs::create_dir_all(&out);
    let runner = Runner {
        ui: ui.as_weak(),
        lines,
        pos: 0,
        out,
        failures: 0,
        checks: 0,
        started: None,
    };
    schedule(runner, 600);
}

fn schedule(r: Runner, ms: u64) {
    Timer::single_shot(Duration::from_millis(ms), move || step(r));
}

fn key_text(name: &str) -> SharedString {
    let k = match name {
        "Down" => Key::DownArrow,
        "Up" => Key::UpArrow,
        "Left" => Key::LeftArrow,
        "Right" => Key::RightArrow,
        "Return" | "Enter" => Key::Return,
        "Escape" | "Esc" => Key::Escape,
        "Delete" => Key::Delete,
        "Backspace" => Key::Backspace,
        "Tab" => Key::Tab,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F5" => Key::F5,
        "Space" => return " ".into(),
        other => return other.into(),
    };
    k.into()
}

fn press(ui: &AppWindow, combo: &str) {
    let parts: Vec<&str> = combo.split('+').collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let mods: Vec<SharedString> = mods
        .iter()
        .map(|m| match *m {
            "ctrl" => Key::Control.into(),
            "shift" => Key::Shift.into(),
            "alt" => Key::Alt.into(),
            other => panic!("unknown modifier {other}"),
        })
        .collect();
    let w = ui.window();
    for m in &mods {
        w.dispatch_event(WindowEvent::KeyPressed { text: m.clone() });
    }
    let t = key_text(key[0]);
    w.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
    w.dispatch_event(WindowEvent::KeyReleased { text: t });
    for m in mods.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: m.clone() });
    }
}

/// Click at (x, y); `mods` may contain "ctrl" and/or "shift", held during the click.
fn click(w: &slint::Window, x: f32, y: f32, mods: &str, button: PointerEventButton, double: bool) {
    let held: Vec<SharedString> = [("ctrl", Key::Control), ("shift", Key::Shift)]
        .into_iter()
        .filter(|(n, _)| mods.contains(n))
        .map(|(_, k)| k.into())
        .collect();
    for k in &held {
        w.dispatch_event(WindowEvent::KeyPressed { text: k.clone() });
    }
    let position = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position });
    for _ in 0..if double { 2 } else { 1 } {
        w.dispatch_event(WindowEvent::PointerPressed { position, button });
        w.dispatch_event(WindowEvent::PointerReleased { position, button });
    }
    for k in held.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: k.clone() });
    }
}

fn field(ui: &AppWindow, name: &str) -> String {
    let tabs = ui.get_tabs();
    match name {
        "status" => ui.get_status_text().into(),
        "selection" => ui.get_selection_text().into(),
        "title" => tabs
            .row_data(ui.get_active_tab() as usize)
            .map(|t| t.title.to_string())
            .unwrap_or_default(),
        "path" => ui.get_path_text().into(),
        "toast" => ui.get_toast_text().into(),
        "view" => ui.get_view_mode().to_string(),
        "tabs" => tabs.row_count().to_string(),
        "rows" | "rows>=" => ui.get_rows().row_count().to_string(),
        "crumbs" => ui
            .get_crumbs()
            .iter()
            .map(|c| c.label.to_string())
            .collect::<Vec<_>>()
            .join(" > "),
        "error" => ui.get_error_text().into(),
        "details" => ui.get_details().name.into(),
        "jobs" => ui.get_active_jobs().to_string(),
        "transfer-window" => transfer().window().is_visible().to_string(),
        "options-window" => options().window().is_visible().to_string(),
        "statusbar" => ui.get_status_bar().to_string(),
        "checkboxes" => ui.get_check_boxes().to_string(),
        "dark" => ui.global::<findex_ui::Theme>().get_dark().to_string(),
        "transparency" => ui
            .global::<findex_ui::Theme>()
            .get_transparency()
            .to_string(),
        "name" | "name=" => ui
            .get_rows()
            .row_data(ui.get_focus_row().max(0) as usize)
            .map(|r| r.name.to_string())
            .unwrap_or_default(),
        "nav" => ui
            .get_nav_items()
            .iter()
            .map(|n| n.label.to_string())
            .collect::<Vec<_>>()
            .join("|"),
        "job" => ui
            .get_jobs()
            .row_data(0)
            .map(|j| j.detail.to_string())
            .unwrap_or_default(),
        other => format!("<unknown field {other}>"),
    }
}

fn transfer() -> findex_ui::TransferWindow {
    let mut w = None;
    crate::app::with_app(|a| w = Some(a.transfer_window().clone_strong()));
    w.expect("app running")
}

fn options() -> findex_ui::OptionsWindow {
    let mut w = None;
    crate::app::with_app(|a| w = Some(a.options_window().clone_strong()));
    w.expect("app running")
}

fn check(field_name: &str, actual: &str, want: &str) -> bool {
    match field_name {
        "rows>=" => actual.parse::<usize>().unwrap_or(0) >= want.parse().unwrap_or(usize::MAX),
        "tabs" | "rows" | "view" | "jobs" | "transfer-window" | "options-window" | "statusbar"
        | "checkboxes" | "dark" | "transparency" | "name=" => actual == want,
        _ => actual.contains(want),
    }
}

fn step(mut r: Runner) {
    let Some(ui) = r.ui.upgrade() else { return };
    let Some((line_no, line)) = r.lines.get(r.pos).cloned() else {
        return finish(&r);
    };
    let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
    let nums: Vec<f32> = rest
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let mut delay = 120;
    match cmd {
        "wait" => delay = nums.first().copied().unwrap_or(300.0) as u64,
        "key" => press(&ui, rest),
        "type" => {
            for c in rest.chars() {
                let t: SharedString = c.to_string().into();
                ui.window()
                    .dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
                ui.window()
                    .dispatch_event(WindowEvent::KeyReleased { text: t });
            }
        }
        "click" => click(
            ui.window(),
            nums[0],
            nums[1],
            rest,
            PointerEventButton::Left,
            false,
        ),
        "dblclick" => click(
            ui.window(),
            nums[0],
            nums[1],
            rest,
            PointerEventButton::Left,
            true,
        ),
        "drag" => {
            let w = ui.window();
            let (a, b) = (
                LogicalPosition::new(nums[0], nums[1]),
                LogicalPosition::new(nums[2], nums[3]),
            );
            let button = PointerEventButton::Left;
            w.dispatch_event(WindowEvent::PointerMoved { position: a });
            w.dispatch_event(WindowEvent::PointerPressed { position: a, button });
            w.dispatch_event(WindowEvent::PointerMoved { position: b });
            w.dispatch_event(WindowEvent::PointerReleased { position: b, button });
        }
        "rclick" => click(
            ui.window(),
            nums[0],
            nums[1],
            rest,
            PointerEventButton::Right,
            false,
        ),
        "resize" => {
            ui.window()
                .set_size(slint::LogicalSize::new(nums[0], nums[1]));
            delay = 400;
        }
        "tclick" => {
            let tw = transfer();
            click(
                tw.window(),
                nums[0],
                nums[1],
                rest,
                PointerEventButton::Left,
                false,
            )
        }
        "oclick" => {
            let ow = options();
            click(
                ow.window(),
                nums[0],
                nums[1],
                rest,
                PointerEventButton::Left,
                false,
            )
        }
        "snap" | "tsnap" | "osnap" => {
            let (tw, ow) = (transfer(), options());
            let window = match cmd {
                "tsnap" => tw.window(),
                "osnap" => ow.window(),
                _ => ui.window(),
            };
            match window.take_snapshot() {
                Ok(buf) => {
                    let p = r.out.join(format!("{rest}.png"));
                    let _ = image::save_buffer(
                        &p,
                        buf.as_bytes(),
                        buf.width(),
                        buf.height(),
                        image::ExtendedColorType::Rgba8,
                    );
                    println!("snap  {}", p.display());
                }
                Err(e) => println!("snap failed: {e}"),
            }
        }
        "expect" | "wait-jobs" => {
            let (name, want) = if cmd == "wait-jobs" {
                ("jobs", "0")
            } else {
                rest.split_once(' ').unwrap_or((rest, ""))
            };
            let actual = field(&ui, name);
            let started = *r.started.get_or_insert_with(Instant::now);
            if !check(name, &actual, want) {
                if started.elapsed() < Duration::from_secs(if cmd == "wait-jobs" { 180 } else { 4 })
                {
                    return schedule(r, 50); // poll again
                }
                println!("FAIL  line {line_no}: {name}: expected '{want}', got '{actual}'");
                r.failures += 1;
            } else {
                println!("ok    line {line_no}: {name} '{want}'");
            }
            r.started = None;
            r.checks += 1;
            delay = 20;
        }
        "quit" => return finish(&r),
        other => {
            println!("FAIL  line {line_no}: unknown command {other}");
            r.failures += 1;
        }
    }
    r.pos += 1;
    schedule(r, delay);
}

fn finish(r: &Runner) {
    println!("{} checks, {} failed", r.checks, r.failures);
    std::process::exit(if r.failures > 0 { 1 } else { 0 });
}

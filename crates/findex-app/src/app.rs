//! UI controller. Owns tabs and app-wide services and lives on the UI thread.
//! Workers never touch UI state directly: they `post` closures back to the event loop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use findex_core::{Cancel, Drive, DriveKind, Entry, History, Sort, SortKey, format};
use findex_ops::{Job, JobKind, Queue, Status};
use findex_ui::*;
use slint::platform::Key;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

mod link;

use crate::model::ListingModel;
use crate::settings::Settings;
use crate::update::{self, Installed, Offer};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app on the UI thread. Re-entrant calls are dropped instead of panicking.
pub fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut g) => {
            if let Some(app) = g.as_mut() {
                f(app)
            }
        }
        Err(_) => eprintln!("findex: re-entrant UI callback ignored"),
    })
}

/// Sends work from any thread to the UI thread.
pub fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

enum Listing {
    Chunk(Vec<Entry>),
    Full(Vec<Entry>, Vec<u32>),
    Order(Vec<u32>),
    Hits(Vec<(Entry, u32)>),
    Error(String),
    Done,
}

#[derive(Clone)]
enum NavTarget {
    None,
    Path(PathBuf),
    ThisPc,
    Drive(usize),
}

enum Undo {
    Rename { from: PathBuf, to: PathBuf },
    TrashCreated(Vec<PathBuf>),
    MoveBack(Vec<(PathBuf, PathBuf)>),
    Restore(Vec<PathBuf>),
}

pub struct Tab {
    /// `None` shows This PC.
    pub path: Option<PathBuf>,
    history: History,
    model: Rc<ListingModel>,
    cancel: Cancel,
    sort: Sort,
    search: Option<String>,
    generation: u64,
    loading: bool,
    error: String,
    stale: bool,
    pending_select: Vec<OsString>,
    rename_after: bool,
}

#[derive(Default)]
struct JobStats {
    last_bytes: u64,
    last_time: Option<Instant>,
    speed: f64,
    /// Speed shown as text; refreshed every 0.5 s so the digits do not flicker.
    shown_speed: f64,
    shown_at: Option<Instant>,
    /// (progress 0..1, bytes per second), sampled each tick.
    samples: Vec<(f64, f64)>,
}

pub struct App {
    ui: AppWindow,
    transfer: TransferWindow,
    props: PropertiesWindow,
    /// Paths the Properties window acts on.
    props_targets: Vec<PathBuf>,
    props_thumb: Option<Image>,
    /// Bumped on every open and close, so late worker results are dropped.
    props_gen: u64,
    props_cancel: Cancel,
    /// Attribute states as first read (read-only, hidden); unchanged ones are not applied.
    props_init: (i32, i32),
    /// The transfer window was opened automatically (it then closes itself when everything is done).
    transfer_auto: bool,
    tabs: Vec<Tab>,
    active: usize,
    clipboard: Option<(Vec<PathBuf>, bool)>,
    queue: Queue,
    thumbs: findex_thumbs::Service,
    /// Drives as shown (after "Hide empty drives"); `all_drives` keeps the full list.
    drives: Vec<Drive>,
    all_drives: Vec<Drive>,
    options: OptionsWindow,
    nav_targets: Vec<NavTarget>,
    places: Vec<(String, PathBuf, &'static str)>,
    settings: Settings,
    next_generation: u64,
    search_timer: Timer,
    toast_timer: Timer,
    jobs_timer: Timer,
    stats: HashMap<u64, JobStats>,
    undo: Option<Undo>,
    type_ahead: (String, Instant),
    /// Recently closed tabs (path, sort), newest last, for Ctrl+Shift+T.
    closed_tabs: Vec<(Option<PathBuf>, Sort)>,
    tabs_model: Rc<VecModel<TabData>>,
    jobs_model: Rc<VecModel<JobData>>,
    default_fm: bool,
    /// Key of the drive being renamed in the dialog.
    drive_target: Option<String>,
    /// The last update check's answer; installing takes it from here.
    update_offer: Option<Offer>,
    update_checking: bool,
    update_busy: bool,
    /// The updated AppImage to start once this process has saved its session.
    relaunch: Option<PathBuf>,
    /// Findex Link: paired phones, server and discovery.
    link: link::LinkState,
}

/// Stable identity of a drive: block device, else mount path.
fn drive_key(d: &Drive) -> String {
    d.device
        .clone()
        .unwrap_or_else(|| d.path.display().to_string())
}

pub fn start(ui: AppWindow, start: Option<PathBuf>) {
    let settings = Settings::load();
    let thumbs = findex_thumbs::Service::new(
        std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .clamp(2, 4),
        findex_platform::thumbnail_cache_dir(),
        |generation, key, t| post(move |app| app.thumb_ready(generation, key as u32, t)),
    );
    let queue = Queue::new(|job: &Job| {
        let id = job.id;
        post(move |app| app.job_finished(id));
    });
    let tabs_model = Rc::new(VecModel::default());
    let jobs_model = Rc::new(VecModel::default());
    ui.set_tabs(ModelRc::from(tabs_model.clone()));
    ui.set_jobs(ModelRc::from(jobs_model.clone()));
    let options = OptionsWindow::new().expect("create options window");
    options.on_apply(|o| with_app(|app| app.apply_options(&o)));
    options.on_restore_defaults(|| {
        with_app(|app| {
            let theme = app.settings.theme;
            let mut d = Settings::default().to_options();
            d.theme = theme;
            app.options.set_opts(d);
        })
    });
    options.on_unhide_drive(|i| with_app(|app| app.unhide_drive(i as usize)));
    options.on_toggle_default_fm(|| with_app(|app| app.toggle_default_fm()));
    options.on_close_requested(|| with_app(|app| drop(app.options.hide())));
    options.on_check_updates(|channel| with_app(|app| app.check_updates(channel, false)));
    options.on_install_update(|| with_app(|app| app.install_update()));
    options.set_app_version(update::VERSION.into());
    options.set_repo_url(update::REPO_URL.into());
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    };
    options.set_platform(format!("{os} ({})", std::env::consts::ARCH).into());
    options.on_open_url(|url| {
        let url = url.to_string();
        std::thread::spawn(move || {
            if let Err(e) = findex_platform::open_default(Path::new(&url)) {
                post(move |app| app.toast(format!("Could not open {url}: {e}"), false));
            }
        });
    });
    options.set_self_install(update::self_install());
    options.set_update_status(
        if update::is_dev() {
            "This is a development build. It updates by rebuilding from source."
        } else {
            "Check whether a newer version is out."
        }
        .into(),
    );
    let props = PropertiesWindow::new().expect("create properties window");
    props.on_close_requested(|| with_app(|app| app.hide_props()));
    props.on_apply(|n, ro, h| with_app(|app| app.props_apply(n.to_string(), ro, h)));
    let transfer = TransferWindow::new().expect("create transfer window");
    transfer.set_jobs(ModelRc::from(jobs_model.clone()));
    transfer.on_pause(|id| with_app(|app| app.job_pause(id as u64)));
    transfer.on_cancel(|id| with_app(|app| app.job_cancel(id as u64)));
    transfer.on_close_requested(|| with_app(|app| app.hide_transfers()));
    // Closing the main window quits, even while the transfer window is open.
    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    let mut app = App {
        ui: ui.clone_strong(),
        transfer,
        props,
        props_targets: Vec::new(),
        props_thumb: None,
        props_gen: 0,
        props_cancel: Cancel::default(),
        props_init: (2, 2),
        transfer_auto: false,
        tabs: Vec::new(),
        active: 0,
        clipboard: None,
        queue,
        thumbs,
        drives: Vec::new(),
        all_drives: Vec::new(),
        drive_target: None,
        options,
        nav_targets: Vec::new(),
        places: findex_platform::places(),
        settings,
        next_generation: 1,
        search_timer: Timer::default(),
        toast_timer: Timer::default(),
        jobs_timer: Timer::default(),
        stats: HashMap::new(),
        undo: None,
        type_ahead: (String::new(), Instant::now()),
        closed_tabs: Vec::new(),
        tabs_model,
        jobs_model,
        default_fm: false,
        update_offer: None,
        update_checking: false,
        update_busy: false,
        relaunch: None,
        link: link::LinkState::default(),
    };
    app.apply_settings();
    let restored: Vec<PathBuf> = if start.is_none() && app.settings.restore_tabs {
        app.settings
            .tabs
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    if restored.is_empty() {
        let (path, select) = match start {
            Some(p) if p.is_file() => (
                p.parent().map(Path::to_path_buf),
                p.file_name().map(|n| vec![n.to_os_string()]),
            ),
            Some(p) => (Some(p), None),
            None => (app.start_location(), None),
        };
        app.new_tab(path);
        if let Some(names) = select {
            app.tabs[0].pending_select = names;
        }
    } else {
        for p in restored {
            app.new_tab(Some(p));
        }
        app.switch_tab(0);
    }
    APP.with(|c| *c.borrow_mut() = Some(app));
    wire(&ui);
    wire_mouse_buttons(&ui);
    spawn_drive_watcher();
    with_app(|app| app.link_start());
    std::thread::spawn(|| {
        let is_default = findex_platform::is_default_file_manager();
        post(move |app| app.set_default_fm_state(is_default));
    });
    with_app(|app| {
        if app.settings.update_check && update::auto_check_allowed() {
            let channel = app.settings.update_channel;
            // Later than the first frame, so startup never waits on the network.
            Timer::single_shot(Duration::from_secs(3), move || {
                with_app(|app| app.check_updates(channel, true))
            });
        }
    });
}

/// Mouse side buttons: back and forward through the tab's history, like Explorer.
fn wire_mouse_buttons(ui: &AppWindow) {
    use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    ui.window().on_winit_window_event(|_, event| {
        if let WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: button @ (MouseButton::Back | MouseButton::Forward),
            ..
        } = event
        {
            let back = *button == MouseButton::Back;
            post(move |app| if back { app.back() } else { app.forward() });
            return EventResult::PreventDefault;
        }
        EventResult::Propagate
    });
}

/// Refreshes the drive list on a worker every few seconds (mounts, unplugs, free space).
fn spawn_drive_watcher() {
    std::thread::Builder::new()
        .name("findex-drives".into())
        .spawn(|| {
            let mut last = String::new();
            loop {
                let drives = findex_platform::drives();
                let sig = drives
                    .iter()
                    .map(|d| {
                        format!(
                            "{}|{}|{:?}|{}",
                            d.label,
                            d.path.display(),
                            d.kind,
                            d.free >> 24
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(";");
                if sig != last {
                    last = sig;
                    post(move |app| app.set_drives(drives));
                }
                std::thread::sleep(Duration::from_secs(4));
            }
        })
        .expect("spawn drive watcher");
}

fn wire(ui: &AppWindow) {
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {
            ui.$setter(move |$($arg),*| with_app(|$app| { $body; }));
        };
    }
    on!(on_navigate, |app, p| app
        .navigate(Some(PathBuf::from(p.as_str())), true));
    on!(on_path_submitted, |app, p| app
        .path_submitted(p.to_string()));
    on!(on_go_back, |app| app.back());
    on!(on_go_forward, |app| app.forward());
    on!(on_go_up, |app| app.up());
    on!(on_refresh, |app| app.reload());
    on!(on_new_tab, |app| {
        let p = app.start_location();
        app.new_tab(p)
    });
    on!(on_close_tab, |app, i| app.close_tab(i as usize));
    on!(on_reorder_tab, |app, from, to| app
        .reorder_tab(from as usize, to as usize));
    on!(on_select_tab, |app, i| app.switch_tab(i as usize));
    on!(on_item_down, |app, i, ctrl, shift, right| app
        .item_down(i as usize, ctrl, shift, right));
    on!(on_item_activated, |app, i| {
        // With single-click opening, the press already opened the item.
        if !app.settings.single_click {
            app.activate(i as usize)
        }
    });
    on!(on_background_down, |app, right, additive| {
        // Right-click on empty space clears; a left press starts a rubber band (Ctrl/Shift keeps the selection).
        app.tab().model.band_begin(additive && !right);
        app.update_status();
    });
    on!(on_band, |app, r0, r1, c0, c1, n| {
        app.tab().model.band_select((r0, r1), (c0, c1), n);
        app.update_status();
    });
    on!(on_sort_by, |app, k, desc| app
        .set_sort(SortKey::from_index(k), desc));
    on!(on_set_view, |app, m| app.set_view(m));
    on!(on_cut, |app| app.clip(true));
    on!(on_copy, |app| app.clip(false));
    on!(on_paste, |app| app.paste());
    on!(on_rename_start, |app| app.rename_start());
    on!(on_rename_commit, |app, name| app
        .rename_commit(name.to_string()));
    on!(on_delete_items, |app, permanent| app
        .delete(permanent, true));
    on!(on_delete_confirmed, |app| app.delete(true, false));
    on!(on_new_folder, |app| app.create_new(true));
    on!(on_new_file, |app| app.create_new(false));
    on!(on_copy_path, |app| app.copy_path());
    on!(on_open_in_new_tab, |app| app.open_selected_in_new_tab());
    on!(on_restore_items, |app| app.restore_selected());
    on!(on_select_all, |app| {
        app.tab().model.select_all();
        app.update_status();
    });
    on!(on_search_edited, |app, t| app.search_edited(t.to_string()));
    on!(on_search_submitted, |app, t| app
        .start_search(t.to_string()));
    on!(on_search_cleared, |app| app.clear_search());
    on!(on_nav_activate, |app, i| app.nav_activate(i as usize));
    on!(on_drive_open, |app, net, i| app.drive_open(net, i as usize));
    on!(on_drive_eject, |app, i| app.drive_eject(i as usize));
    on!(on_drive_rename_start, |app, net, i| app
        .drive_rename_start(net, i as usize));
    on!(on_drive_rename_commit, |app, name| app
        .drive_rename_commit(name.to_string()));
    on!(on_drive_hide, |app, net, i| app.drive_hide(net, i as usize));
    on!(on_job_pause, |app, id| app.job_pause(id as u64));
    on!(on_job_cancel, |app, id| app.job_cancel(id as u64));
    on!(on_clear_finished, |app| {
        app.queue.clear_finished();
        app.update_jobs();
    });
    on!(on_set_theme, |app, m| app.set_theme(m));
    on!(on_set_transparency, |app, b| {
        app.settings.transparency = b;
        app.apply_settings();
    });
    on!(on_set_show_hidden, |app, b| app.set_show_hidden(b));
    on!(on_set_compact, |app, b| {
        app.settings.compact = b;
        app.apply_settings();
    });
    on!(on_toggle_default_fm, |app| app.toggle_default_fm());
    on!(on_undo, |app| app.do_undo());
    on!(on_window_changed, |_app| {});
    on!(on_show_transfers, |app| app.show_transfers(false));
    on!(on_show_options, |app| app.show_options());
    on!(on_update_install, |app| app.install_update());
    on!(on_update_later, |app| app.update_later());
    on!(on_show_properties, |app| app.show_properties());
    on!(on_phone_drive_open, |app, i| app
        .phone_drive_open(i as usize));
    on!(on_pair_phone, |app| app.pair_phone());
    on!(on_pair_submit, |app, a| app.pair_submit(a.to_string()));
    on!(on_link_answer, |app, yes| app.link_answer(yes));
    on!(on_send_to_phone, |app| app.send_to_phone());
    on!(on_unpair_confirmed, |app| app.unpair_confirmed());
    on!(on_send_to_phone_named, |app, n| app
        .send_to_phone_named(n.to_string()));
    ui.on_key(move |text, ctrl, shift, alt, typing| {
        let mut handled = false;
        with_app(|app| handled = app.key(&text, ctrl, shift, alt, typing));
        handled
    });
}

fn key_is(text: &str, key: Key) -> bool {
    let mut c = text.chars();
    c.next() == Some(char::from(key)) && c.next().is_none()
}

impl App {
    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    // ---------------- settings and theme ----------------

    fn apply_settings(&mut self) {
        let s = self.settings.clone();
        for theme in [
            self.ui.global::<Theme>(),
            self.transfer.global::<Theme>(),
            self.props.global::<Theme>(),
            self.options.global::<Theme>(),
        ] {
            theme.set_transparency(s.transparency);
            theme.set_compact(s.compact);
        }
        self.ui.set_theme_mode(s.theme);
        self.ui.set_show_hidden(s.show_hidden);
        self.ui.set_view_mode(s.view);
        self.ui.set_details_pane(s.details_pane);
        self.ui.set_check_boxes(s.check_boxes);
        self.ui.set_status_bar(s.show_status_bar);
        if s.theme != 0 {
            self.set_color_scheme(s.theme == 2);
        }
        let want = self.want_thumbs();
        for t in &self.tabs {
            t.model.set_display(want, s.hide_extensions);
            t.model.set_show_hidden(s.show_hidden);
        }
        // Newly shown hidden items must take their sorted place.
        for i in 0..self.tabs.len() {
            self.resort(i);
        }
        // Before the first tab exists there is nothing on screen to refresh.
        if !self.tabs.is_empty() {
            let all = self.all_drives.clone();
            self.set_drives(all);
            self.refresh_ui();
        }
        s.save();
    }

    fn want_thumbs(&self) -> bool {
        self.settings.view != 5 && !self.settings.never_thumbnails
    }

    /// Where Findex and new tabs open ("Open Findex to").
    fn start_location(&self) -> Option<PathBuf> {
        if self.settings.open_to_this_pc {
            None
        } else {
            dirs::home_dir()
        }
    }

    // ---------------- options window ----------------

    fn show_options(&mut self) {
        self.options.set_opts(self.settings.to_options());
        self.options.set_page(0);
        self.sync_hidden_drives();
        self.set_default_fm_state(self.default_fm);
        if !self.options.window().is_visible() {
            let _ = self.options.show();
            self.options.invoke_focus_window();
            Timer::single_shot(Duration::ZERO, || {
                with_app(|app| apply_effects(app.options.window(), &app.options.global::<Theme>()))
            });
        }
    }

    fn apply_options(&mut self, o: &Options) {
        let old_theme = self.settings.theme;
        let old_link = self.settings.link_enabled;
        self.settings.set_options(o);
        if self.settings.link_enabled != old_link {
            self.link_start();
        }
        let theme = self.settings.theme;
        self.apply_settings();
        if theme == 0 && old_theme != 0 {
            self.set_theme(0);
        }
    }

    pub fn options_window(&self) -> &OptionsWindow {
        &self.options
    }

    /// Remembers open folder tabs for "Restore previous folder tabs at startup". Called at exit.
    pub fn save_session(&mut self) {
        self.settings.tabs = self.tabs.iter().filter_map(|t| t.path.clone()).collect();
        self.settings.save_now();
    }

    fn set_color_scheme(&self, dark: bool) {
        let scheme = if dark {
            slint::language::ColorScheme::Dark
        } else {
            slint::language::ColorScheme::Light
        };
        self.ui.global::<Palette>().set_color_scheme(scheme);
        self.transfer.global::<Palette>().set_color_scheme(scheme);
        self.props.global::<Palette>().set_color_scheme(scheme);
        self.options.global::<Palette>().set_color_scheme(scheme);
    }

    fn set_theme(&mut self, mode: i32) {
        self.settings.theme = mode;
        self.apply_settings();
        if mode == 0 {
            std::thread::spawn(|| {
                let dark = findex_platform::system_prefers_dark();
                post(move |app| app.set_color_scheme(dark));
            });
        }
    }

    /// Called once the native window exists: ask the OS for a Mica backdrop (Windows 11).
    pub fn apply_window_effects(&mut self) {
        apply_effects(self.ui.window(), &self.ui.global::<Theme>());
    }

    // ---------------- properties window ----------------

    /// Opens Properties for the selection, or for the open folder when nothing is selected.
    fn show_properties(&mut self) {
        if self.tab().path.as_deref().is_some_and(link::is_remote) {
            return self.toast(
                "Properties aren't available for phone files yet.".into(),
                false,
            );
        }
        let sel = self.tab().model.selected_entries();
        let (targets, can_edit) = if sel.is_empty() {
            let Some(p) = self.tab().path.clone() else {
                return;
            };
            (vec![p], false)
        } else {
            let paths = sel.iter().map(|&i| self.tab().model.path_of(i)).collect();
            (paths, self.tab().search.is_none())
        };
        let d = self.ui.get_details();
        self.props_thumb = (sel.len() == 1 && d.has_thumb).then_some(d.thumb);
        self.props.set_can_edit(can_edit);
        self.open_props(targets);
        if !self.props.window().is_visible() {
            let _ = self.props.show();
            self.props.invoke_focus_window();
            Timer::single_shot(Duration::ZERO, || {
                with_app(|app| apply_effects(app.props.window(), &app.props.global::<Theme>()))
            });
        }
    }

    /// Starts gathering properties for `targets` on a worker; stale results are dropped.
    fn open_props(&mut self, targets: Vec<PathBuf>) {
        self.props_cancel.cancel();
        self.props_cancel = Cancel::default();
        self.props_gen += 1;
        self.props_targets = targets.clone();
        self.props.set_data(PropsData::default());
        self.props.set_dirty(false);
        let (generation, cancel) = (self.props_gen, self.props_cancel.clone());
        std::thread::spawn(move || {
            gather_props(&targets, &cancel, &mut |info| {
                post(move |app| app.props_update(generation, info))
            })
        });
    }

    fn props_update(&mut self, generation: u64, info: PropsInfo) {
        if generation != self.props_gen {
            return;
        }
        let first = self.props.get_data().count == 0;
        let mut d = PropsData {
            count: info.count,
            name: info.name.clone().into(),
            kind: info.kind,
            is_dir: info.is_dir,
            type_name: info.type_name.into(),
            opens_with: info.opens_with.into(),
            location: info.location.into(),
            size: info.size.into(),
            size_on_disk: info.size_on_disk.into(),
            contains: info.contains.into(),
            created: info.created.into(),
            modified: info.modified.into(),
            accessed: info.accessed.into(),
            ..Default::default()
        };
        if let Some(t) = &self.props_thumb {
            d.thumb = t.clone();
            d.has_thumb = true;
        }
        self.props.set_data(d);
        // Reset staged edits once per open; later updates only refresh the counts.
        if first {
            self.props.set_name_text(info.name.into());
            self.props.set_readonly(info.readonly);
            self.props.set_hidden(info.hidden);
            self.props_init = (info.readonly, info.hidden);
        }
    }

    fn props_apply(&mut self, name: String, readonly: i32, hidden: i32) {
        // 2 means "leave as is": also for attributes the user did not touch, so a typed name wins.
        let readonly = if readonly == self.props_init.0 {
            2
        } else {
            readonly
        };
        let hidden = if hidden == self.props_init.1 {
            2
        } else {
            hidden
        };
        let targets = self.props_targets.clone();
        let name = name.trim().to_string();
        let rename = targets.len() == 1
            && targets[0]
                .file_name()
                .is_some_and(|n| n.to_string_lossy() != name);
        if rename && !valid_name(&name) {
            return self.toast(
                "A file name can't be empty or contain a path separator.".into(),
                false,
            );
        }
        self.props.set_dirty(false);
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut err = None;
            for mut p in targets {
                let r = (|| {
                    if rename {
                        let to = p.with_file_name(&name);
                        if std::fs::symlink_metadata(&to).is_ok() {
                            return Err(std::io::Error::other(format!(
                                "There is already a file named '{name}' here."
                            )));
                        }
                        std::fs::rename(&p, &to)?;
                        p = to;
                    }
                    if hidden != 2 {
                        p = findex_platform::set_hidden(&p, hidden == 1)?;
                    }
                    if readonly != 2 {
                        findex_platform::set_readonly(&p, readonly == 1)?;
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    err = Some(e);
                }
                out.push(p);
            }
            post(move |app| {
                if let Some(e) = err {
                    app.toast(format!("{e}"), false);
                }
                app.tab_mut().pending_select = out
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_os_string()))
                    .collect();
                app.load();
                if app.props.window().is_visible() {
                    app.open_props(out);
                }
            });
        });
    }

    fn hide_props(&mut self) {
        self.props_cancel.cancel();
        self.props_gen += 1;
        let _ = self.props.hide();
    }

    pub fn properties_window(&self) -> &PropertiesWindow {
        &self.props
    }

    // ---------------- transfer window ----------------

    fn show_transfers(&mut self, auto: bool) {
        self.transfer_auto = auto;
        if self.transfer.window().is_visible() {
            return;
        }
        let _ = self.transfer.show();
        Timer::single_shot(Duration::ZERO, || {
            with_app(|app| apply_effects(app.transfer.window(), &app.transfer.global::<Theme>()))
        });
    }

    fn hide_transfers(&mut self) {
        let _ = self.transfer.hide();
        self.queue.clear_finished();
        self.update_jobs();
    }

    pub fn transfer_window(&self) -> &TransferWindow {
        &self.transfer
    }

    // ---------------- tabs ----------------

    fn new_tab(&mut self, path: Option<PathBuf>) {
        let model = Rc::new(ListingModel::new(self.thumbs.clone()));
        model.set_display(self.want_thumbs(), self.settings.hide_extensions);
        self.tabs.push(Tab {
            path: None,
            history: History::default(),
            model,
            cancel: Cancel::default(),
            sort: Sort::default(),
            search: None,
            generation: 0,
            loading: false,
            error: String::new(),
            stale: false,
            pending_select: Vec::new(),
            rename_after: false,
        });
        self.active = self.tabs.len() - 1;
        self.bind_tab();
        self.navigate(path, false);
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() == 1 {
            let _ = self.ui.window().hide();
            return;
        }
        self.tabs[i].cancel.cancel();
        let closed = self.tabs.remove(i);
        self.closed_tabs.push((closed.path, closed.sort));
        if self.closed_tabs.len() > 10 {
            self.closed_tabs.remove(0);
        }
        if self.active >= self.tabs.len() || self.active > i {
            self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
        }
        self.bind_tab();
    }

    /// A request from another Findex launch: open the folder in a new tab and bring the window forward.
    pub fn open_external(&mut self, req: &str) {
        if !req.is_empty() {
            // Paths from the socket, `file://` URIs from FileManager1.
            let p = crate::parse_location(req);
            match p.parent().filter(|_| p.is_file()) {
                // "Show in folder": open the parent and select the file.
                Some(parent) => {
                    self.new_tab(Some(parent.to_path_buf()));
                    self.tab_mut().pending_select = p
                        .file_name()
                        .map(|n| vec![n.to_os_string()])
                        .unwrap_or_default();
                }
                None => self.new_tab(Some(p)),
            }
        }
        let w = self.ui.window();
        w.set_minimized(false);
        let _ = w.show();
    }

    fn reorder_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        // The active tab keeps its identity, not its index.
        self.active = match self.active {
            a if a == from => to,
            a if from < a && a <= to => a - 1,
            a if to <= a && a < from => a + 1,
            a => a,
        };
        self.sync_tabs();
    }

    fn reopen_closed_tab(&mut self) {
        if let Some((path, sort)) = self.closed_tabs.pop() {
            self.new_tab(path);
            self.tab_mut().sort = sort;
        }
    }

    fn duplicate_tab(&mut self, i: usize) {
        let (path, sort) = (self.tabs[i].path.clone(), self.tabs[i].sort);
        self.new_tab(path);
        self.tab_mut().sort = sort;
    }

    fn switch_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.active = i;
        self.bind_tab();
        if self.tab().stale {
            self.reload();
        }
    }

    /// Points the UI at the active tab's model and state.
    fn bind_tab(&mut self) {
        let generation = self.tab().generation;
        self.thumbs.set_generation(generation);
        self.ui.set_rows(ModelRc::from(
            self.tab().model.clone() as Rc<dyn Model<Data = FileRow>>
        ));
        self.ui.set_active_tab(self.active as i32);
        self.refresh_ui();
    }

    fn sync_tabs(&self) {
        let data: Vec<TabData> = self
            .tabs
            .iter()
            .map(|t| TabData {
                title: tab_title(t, self.settings.full_path_title).into(),
                tooltip: t
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "This PC".into())
                    .into(),
            })
            .collect();
        if self.tabs_model.row_count() == data.len() {
            for (i, d) in data.into_iter().enumerate() {
                if self.tabs_model.row_data(i).as_ref() != Some(&d) {
                    self.tabs_model.set_row_data(i, d);
                }
            }
        } else {
            self.tabs_model.set_vec(data);
        }
        self.ui.set_active_tab(self.active as i32);
    }

    // ---------------- navigation ----------------

    fn navigate(&mut self, path: Option<PathBuf>, push: bool) {
        let tab = &mut self.tabs[self.active];
        if push && tab.path != path {
            let from = tab.path.clone().unwrap_or_default();
            tab.history.push(from);
        }
        tab.path = path;
        tab.search = None;
        self.ui.set_search_text(SharedString::new());
        self.load();
    }

    /// (Re)lists the active tab's folder on a worker. Cancels any previous listing.
    fn load(&mut self) {
        let generation = self.next_generation;
        self.next_generation += 1;
        let show_hidden = self.settings.show_hidden;
        let tab = &mut self.tabs[self.active];
        tab.cancel.cancel();
        tab.cancel = Cancel::default();
        tab.generation = generation;
        tab.error.clear();
        tab.stale = false;
        self.thumbs.set_generation(generation);
        let Some(path) = tab.path.clone() else {
            tab.loading = false;
            tab.model.reset(PathBuf::new(), generation, show_hidden);
            self.refresh_ui();
            // This PC shows phone storage: ask the phones again.
            self.link_refresh_infos();
            return;
        };
        tab.loading = true;
        tab.model.reset(path.clone(), generation, show_hidden);
        let (cancel, sort) = (tab.cancel.clone(), tab.sort);
        if link::is_remote(&path) {
            let remote = self.remote_list(&path);
            std::thread::spawn(move || {
                let r = remote
                    .ok_or_else(|| "Findex Link is off. Turn on Phones in Options.".to_string())
                    .and_then(|r| r.list());
                if cancel.is_cancelled() {
                    return;
                }
                post(move |app| {
                    let msg = match r {
                        Ok(all) => {
                            let order = findex_core::sort_order(&all, sort);
                            Listing::Full(all, order)
                        }
                        Err(e) => Listing::Error(e),
                    };
                    app.on_listing(generation, msg)
                });
            });
            self.refresh_ui();
            return;
        }
        std::thread::Builder::new()
            .name("findex-list".into())
            .spawn(move || {
                let send = move |msg: Listing| post(move |app| app.on_listing(generation, msg));
                let start = Instant::now();
                let mut all: Vec<Entry> = Vec::new();
                let mut sent = 0;
                // Small folders arrive in one sorted batch (no flicker); big ones stream after 80 ms.
                let r = findex_platform::list_dir(&path, &cancel, &mut |chunk| {
                    all.extend(chunk);
                    if start.elapsed() > Duration::from_millis(80) {
                        send(Listing::Chunk(all[sent..].to_vec()));
                        sent = all.len();
                    }
                });
                if cancel.is_cancelled() {
                    return;
                }
                match r {
                    Err(e) => send(Listing::Error(format!("{} ({})", e, path.display()))),
                    Ok(()) => {
                        let order = findex_core::sort_order(&all, sort);
                        if sent == 0 {
                            send(Listing::Full(all, order));
                        } else {
                            send(Listing::Chunk(all.split_off(sent)));
                            send(Listing::Order(order));
                        }
                    }
                }
            })
            .expect("spawn listing worker");
        self.refresh_ui();
    }

    fn on_listing(&mut self, generation: u64, msg: Listing) {
        let Some(ti) = self.tabs.iter().position(|t| t.generation == generation) else {
            return;
        };
        let is_active = ti == self.active;
        let tab = &mut self.tabs[ti];
        let finished = matches!(
            msg,
            Listing::Full(..) | Listing::Order(_) | Listing::Error(_) | Listing::Done
        );
        match msg {
            Listing::Chunk(c) => tab.model.append(c),
            Listing::Full(entries, order) => tab.model.set_all(entries, order),
            Listing::Order(order) => tab.model.set_order(order),
            Listing::Hits(h) => tab.model.add_hits(h),
            Listing::Error(e) => tab.error = e,
            Listing::Done => {}
        }
        if finished {
            tab.loading = false;
            if !tab.pending_select.is_empty() {
                let names = std::mem::take(&mut tab.pending_select);
                if let Some(r) = tab.model.select_names(&names)
                    && is_active
                {
                    self.ui.set_focus_row(r as i32);
                }
                if std::mem::take(&mut tab.rename_after) && is_active {
                    self.rename_start();
                }
            }
        }
        if is_active {
            self.update_status();
            self.ui.set_loading(self.tab().loading);
            self.ui.set_error_text(self.tab().error.clone().into());
        }
    }

    fn reload(&mut self) {
        match self.tab().search.clone() {
            Some(q) => self.start_search(q),
            None => self.load(),
        }
    }

    fn back(&mut self) {
        let tab = self.tab_mut();
        let cur = tab.path.clone().unwrap_or_default();
        if let Some(p) = tab.history.back(cur) {
            let p = (!p.as_os_str().is_empty()).then_some(p);
            self.navigate(p, false);
        }
    }

    fn forward(&mut self) {
        let tab = self.tab_mut();
        let cur = tab.path.clone().unwrap_or_default();
        if let Some(p) = tab.history.forward(cur) {
            let p = (!p.as_os_str().is_empty()).then_some(p);
            self.navigate(p, false);
        }
    }

    fn up(&mut self) {
        if let Some(parent) = self
            .tab()
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
        {
            let child = self
                .tab()
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_os_string());
            self.navigate(Some(parent), true);
            if let Some(c) = child {
                self.tab_mut().pending_select = vec![c];
            }
        } else if self.tab().path.is_some() {
            self.navigate(None, true);
        }
    }

    fn path_submitted(&mut self, text: String) {
        let t = text.trim();
        if t.is_empty() {
            return; // Escape: keep the current folder
        }
        if t.eq_ignore_ascii_case("this pc") {
            return self.navigate(None, true);
        }
        let p = crate::parse_location(t);
        if p.is_dir() {
            self.navigate(Some(p), true);
        } else if p.is_file() {
            let _ = findex_platform::open_default(&p);
        } else {
            self.toast(
                format!("Can't find '{t}'. Check the spelling and try again."),
                false,
            );
        }
    }

    // ---------------- UI sync ----------------

    fn refresh_ui(&mut self) {
        self.sync_tabs();
        let tab = self.tab();
        let ui = &self.ui;
        ui.set_this_pc(tab.path.is_none());
        ui.set_phones_root(tab.path.as_deref() == Some(Path::new(link::PHONES)));
        ui.set_in_phone(tab.path.as_deref().is_some_and(link::is_remote));
        ui.set_in_trash(tab.path.is_some() && tab.path == findex_platform::trash_dir());
        ui.set_loading(tab.loading);
        ui.set_error_text(tab.error.clone().into());
        ui.set_search_mode(tab.search.is_some());
        ui.set_can_back(tab.history.can_back());
        ui.set_can_forward(tab.history.can_forward());
        ui.set_can_up(tab.path.is_some());
        ui.set_sort_key(tab.sort.key.index());
        ui.set_sort_desc(tab.sort.descending);
        ui.set_folder_name(tab_title(tab, false).into());
        ui.set_path_text(
            tab.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "This PC".into())
                .into(),
        );
        ui.set_crumbs(ModelRc::new(VecModel::from(crumbs(tab.path.as_deref()))));
        ui.set_can_paste(self.clipboard.is_some());
        self.sync_nav();
        self.update_status();
    }

    fn update_status(&mut self) {
        let tab = self.tab();
        let model = &tab.model;
        let n = model.len();
        let sel = model.selected_entries();
        let entries = model.entries();
        let mut status = if tab.path.is_none() && tab.search.is_none() {
            format!("{} items", self.drives.len())
        } else {
            format!("{n} item{}", if n == 1 { "" } else { "s" })
        };
        if tab.search.is_some() && tab.loading {
            status.push_str("  ·  Searching…");
        }
        let bytes: u64 = sel.iter().map(|&i| entries[i as usize].size).sum();
        let any_files = sel.iter().any(|&i| !entries[i as usize].is_dir());
        let selection = match sel.len() {
            0 => String::new(),
            k => format!(
                "{k} item{} selected{}",
                if k == 1 { "" } else { "s" },
                if any_files {
                    format!("  {}", format::size(bytes))
                } else {
                    String::new()
                }
            ),
        };
        self.ui.set_status_text(status.into());
        self.ui.set_selection_text(selection.into());
        self.ui.set_selected_count(sel.len() as i32);
        self.ui
            .set_focus_row(model.focus().map_or(-1, |f| f as i32));

        // Details pane
        let mut d = Details {
            count: sel.len() as i32,
            ..Default::default()
        };
        if let Some(&idx) = sel.first() {
            let e = &entries[idx as usize];
            let folder = model.inner.borrow().folder.clone();
            if sel.len() == 1 {
                d.name = e.name.to_string().into();
                d.kind = e.kind as i32;
                d.type_name = e.kind.label().into();
                d.size = if e.is_dir() {
                    SharedString::new()
                } else {
                    format::size(e.size).into()
                };
                d.date = format::date(e.mtime).into();
                d.location = e
                    .parent
                    .as_deref()
                    .unwrap_or(&folder)
                    .display()
                    .to_string()
                    .into();
                if let Some(img) = model.inner.borrow().thumbs.get(&idx) {
                    d.thumb = img.clone();
                    d.has_thumb = true;
                } else if findex_thumbs::supported(&e.name) {
                    self.thumbs
                        .request(tab.generation, idx as u64, e.path_in(&folder), e.mtime);
                }
            } else if any_files {
                d.size = format::size(bytes).into();
            }
        }
        self.ui.set_details(d);
    }

    fn sync_nav(&mut self) {
        let current = self.tab().path.clone();
        let mut items = Vec::new();
        let mut targets = Vec::new();
        let mut push = |items: &mut Vec<NavItem>,
                        label: &str,
                        icon: &str,
                        kind: i32,
                        cur: bool,
                        t: NavTarget| {
            items.push(NavItem {
                label: label.into(),
                icon: icon.into(),
                kind,
                current: cur,
            });
            targets.push(t);
        };
        let s = &self.settings;
        for (label, path, icon) in self.places.iter().filter(|_| s.nav_places) {
            push(
                &mut items,
                label,
                icon,
                0,
                current.as_ref() == Some(path),
                NavTarget::Path(path.clone()),
            );
        }
        if s.nav_places && (s.nav_this_pc || s.nav_network) {
            push(&mut items, "", "", 2, false, NavTarget::None);
        }
        if s.nav_this_pc {
            push(
                &mut items,
                "This PC",
                "pc",
                0,
                current.is_none(),
                NavTarget::ThisPc,
            );
        }
        // Mounted and unmounted devices first, then a separate "Network" group.
        for network in [false, true] {
            if (network && !s.nav_network) || (!network && !s.nav_this_pc) {
                continue;
            }
            if network && self.drives.iter().any(|d| d.kind == DriveKind::Network) {
                push(&mut items, "", "", 2, false, NavTarget::None);
                push(
                    &mut items,
                    "Network",
                    "network",
                    0,
                    false,
                    NavTarget::ThisPc,
                );
            }
            for (i, d) in self
                .drives
                .iter()
                .enumerate()
                .filter(|(_, d)| (d.kind == DriveKind::Network) == network)
            {
                let icon = match d.kind {
                    DriveKind::Removable => "usb",
                    DriveKind::Network => "network",
                    _ => "drive",
                };
                let cur = !d.path.as_os_str().is_empty() && current.as_ref() == Some(&d.path);
                push(&mut items, &d.label, icon, 1, cur, NavTarget::Drive(i));
            }
        }
        if self.link_on() {
            let phones = link::phones_root();
            push(&mut items, "", "", 2, false, NavTarget::None);
            push(
                &mut items,
                "Phones",
                "phone",
                0,
                current.as_ref() == Some(&phones),
                NavTarget::Path(phones.clone()),
            );
            for p in self.link_peer_names() {
                let path = phones.join(&p);
                let cur = current.as_ref() == Some(&path);
                push(&mut items, &p, "phone", 1, cur, NavTarget::Path(path));
            }
        }
        let s = &self.settings;
        if let Some(trash) = findex_platform::trash_dir().filter(|t| s.nav_trash && t.is_dir()) {
            push(&mut items, "", "", 2, false, NavTarget::None);
            push(
                &mut items,
                "Trash",
                "trash",
                0,
                current.as_ref() == Some(&trash),
                NavTarget::Path(trash),
            );
        }
        self.nav_targets = targets;
        self.ui.set_nav_items(ModelRc::new(VecModel::from(items)));
    }

    fn set_drives(&mut self, drives: Vec<Drive>) {
        let hide_empty = self.settings.hide_empty_drives;
        self.all_drives = drives.clone();
        let settings = &self.settings;
        self.drives = drives
            .into_iter()
            // "Hide empty drives": card readers and optical drives without media report no size.
            .filter(|d| {
                !(hide_empty
                    && d.total == 0
                    && matches!(d.kind, DriveKind::Removable | DriveKind::Optical))
                    && !settings
                        .hidden_drives
                        .iter()
                        .any(|(k, _)| *k == drive_key(d))
            })
            .map(|mut d| {
                // The title is the volume name only; the mount point goes to the detail line.
                if d.kind != DriveKind::Unmounted
                    && let Some((name, _)) =
                        d.label.rsplit_once(" (").filter(|_| d.label.ends_with(')'))
                {
                    d.label = name.to_string();
                }
                let key = drive_key(&d);
                if let Some((_, name)) = settings.drive_names.iter().find(|(k, _)| *k == key) {
                    d.label = name.clone();
                }
                d
            })
            .collect();
        let show_paths = self.settings.show_mount_paths;
        let to_data = |d: &Drive| {
            // Unmounted volumes have a size but no free-space figure: no usage bar, no low-space warning.
            let measured = d.total > 0 && d.kind != DriveKind::Unmounted;
            let usage = if measured {
                1.0 - d.free as f32 / d.total as f32
            } else {
                -1.0
            };
            DriveData {
                label: d.label.clone().into(),
                kind: match d.kind {
                    DriveKind::Fixed => 0,
                    DriveKind::Removable => 1,
                    DriveKind::Network => 2,
                    DriveKind::Optical => 3,
                    DriveKind::Unmounted => 4,
                },
                usage,
                detail: match d.kind {
                    DriveKind::Unmounted => format!("{} · Click to mount", format::size(d.total)),
                    _ => {
                        let space = (d.total > 0).then(|| {
                            format!("{} free of {}", format::size(d.free), format::size(d.total))
                        });
                        let path = (show_paths && d.kind != DriveKind::Network)
                            .then(|| d.path.display().to_string());
                        [space, path]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ")
                    }
                }
                .into(),
                low_space: measured && (d.free as f64) < d.total as f64 * 0.1,
                can_eject: d.kind == DriveKind::Removable && d.device.is_some(),
            }
        };
        let local: Vec<DriveData> = self
            .drives
            .iter()
            .filter(|d| d.kind != DriveKind::Network)
            .map(to_data)
            .collect();
        let net: Vec<DriveData> = self
            .drives
            .iter()
            .filter(|d| d.kind == DriveKind::Network)
            .map(to_data)
            .collect();
        self.ui.set_drives(ModelRc::new(VecModel::from(local)));
        self.ui
            .set_network_drives(ModelRc::new(VecModel::from(net)));
        self.sync_nav();
        self.update_status();
    }

    fn nav_activate(&mut self, i: usize) {
        match self.nav_targets.get(i).cloned() {
            Some(NavTarget::Path(p)) => self.navigate(Some(p), true),
            Some(NavTarget::ThisPc) => self.navigate(None, true),
            Some(NavTarget::Drive(d)) => self.open_drive_index(d),
            _ => {}
        }
    }

    fn drive_open(&mut self, network: bool, i: usize) {
        let idx = self
            .drives
            .iter()
            .enumerate()
            .filter(|(_, d)| (d.kind == DriveKind::Network) == network)
            .nth(i)
            .map(|(k, _)| k);
        if let Some(k) = idx {
            self.open_drive_index(k);
        }
    }

    fn open_drive_index(&mut self, k: usize) {
        let Some(d) = self.drives.get(k).cloned() else {
            return;
        };
        if d.kind != DriveKind::Unmounted {
            return self.navigate(Some(d.path), true);
        }
        let Some(dev) = d.device else { return };
        self.toast(format!("Mounting {}…", d.label), false);
        std::thread::spawn(move || {
            let r = findex_platform::mount(&dev);
            post(move |app| match r {
                Ok(p) => {
                    app.navigate(Some(p), true);
                    app.toast(String::new(), false);
                }
                Err(e) => app.toast(format!("Couldn't mount {dev}: {e}"), false),
            });
        });
    }

    fn drive_eject(&mut self, i: usize) {
        let Some(d) = self
            .drives
            .iter()
            .filter(|d| d.kind != DriveKind::Network)
            .nth(i)
            .cloned()
        else {
            return;
        };
        let Some(dev) = d.device else { return };
        let label = d.label;
        std::thread::spawn(move || {
            let r = findex_platform::eject(&dev);
            post(move |app| match r {
                Ok(()) => app.toast(format!("{label} can be safely removed."), false),
                Err(e) => app.toast(format!("Couldn't eject {label}: {e}"), false),
            });
        });
    }

    /// The drive shown at `i` in the local or network list.
    fn drive_at(&self, network: bool, i: usize) -> Option<Drive> {
        self.drives
            .iter()
            .filter(|d| (d.kind == DriveKind::Network) == network)
            .nth(i)
            .cloned()
    }

    fn drive_rename_start(&mut self, network: bool, i: usize) {
        let Some(d) = self.drive_at(network, i) else {
            return;
        };
        self.drive_target = Some(drive_key(&d));
        self.ui.invoke_show_drive_rename(d.label.into());
    }

    /// An empty name restores the original one. Names persist in settings.
    fn drive_rename_commit(&mut self, name: String) {
        let Some(key) = self.drive_target.take() else {
            return;
        };
        let name = name.trim().to_string();
        self.settings.drive_names.retain(|(k, _)| *k != key);
        if !name.is_empty() {
            self.settings.drive_names.push((key, name));
        }
        self.settings.save();
        let all = self.all_drives.clone();
        self.set_drives(all);
    }

    fn drive_hide(&mut self, network: bool, i: usize) {
        let Some(d) = self.drive_at(network, i) else {
            return;
        };
        self.settings.hidden_drives.push((drive_key(&d), d.label));
        self.settings.save();
        let all = self.all_drives.clone();
        self.set_drives(all);
        self.toast(
            "Drive hidden. Show it again in Options > Hidden drives.".into(),
            false,
        );
    }

    fn unhide_drive(&mut self, i: usize) {
        if i < self.settings.hidden_drives.len() {
            self.settings.hidden_drives.remove(i);
            self.settings.save();
            let all = self.all_drives.clone();
            self.set_drives(all);
            self.sync_hidden_drives();
        }
    }

    fn sync_hidden_drives(&self) {
        let names: Vec<SharedString> = self
            .settings
            .hidden_drives
            .iter()
            .map(|(_, n)| n.as_str().into())
            .collect();
        self.options
            .set_hidden_drives(ModelRc::new(VecModel::from(names)));
    }

    // ---------------- items ----------------

    fn item_down(&mut self, row: usize, ctrl: bool, shift: bool, right: bool) {
        let m = &self.tab().model;
        if right || ctrl || shift {
            m.click(row, ctrl, shift);
        } else {
            m.select_only(row);
        }
        self.update_status();
        if self.settings.single_click && !(right || ctrl || shift) {
            self.activate(row);
        }
    }

    fn activate(&mut self, row: usize) {
        let Some((idx, e)) = self.tab().model.entry_at(row) else {
            return;
        };
        let path = self.tab().model.path_of(idx);
        if e.is_dir() {
            if self.settings.folders_in_new_tab {
                self.new_tab(Some(path));
            } else {
                self.navigate(Some(path), true);
            }
        } else if link::is_remote(&path) {
            self.link_open(path);
        } else {
            std::thread::spawn(move || {
                if let Err(err) = findex_platform::open_default(&path) {
                    post(move |app| {
                        app.toast(format!("Couldn't open {}: {err}", path.display()), false)
                    });
                }
            });
        }
    }

    /// Restores the selected Trash items to where they were deleted from.
    fn restore_selected(&mut self) {
        let names: Vec<OsString> = self
            .tab()
            .model
            .selected_paths()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_os_string()))
            .collect();
        if names.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            let r = trash::os_limited::list().and_then(|items| {
                // The freedesktop item id is the `<name>.trashinfo` path next to `files/<name>`.
                let pick: Vec<_> = items
                    .into_iter()
                    .filter(|i| {
                        Path::new(&i.id)
                            .file_stem()
                            .is_some_and(|stem| names.iter().any(|n| n == stem))
                    })
                    .collect();
                let n = pick.len();
                trash::os_limited::restore_all(pick).map(|_| n)
            });
            post(move |app| {
                match r {
                    Ok(n) => app.toast(
                        format!("Restored {n} item{}", if n == 1 { "" } else { "s" }),
                        false,
                    ),
                    Err(e) => app.toast(format!("Couldn't restore: {e}"), false),
                }
                app.load();
            });
        });
    }

    fn open_selected_in_new_tab(&mut self) {
        let dirs: Vec<PathBuf> = self
            .tab()
            .model
            .selected_paths()
            .into_iter()
            .filter(|p| p.is_dir())
            .collect();
        for d in dirs {
            self.new_tab(Some(d));
        }
    }

    fn set_sort(&mut self, key: SortKey, descending: bool) {
        let tab = self.tab_mut();
        tab.sort = Sort { key, descending };
        if tab.search.is_some() {
            return self.refresh_ui();
        }
        self.resort(self.active);
        self.refresh_ui();
    }

    /// Re-sorts a tab's listing off the UI thread (search results keep their ranking).
    fn resort(&self, i: usize) {
        let tab = &self.tabs[i];
        if tab.search.is_some() {
            return tab.model.add_hits(Vec::new());
        }
        let (entries, generation) = tab.model.sort_key();
        let sort = tab.sort;
        std::thread::spawn(move || {
            let order = findex_core::sort_order(&entries, sort);
            post(move |app| app.on_listing(generation, Listing::Order(order)));
        });
    }

    fn set_view(&mut self, mode: i32) {
        self.settings.view = mode;
        self.ui.set_view_mode(mode);
        let (want, hide_ext) = (self.want_thumbs(), self.settings.hide_extensions);
        for t in &self.tabs {
            t.model.set_display(want, hide_ext);
        }
        self.settings.save();
    }

    fn set_show_hidden(&mut self, show: bool) {
        self.settings.show_hidden = show;
        self.ui.set_show_hidden(show);
        for t in &self.tabs {
            t.model.set_show_hidden(show);
        }
        for i in 0..self.tabs.len() {
            self.resort(i);
        }
        self.settings.save();
        self.update_status();
    }

    // ---------------- search ----------------

    fn search_edited(&mut self, text: String) {
        if text.trim().is_empty() {
            self.search_timer.stop();
            return self.clear_search();
        }
        self.search_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(120),
            move || {
                let t = text.clone();
                with_app(move |app| app.start_search(t));
            },
        );
    }

    /// Recursive fuzzy search from the current folder (home on This PC), streamed and ranked.
    fn start_search(&mut self, query: String) {
        if query.trim().is_empty() {
            return;
        }
        if self.tab().path.as_deref().is_some_and(link::is_remote) {
            return self.toast("Search isn't available in phone folders yet.".into(), false);
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        let show_hidden = self.settings.show_hidden || self.settings.search_hidden;
        let recursive = self.settings.search_subfolders;
        let tab = &mut self.tabs[self.active];
        tab.cancel.cancel();
        tab.cancel = Cancel::default();
        tab.generation = generation;
        tab.search = Some(query.clone());
        tab.loading = true;
        tab.error.clear();
        let root = tab
            .path
            .clone()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        tab.model.reset(root.clone(), generation, show_hidden);
        self.thumbs.set_generation(generation);
        let cancel = tab.cancel.clone();
        std::thread::Builder::new()
            .name("findex-search".into())
            .spawn(move || {
                let send = move |msg: Listing| post(move |app| app.on_listing(generation, msg));
                let opts = findex_scan::Options {
                    include_hidden: show_hidden,
                    recursive,
                    ..Default::default()
                };
                let mut buf: Vec<(Entry, u32)> = Vec::new();
                let mut last = Instant::now();
                findex_scan::search(&root, &query, &opts, &cancel, &mut |hits| {
                    buf.extend(hits.into_iter().map(|h| (h.entry, h.score)));
                    // Coalesce: at most ~20 UI updates per second.
                    if last.elapsed() > Duration::from_millis(50) {
                        send(Listing::Hits(std::mem::take(&mut buf)));
                        last = Instant::now();
                    }
                });
                if !cancel.is_cancelled() {
                    if !buf.is_empty() {
                        send(Listing::Hits(buf));
                    }
                    send(Listing::Done);
                }
            })
            .expect("spawn search worker");
        self.refresh_ui();
    }

    fn clear_search(&mut self) {
        if self.tab().search.is_some() {
            self.tab_mut().search = None;
            self.ui.set_search_text(SharedString::new());
            self.load();
        }
    }

    // ---------------- clipboard and operations ----------------

    fn clip(&mut self, cut: bool) {
        let paths = self.tab().model.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some((paths.clone(), cut));
        self.ui.set_can_paste(true);
        if paths.iter().any(|p| link::is_remote(p)) {
            return;
        }
        // Share with other file managers; the internal copy above stays the fallback.
        std::thread::spawn(move || {
            findex_platform::set_clipboard_files(&paths, cut);
        });
    }

    fn copy_path(&mut self) {
        let text = self
            .tab()
            .model
            .selected_paths()
            .iter()
            .map(|p| format!("\"{}\"", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            self.ui.invoke_copy_text(text.into());
            self.toast("Path copied".into(), false);
        }
    }

    fn paste(&mut self) {
        let Some(dest) = self.tab().path.clone() else {
            return;
        };
        if self.tab().search.is_some() {
            return;
        }
        // Another app may own the clipboard: ask the system first, then fall back to ours.
        let ours = self.clipboard.clone();
        // Phone paths live only in our clipboard; the system one cannot carry them.
        let ours_remote = ours
            .as_ref()
            .is_some_and(|(p, _)| p.iter().any(|p| link::is_remote(p)));
        std::thread::spawn(move || {
            let system = (!ours_remote)
                .then(findex_platform::get_clipboard_files)
                .flatten()
                .map(|(uris, cut)| {
                    (
                        uris.iter()
                            .map(|u| crate::parse_location(u))
                            .collect::<Vec<_>>(),
                        cut,
                    )
                });
            post(move |app| {
                let Some((paths, cut)) = system.or(ours) else {
                    return;
                };
                if cut {
                    app.clipboard = None;
                    app.ui.set_can_paste(false);
                }
                if link::is_remote(&dest) || paths.iter().any(|p| link::is_remote(p)) {
                    return app.link_paste(paths, dest, cut);
                }
                let kind = if cut { JobKind::Move } else { JobKind::Copy };
                app.submit(kind, paths, Some(dest));
            });
        });
    }

    fn delete(&mut self, permanent: bool, ask: bool) {
        let paths = self.tab().model.selected_paths();
        if paths.is_empty() {
            return;
        }
        if self.tab().path.as_deref().is_some_and(link::is_remote) {
            return self.link_delete(paths);
        }
        if permanent && ask {
            // The dialog calls `delete-confirmed`, which comes back here with ask = false.
            self.ui.invoke_confirm_delete(paths.len() as i32);
            return;
        }
        self.submit(
            if permanent {
                JobKind::Delete
            } else {
                JobKind::Trash
            },
            paths,
            None,
        );
    }

    fn submit(&mut self, kind: JobKind, paths: Vec<PathBuf>, dest: Option<PathBuf>) {
        let job = self.queue.submit(kind, paths, dest);
        self.stats.insert(job.id, JobStats::default());
        self.update_jobs();
        self.jobs_timer
            .start(TimerMode::Repeated, Duration::from_millis(100), || {
                with_app(|app| app.update_jobs())
            });
        // Open the transfer window only for jobs that take a while (as Explorer does).
        let id = job.id;
        Timer::single_shot(Duration::from_millis(800), move || {
            with_app(|app| {
                if app
                    .queue
                    .jobs()
                    .iter()
                    .any(|j| j.id == id && !j.is_finished())
                {
                    app.show_transfers(true);
                }
            })
        });
    }

    fn job_finished(&mut self, id: u64) {
        let Some(job) = self.queue.jobs().into_iter().find(|j| j.id == id) else {
            return;
        };
        self.update_jobs();
        let created = job.created.lock().unwrap().clone();
        let n = job.sources.len();
        let items = format!("{n} item{}", if n == 1 { "" } else { "s" });
        match job.status() {
            Status::Done => {
                let (msg, undo) = match job.kind {
                    JobKind::Copy => (
                        format!("Copied {items}"),
                        Some(Undo::TrashCreated(created.clone())),
                    ),
                    JobKind::Move => (
                        format!("Moved {items}"),
                        (created.len() == job.sources.len()).then(|| {
                            Undo::MoveBack(
                                created
                                    .iter()
                                    .cloned()
                                    .zip(job.sources.iter().cloned())
                                    .collect(),
                            )
                        }),
                    ),
                    JobKind::Trash => (
                        format!("Moved {items} to Trash"),
                        Some(Undo::Restore(job.sources.clone())),
                    ),
                    JobKind::Delete => (format!("Deleted {items}"), None),
                };
                self.undo = undo;
                self.toast(msg, self.undo.is_some());
            }
            Status::Failed(e) => self.toast(format!("Couldn't finish: {e}"), false),
            Status::Cancelled => self.toast("Cancelled".into(), false),
            _ => {}
        }
        // Refresh folders the job touched (watchers come later; see TODO M1).
        let mut touched: Vec<PathBuf> = job
            .sources
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        touched.extend(job.dest.clone());
        let names: Vec<OsString> = created
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_os_string()))
            .collect();
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if t.path.as_ref().is_some_and(|p| touched.contains(p)) {
                if job.dest.as_ref() == t.path.as_ref() {
                    t.pending_select = names.clone();
                }
                t.stale = i != self.active;
            }
        }
        if self
            .tab()
            .path
            .as_ref()
            .is_some_and(|p| touched.contains(p))
            && self.tab().search.is_none()
        {
            self.load();
        }
    }

    fn update_jobs(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        let jobs = self.queue.jobs();
        let now = Instant::now();
        let mut data = Vec::new();
        let (mut active, mut done_sum, mut total_sum) = (0, 0u64, 0u64);
        for j in &jobs {
            let st = self.stats.entry(j.id).or_default();
            let (done, total) = (j.done_bytes.load(Relaxed), j.total_bytes.load(Relaxed));
            let (di, ti) = (j.done_items.load(Relaxed), j.total_items.load(Relaxed));
            let progress = if total > 0 {
                done as f64 / total as f64
            } else if ti > 0 {
                di as f64 / ti as f64
            } else {
                0.0
            };
            let status = j.status();
            let paused = j.paused.load(Relaxed);
            // Sample every tick (100 ms). An exponential moving average (time constant about 1 s)
            // keeps the speed line and graph smooth; the graph plots speed against progress.
            match st.last_time {
                Some(t) if now.duration_since(t) >= Duration::from_millis(80) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    let instant = done.saturating_sub(st.last_bytes) as f64 / dt;
                    let a = (dt / 1.0).min(1.0);
                    st.speed = if st.speed == 0.0 {
                        instant
                    } else {
                        (1.0 - a) * st.speed + a * instant
                    };
                    let advanced = st.samples.last().is_none_or(|l| progress > l.0 + 0.002); // at most ~500 points
                    if status == Status::Running && !paused && total > 0 && advanced {
                        st.samples.push((progress, st.speed));
                    }
                    st.last_bytes = done;
                    st.last_time = Some(now);
                }
                None => {
                    st.last_time = Some(now);
                    st.last_bytes = done;
                }
                _ => {}
            }
            // Finished: the graph ends exactly at the right edge.
            if status == Status::Done && total > 0 && st.samples.last().is_some_and(|l| l.0 < 1.0) {
                let v = st.samples.last().unwrap().1;
                st.samples.push((1.0, v));
            }
            let state = match status {
                Status::Queued => 0,
                Status::Running if paused => 2,
                Status::Running => 1,
                Status::Done => 3,
                Status::Failed(_) => 4,
                Status::Cancelled => 5,
            };
            if state <= 2 {
                active += 1;
                done_sum += done;
                total_sum += total;
            }
            let running = state == 1;
            if st
                .shown_at
                .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(500))
            {
                st.shown_speed = st.speed;
                st.shown_at = Some(now);
            }
            let has_speed = running && st.shown_speed > 0.0;
            let pct = (progress * 100.0).round() as u32;
            let time_left = match state {
                1 if has_speed => format::eta(total.saturating_sub(done) as f64 / st.shown_speed),
                1 => "Calculating…".into(),
                2 => "Paused".into(),
                _ => String::new(),
            };
            let items_left = ti.saturating_sub(di);
            let detail = match &status {
                Status::Queued => "Waiting…".to_string(),
                Status::Running if paused => format!("Paused  ·  {pct}% complete"),
                Status::Running => format!(
                    "{pct}% complete  ·  {}  ·  {time_left}",
                    if has_speed {
                        format::speed(st.shown_speed)
                    } else {
                        "Calculating…".into()
                    }
                ),
                Status::Done => format!(
                    "Done  ·  {}  ·  {}",
                    plural(ti, "item"),
                    format::size(total)
                ),
                Status::Failed(e) => e.clone(),
                Status::Cancelled => "Cancelled".into(),
            };
            let max = st.samples.iter().map(|s| s.1).fold(st.speed, f64::max);
            let name = j
                .sources
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            data.push(JobData {
                id: j.id as i32,
                title: job_title(j).into(),
                detail: detail.into(),
                progress: if state == 3 { 1.0 } else { progress as f32 },
                state,
                // While running, the graph always reaches the current progress point.
                graph: {
                    let mut pts = st.samples.clone();
                    if state <= 2 && total > 0 && pts.last().is_some_and(|l| progress > l.0) {
                        pts.push((progress, if paused { 0.0 } else { st.speed }));
                    }
                    graph_path(&pts)
                }
                .into(),
                speed_level: if has_speed && max > 0.0 {
                    (st.speed / max) as f32
                } else {
                    0.0
                },
                speed: if has_speed {
                    format::speed(st.shown_speed).into()
                } else {
                    SharedString::new()
                },
                name: if j.sources.len() > 1 {
                    format!("{name} and {} more", j.sources.len() - 1)
                } else {
                    name
                }
                .into(),
                time_left: time_left.into(),
                items_left: if state <= 2 {
                    format!(
                        "{} ({})",
                        items_left,
                        format::size(total.saturating_sub(done))
                    )
                } else {
                    "0".into()
                }
                .into(),
            });
        }
        data.reverse(); // newest first
        self.jobs_model.set_vec(data);
        self.ui.set_active_jobs(active);
        self.transfer.set_active_jobs(active);
        self.ui.set_jobs_progress(if total_sum > 0 {
            done_sum as f32 / total_sum as f32
        } else {
            0.0
        });
        if active == 0 {
            self.jobs_timer.stop();
            // Like Explorer, an automatically opened window closes once everything is done.
            if self.transfer_auto && self.transfer.window().is_visible() {
                self.transfer_auto = false;
                Timer::single_shot(Duration::from_millis(900), || {
                    with_app(|app| {
                        if app.ui.get_active_jobs() == 0 {
                            app.hide_transfers();
                        }
                    })
                });
            }
        }
    }

    fn job_pause(&mut self, id: u64) {
        if let Some(j) = self.queue.jobs().into_iter().find(|j| j.id == id) {
            let p = j.paused.load(std::sync::atomic::Ordering::Relaxed);
            j.paused.store(!p, std::sync::atomic::Ordering::Relaxed);
        }
        self.update_jobs();
    }

    fn job_cancel(&mut self, id: u64) {
        if let Some(j) = self.queue.jobs().into_iter().find(|j| j.id == id) {
            j.cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.update_jobs();
    }

    fn do_undo(&mut self) {
        let Some(undo) = self.undo.take() else { return };
        self.toast(String::new(), false);
        match undo {
            Undo::Rename { from, to } => {
                self.fs_task(move || std::fs::rename(&to, &from).map(|_| from))
            }
            Undo::TrashCreated(paths) => self.submit(JobKind::Trash, paths, None),
            Undo::MoveBack(pairs) => {
                for (now, orig) in pairs {
                    if let Some(parent) = orig.parent() {
                        self.submit(JobKind::Move, vec![now], Some(parent.to_path_buf()));
                    }
                }
            }
            Undo::Restore(originals) => {
                std::thread::spawn(move || {
                    let r = trash::os_limited::list().and_then(|items| {
                        let pick: Vec<_> = items
                            .into_iter()
                            .filter(|i| originals.contains(&i.original_path()))
                            .collect();
                        trash::os_limited::restore_all(pick)
                    });
                    post(move |app| {
                        if let Err(e) = r {
                            app.toast(format!("Couldn't restore: {e}"), false);
                        }
                        app.load();
                    });
                });
            }
        }
    }

    /// Runs a small filesystem task off the UI thread, then reloads and selects the resulting path.
    fn fs_task(&mut self, f: impl FnOnce() -> std::io::Result<PathBuf> + Send + 'static) {
        std::thread::spawn(move || {
            let r = f();
            post(move |app| match r {
                Ok(p) => {
                    if let Some(n) = p.file_name() {
                        app.tab_mut().pending_select = vec![n.to_os_string()];
                    }
                    app.load();
                }
                Err(e) => app.toast(format!("{e}"), false),
            });
        });
    }

    fn create_new(&mut self, folder: bool) {
        let Some(dir) = self.tab().path.clone() else {
            return;
        };
        if self.tab().search.is_some() {
            return;
        }
        if link::is_remote(&dir) {
            return self.link_create(dir, folder);
        }
        self.tab_mut().rename_after = true;
        self.fs_task(move || {
            let (stem, ext) = if folder {
                ("New folder", "")
            } else {
                ("New Text Document", ".txt")
            };
            for i in 1.. {
                let name = if i == 1 {
                    format!("{stem}{ext}")
                } else {
                    format!("{stem} ({i}){ext}")
                };
                let p = dir.join(&name);
                let r = if folder {
                    std::fs::create_dir(&p)
                } else {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&p)
                        .map(|_| ())
                };
                match r {
                    Ok(()) => return Ok(p),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            unreachable!()
        });
    }

    fn rename_start(&mut self) {
        let sel = self.tab().model.selected_entries();
        if sel.len() != 1 || self.tab().search.is_some() {
            return;
        }
        let name = self.tab().model.entries()[sel[0] as usize].name.to_string();
        self.ui.invoke_show_rename(name.into());
    }

    fn rename_commit(&mut self, name: String) {
        let sel = self.tab().model.selected_entries();
        let Some(&idx) = sel.first() else { return };
        let from = self.tab().model.path_of(idx);
        let name = name.trim().to_string();
        if !valid_name(&name) {
            return self.toast(
                "A file name can't be empty or contain a path separator.".into(),
                false,
            );
        }
        let to = from.with_file_name(&name);
        if to == from {
            return;
        }
        if link::is_remote(&from) {
            return self.link_rename(from, name);
        }
        self.undo = Some(Undo::Rename {
            from: from.clone(),
            to: to.clone(),
        });
        self.fs_task(move || {
            if std::fs::symlink_metadata(&to).is_ok() {
                return Err(std::io::Error::other(format!(
                    "There is already a file named '{name}' here."
                )));
            }
            std::fs::rename(&from, &to).map(|_| to)
        });
    }

    // ---------------- default file manager ----------------

    fn set_default_fm_state(&mut self, is_default: bool) {
        self.default_fm = is_default;
        let text: SharedString = if is_default {
            "Findex opens folders, and Super+E / Win+E opens Findex."
        } else {
            "Another app opens folders by default."
        }
        .into();
        self.ui.set_default_fm(is_default);
        self.ui.set_default_fm_text(text.clone());
        self.options.set_default_fm(is_default);
        self.options.set_default_fm_text(text);
    }

    fn toggle_default_fm(&mut self) {
        let enable = !self.default_fm;
        std::thread::spawn(move || {
            let r = findex_platform::set_default_file_manager(enable);
            let now = findex_platform::is_default_file_manager();
            post(move |app| {
                app.set_default_fm_state(now);
                match r {
                    Ok(msg) => app.toast(msg, false),
                    Err(e) => app.toast(e.to_string(), false),
                }
            });
        });
    }

    // ---------------- updates ----------------

    fn check_updates(&mut self, channel: i32, at_start: bool) {
        if self.update_checking || self.update_busy {
            return;
        }
        self.update_checking = true;
        self.options.set_update_checking(true);
        std::thread::spawn(move || {
            let r = update::check(channel);
            post(move |app| app.update_checked(r, at_start));
        });
    }

    fn update_checked(&mut self, r: Result<Option<Offer>, String>, at_start: bool) {
        self.update_checking = false;
        self.options.set_update_checking(false);
        let status = match &r {
            _ if update::is_dev() => {
                "This is a development build. It updates by rebuilding from source.".into()
            }
            Ok(Some(o)) => format!("Findex {} is available.", o.version),
            Ok(None) => "Findex is up to date.".into(),
            Err(e) => format!("Could not check for updates: {e}."),
        };
        self.options.set_update_status(status.into());
        let offer = r.ok().flatten();
        let (version, notes) = offer.as_ref().map_or((String::new(), String::new()), |o| {
            (o.version.clone(), o.notes.clone())
        });
        self.options.set_update_found(version.into());
        self.options.set_update_notes(notes.into());
        if at_start
            && let Some(o) = &offer
            && o.version != self.settings.update_skipped
        {
            self.ui.set_update_version(o.version.clone().into());
            self.ui.set_update_installed(update::VERSION.into());
            self.ui.set_update_self_install(update::self_install());
            self.ui.invoke_show_update();
        }
        self.update_offer = offer;
    }

    fn install_update(&mut self) {
        if self.update_busy {
            return;
        }
        let Some(offer) = self.update_offer.clone() else {
            return;
        };
        self.update_busy = true;
        self.set_update_progress(0.0);
        std::thread::spawn(move || {
            let shown = std::cell::Cell::new(0);
            let r = update::install(&offer, &|f| {
                let percent = (f * 100.0) as i32;
                if percent != shown.get() {
                    shown.set(percent);
                    post(move |app| app.set_update_progress(f));
                }
            });
            post(move |app| app.update_installed(r));
        });
    }

    fn set_update_progress(&self, fraction: f32) {
        self.ui.set_update_progress(fraction);
        self.options.set_update_progress(fraction);
    }

    fn update_installed(&mut self, r: Result<Installed, String>) {
        self.update_busy = false;
        self.set_update_progress(-1.0);
        self.ui.invoke_close_update();
        match r {
            Ok(Installed::Relaunch(path)) => {
                self.relaunch = Some(path);
                let _ = slint::quit_event_loop();
            }
            Ok(Installed::Saved(path)) => {
                // Nothing opens the installer: show where it is.
                if findex_platform::open_default(&path).is_err() {
                    self.open_external(&path.display().to_string());
                }
                self.toast(format!("Update saved to {}", path.display()), false);
            }
            Err(e) => {
                let msg = format!("The update failed: {e}.");
                self.options.set_update_status(msg.clone().into());
                self.toast(msg, false);
            }
        }
    }

    /// "Later" on the startup offer is remembered for that version.
    fn update_later(&mut self) {
        if self.update_busy {
            return;
        }
        if let Some(o) = &self.update_offer {
            self.settings.update_skipped = o.version.clone();
            self.settings.save();
        }
    }

    /// Starts the updated AppImage. Called at exit, after the session is saved.
    pub fn finish_relaunch(&mut self) {
        let Some(path) = self.relaunch.take() else {
            return;
        };
        let mut cmd = std::process::Command::new(path);
        // The new process must not hand its window to this one, which is exiting.
        cmd.arg("--relaunched");
        if let Some(p) = &self.tab().path {
            cmd.arg(p);
        }
        if let Err(e) = cmd.spawn() {
            eprintln!("findex: could not restart after the update: {e}");
        }
    }

    // ---------------- thumbnails ----------------

    fn thumb_ready(&mut self, generation: u64, idx: u32, t: findex_thumbs::Thumb) {
        let Some(tab) = self.tabs.iter().find(|t| t.generation == generation) else {
            return;
        };
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &t.rgba, t.width, t.height,
        );
        tab.model.set_thumb(generation, idx, Image::from_rgba8(buf));
        if self.tab().generation == generation
            && self.tab().model.selected_entries().first() == Some(&idx)
        {
            self.update_status();
        }
    }

    // ---------------- toast ----------------

    fn toast(&mut self, text: String, undo: bool) {
        self.ui.set_toast_text(text.clone().into());
        self.ui.set_toast_undo(undo);
        if !text.is_empty() {
            self.toast_timer.start(
                TimerMode::SingleShot,
                Duration::from_secs(if undo { 6 } else { 4 }),
                || {
                    with_app(|app| {
                        app.ui.set_toast_text(SharedString::new());
                        app.ui.set_toast_undo(false);
                    })
                },
            );
        }
    }

    // ---------------- keyboard ----------------

    /// All shortcuts. `typing` is true while a text field has focus: then only global chords apply.
    fn key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool, typing: bool) -> bool {
        let lower = text.to_lowercase();
        // Slint encodes special keys as private-use characters; those are not type-ahead text.
        let mut chars = text.chars();
        let printable = matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c));
        if !printable || ctrl || alt {
            self.type_ahead.0.clear(); // any other key ends type-ahead, as in Explorer
        }
        // Global chords
        if ctrl && !alt {
            match lower.as_str() {
                "t" if shift => {
                    self.reopen_closed_tab();
                    return true;
                }
                "k" if shift => {
                    self.duplicate_tab(self.active);
                    return true;
                }
                "t" => {
                    self.new_tab(dirs::home_dir());
                    return true;
                }
                "w" => {
                    self.close_tab(self.active);
                    return true;
                }
                "l" => {
                    self.ui.invoke_edit_path();
                    return true;
                }
                "f" | "e" => {
                    self.ui.invoke_focus_search();
                    return true;
                }
                _ => {}
            }
            if key_is(text, Key::Tab) || text == "\u{19}" {
                let n = self.tabs.len();
                self.switch_tab(if shift {
                    (self.active + n - 1) % n
                } else {
                    (self.active + 1) % n
                });
                return true;
            }
        }
        if alt && !ctrl {
            if key_is(text, Key::LeftArrow) {
                self.back();
                return true;
            }
            if key_is(text, Key::RightArrow) {
                self.forward();
                return true;
            }
            if key_is(text, Key::UpArrow) {
                self.up();
                return true;
            }
            if lower == "d" {
                self.ui.invoke_edit_path();
                return true;
            }
            if text == "\n" || text == "\r" {
                self.show_properties();
                return true;
            }
        }
        if key_is(text, Key::F5) {
            self.reload();
            return true;
        }
        if key_is(text, Key::F3) {
            self.ui.invoke_focus_search();
            return true;
        }
        if typing {
            // Down from the search box moves into the results.
            if key_is(text, Key::DownArrow)
                && self.tab().search.is_some()
                && self.tab().model.len() > 0
            {
                self.ui.invoke_focus_list();
                self.tab().model.move_focus(0, false, false);
                self.update_status();
                return true;
            }
            return false;
        }

        let model = self.tab().model.clone();
        let n = model.len() as isize;
        let cur = model.focus().map_or(-1, |f| f as isize);
        let view = self.ui.get_view_mode();
        let cols = if view == 5 {
            1
        } else {
            self.ui.get_grid_cols().max(1) as isize
        };
        let mut moved = None;
        if key_is(text, Key::DownArrow) {
            moved = Some(if cur < 0 { 0 } else { cur + cols });
        } else if key_is(text, Key::UpArrow) {
            moved = Some(if cur < 0 { 0 } else { cur - cols });
        } else if key_is(text, Key::RightArrow) && cols > 1 {
            moved = Some(cur + 1);
        } else if key_is(text, Key::LeftArrow) && cols > 1 {
            moved = Some((cur - 1).max(0));
        } else if key_is(text, Key::Home) {
            moved = Some(0);
        } else if key_is(text, Key::End) {
            moved = Some(n - 1);
        } else if key_is(text, Key::PageDown) {
            moved = Some(cur.max(0) + 10 * cols);
        } else if key_is(text, Key::PageUp) {
            moved = Some(cur - 10 * cols);
        }
        if let Some(to) = moved {
            if n > 0 {
                model.move_focus(to.min(n - 1), shift, ctrl);
                self.update_status();
            }
            return true;
        }
        if text == "\n" || key_is(text, Key::Return) {
            if let Some(f) = model.focus() {
                self.activate(f);
            }
            return true;
        }
        if key_is(text, Key::Backspace) {
            self.back();
            return true;
        }
        if key_is(text, Key::Delete) {
            self.delete(shift, true);
            return true;
        }
        if key_is(text, Key::F2) {
            self.rename_start();
            return true;
        }
        if key_is(text, Key::Escape) {
            if self.tab().search.is_some() {
                self.clear_search();
            } else {
                model.clear_selection();
                self.update_status();
            }
            return true;
        }
        if ctrl {
            // Ctrl+Shift+1..8 picks a view; with Shift the text may be the shifted symbol.
            if shift && let Some(v) = "!@#$%^&*".find(text).or_else(|| "12345678".find(text)) {
                self.set_view(v as i32);
                return true;
            }
            match (lower.as_str(), shift) {
                ("a", false) => {
                    model.select_all();
                    self.update_status();
                }
                ("c", false) => self.clip(false),
                ("x", false) => self.clip(true),
                ("v", false) => self.paste(),
                ("c", true) => self.copy_path(),
                ("n", true) => self.create_new(true),
                ("z", false) => self.do_undo(),
                ("h", false) => self.set_show_hidden(!self.settings.show_hidden),
                (" ", false) => {
                    if let Some(f) = model.focus() {
                        model.click(f, true, false);
                        self.update_status();
                    }
                }
                _ => return false,
            }
            return true;
        }
        // "When typing into list view: Automatically type into the Search Box".
        if printable && !alt && self.settings.typing_searches {
            self.ui.invoke_type_into_search(text.into());
            self.search_edited(text.to_string());
            return true;
        }
        // Type-ahead: jump to the next item starting with the typed letters.
        if printable && !alt {
            if self.type_ahead.1.elapsed() > Duration::from_millis(1000) {
                self.type_ahead.0.clear();
            }
            self.type_ahead.0.push_str(text);
            self.type_ahead.1 = Instant::now();
            if let Some(r) = model.find_prefix(&self.type_ahead.0) {
                model.select_only(r);
                self.update_status();
            }
            return true;
        }
        false
    }
}

/// Tab title; `full_path` is the "Display the full path in the title bar" option.
fn tab_title(t: &Tab, full_path: bool) -> String {
    match (&t.path, &t.search) {
        (_, Some(q)) => format!("Search: {q}"),
        (None, _) => "This PC".into(),
        (Some(p), _) if p.as_path() == Path::new(link::PHONES) => "Phones".into(),
        (Some(p), _) if full_path => p.display().to_string(),
        (Some(p), _) if Some(p) == dirs::home_dir().as_ref() => "Home".into(),
        (Some(p), _) => p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string()),
    }
}

/// Breadcrumbs. Paths under home start at "Home", like Explorer's user folder.
fn crumbs(path: Option<&Path>) -> Vec<Crumb> {
    let mut v = Vec::new();
    let Some(path) = path else {
        return vec![Crumb {
            label: "This PC".into(),
            path: SharedString::new(),
        }];
    };
    if link::is_remote(path) {
        let mut acc = PathBuf::new();
        for c in path.components() {
            acc.push(c);
            let label = c.as_os_str().to_string_lossy().into_owned();
            v.push(Crumb {
                label: if label == link::PHONES {
                    "Phones".into()
                } else {
                    label.into()
                },
                path: acc.display().to_string().into(),
            });
        }
        return v;
    }
    let home = dirs::home_dir();
    let (mut acc, rest) = match home
        .as_deref()
        .and_then(|h| path.strip_prefix(h).ok().map(|r| (h, r)))
    {
        Some((h, r)) => {
            v.push(Crumb {
                label: "Home".into(),
                path: h.display().to_string().into(),
            });
            (h.to_path_buf(), r.to_path_buf())
        }
        None => (PathBuf::new(), path.to_path_buf()),
    };
    for c in rest.components() {
        acc.push(c);
        let label = match c {
            std::path::Component::RootDir if v.is_empty() => "File System".to_string(),
            std::path::Component::RootDir => continue,
            other => other.as_os_str().to_string_lossy().into_owned(),
        };
        v.push(Crumb {
            label: label.into(),
            path: acc.display().to_string().into(),
        });
    }
    v
}

fn job_title(j: &Job) -> String {
    let n = j.sources.len();
    let what = if n == 1 {
        format!(
            "'{}'",
            j.sources[0]
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        )
    } else {
        format!("{n} items")
    };
    let dest = j
        .dest
        .as_ref()
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match j.kind {
        JobKind::Copy => format!("Copying {what} to {dest}"),
        JobKind::Move => format!("Moving {what} to {dest}"),
        JobKind::Trash => format!("Moving {what} to Trash"),
        JobKind::Delete => format!("Deleting {what}"),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(std::path::is_separator)
}

/// Properties gathered off the UI thread (`slint::Image` is not `Send`, so the thumb is added later).
#[derive(Clone, Default)]
struct PropsInfo {
    count: i32,
    name: String,
    kind: i32,
    is_dir: bool,
    type_name: String,
    opens_with: String,
    location: String,
    size: String,
    size_on_disk: String,
    contains: String,
    created: String,
    modified: String,
    accessed: String,
    /// 0 off, 1 on, 2 mixed
    readonly: i32,
    hidden: i32,
}

/// "1.2 MB (1,234,567 bytes)"
fn size_with_bytes(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if bytes < 1024 {
        format!("{grouped} bytes")
    } else {
        format!("{} ({grouped} bytes)", format::size(bytes))
    }
}

fn time_text(t: std::io::Result<std::time::SystemTime>) -> String {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format::date(d.as_secs() as i64))
        .unwrap_or_default()
}

/// Reads properties of `targets` and walks folders for size and contents, never following symlinks.
/// Calls `sink` with partial results about every 250 ms, then with the final result.
fn gather_props(targets: &[PathBuf], cancel: &Cancel, sink: &mut dyn FnMut(PropsInfo)) {
    let single = targets.len() == 1;
    let metas: Vec<_> = targets
        .iter()
        .map(|p| std::fs::symlink_metadata(p).ok())
        .collect();
    let tri = |f: &dyn Fn(usize) -> bool| {
        let on = (0..targets.len()).filter(|&i| f(i)).count();
        if on == 0 {
            0
        } else if on == targets.len() {
            1
        } else {
            2
        }
    };
    let mut base = PropsInfo {
        count: targets.len() as i32,
        readonly: tri(&|i| {
            std::fs::metadata(&targets[i]).is_ok_and(|m| findex_platform::is_readonly(&m))
        }),
        hidden: tri(&|i| findex_platform::is_hidden(&targets[i])),
        ..Default::default()
    };
    let kind_of = |i: usize| {
        if metas[i].as_ref().is_some_and(|m| m.is_dir()) {
            findex_core::Kind::Folder
        } else {
            findex_core::Kind::from_name(
                &targets[i].file_name().unwrap_or_default().to_string_lossy(),
            )
        }
    };
    let parent = |p: &PathBuf| {
        p.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    };
    if single {
        let p = &targets[0];
        let kind = kind_of(0);
        base.name = p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        base.kind = kind as i32;
        base.is_dir = kind == findex_core::Kind::Folder;
        base.type_name = kind.label().into();
        base.location = parent(p);
        if let Some(m) = &metas[0] {
            base.created = time_text(m.created());
            base.modified = time_text(m.modified());
            base.accessed = time_text(m.accessed());
        }
        // Show what we have at once; "Opens with" can take a moment.
        sink(base.clone());
        if !base.is_dir {
            base.opens_with = findex_platform::default_app_name(p).unwrap_or_default();
        }
    } else {
        let first = kind_of(0);
        base.type_name = if (1..targets.len()).all(|i| kind_of(i) == first) {
            format!("All of type {}", first.label())
        } else {
            "Multiple types".into()
        };
        let loc = parent(&targets[0]);
        if targets.iter().all(|p| parent(p) == loc) {
            base.location = format!("All in {loc}");
        }
        sink(base.clone());
    }

    let (mut bytes, mut alloc, mut files, mut folders) = (0u64, 0u64, 0u64, 0u64);
    let mut has_alloc = true;
    // (folder, device): the walk stays on the device it started on, as `du -x` does.
    let mut stack = Vec::new();
    for (p, m) in targets.iter().zip(&metas) {
        let Some(m) = m else { continue };
        if m.is_dir() {
            stack.push((p.clone(), findex_platform::device_id(p)));
            folders += !single as u64;
        } else {
            files += !single as u64;
            bytes += m.len();
            match findex_platform::allocated_size(m) {
                Some(a) => alloc += a,
                None => has_alloc = false,
            }
        }
    }
    let any_dir = !stack.is_empty();
    let mut last = Instant::now();
    let emit = |bytes: u64, alloc: u64, files: u64, folders: u64, has_alloc: bool| PropsInfo {
        size: size_with_bytes(bytes),
        size_on_disk: if has_alloc {
            size_with_bytes(alloc)
        } else {
            String::new()
        },
        contains: if any_dir || !single {
            format!(
                "{files} File{}, {folders} Folder{}",
                if files == 1 { "" } else { "s" },
                if folders == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        },
        ..base.clone()
    };
    while let Some((dir, dev)) = stack.pop() {
        if cancel.is_cancelled() {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            // DirEntry::metadata does not follow symlinks.
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                folders += 1;
                let sub = e.path();
                if findex_platform::device_id(&sub) == dev {
                    stack.push((sub, dev));
                }
            } else {
                files += 1;
                bytes += m.len();
                match findex_platform::allocated_size(&m) {
                    Some(a) => alloc += a,
                    None => has_alloc = false,
                }
            }
        }
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            sink(emit(bytes, alloc, files, folders, has_alloc));
        }
    }
    sink(emit(bytes, alloc, files, folders, has_alloc));
}

/// Graph size in pixels; must match `GraphMetrics` in transfer.slint.
const GRAPH_W: f64 = 472.0;
const GRAPH_H: f64 = 96.0;

/// Area chart of speed (y) against progress (x), like the Windows copy dialog: the graph grows
/// from left to right as the job advances and reaches the right edge at 100%. Peaks reach 85% of
/// the height. Segments are quadratic curves through the midpoints, so the outline is smooth.
fn graph_path(samples: &[(f64, f64)]) -> String {
    if samples.is_empty() {
        return String::new();
    }
    let max = samples.iter().map(|s| s.1).fold(1.0, f64::max);
    let pt = |(p, v): (f64, f64)| {
        (
            p.clamp(0.0, 1.0) * GRAPH_W,
            GRAPH_H - v / max * GRAPH_H * 0.85,
        )
    };
    let pts: Vec<(f64, f64)> = std::iter::once((0.0, samples[0].1))
        .chain(samples.iter().copied())
        .map(pt)
        .collect();
    let mut s = format!("M 0 {GRAPH_H} L {:.1} {:.1}", pts[0].0, pts[0].1);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        s.push_str(&format!(
            " Q {:.1} {:.1} {:.1} {:.1}",
            a.0,
            a.1,
            (a.0 + b.0) / 2.0,
            (a.1 + b.1) / 2.0
        ));
    }
    let last = *pts.last().unwrap();
    s.push_str(&format!(
        " L {:.1} {:.1} L {:.1} {GRAPH_H} Z",
        last.0, last.1, last.0
    ));
    s
}

fn plural(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Requests the OS backdrop (Mica on Windows 11) for a window and tells its theme.
fn apply_effects(window: &slint::Window, theme: &Theme) {
    use raw_window_handle::HasWindowHandle;
    let applied = window
        .window_handle()
        .window_handle()
        .map(|h| findex_platform::apply_window_effects(h.as_raw(), theme.get_dark()))
        .unwrap_or(false);
    theme.set_system_backdrop(applied);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_spans_progress_and_peaks_at_85_percent() {
        let p = graph_path(&[(0.1, 5.0), (0.5, 10.0), (1.0, 10.0)]);
        assert!(p.starts_with("M 0 96") && p.ends_with("Z"), "{p}");
        assert!(p.contains(" Q "), "{p}"); // smooth curve segments
        assert!(p.ends_with("L 472.0 14.4 L 472.0 96 Z"), "{p}"); // reaches the right edge at 100%, peak at 85%
        assert!(graph_path(&[]).is_empty());
    }

    #[test]
    fn crumbs_for_root_path() {
        let c = crumbs(Some(Path::new("/usr/share")));
        let labels: Vec<_> = c.iter().map(|c| c.label.to_string()).collect();
        assert_eq!(labels, ["File System", "usr", "share"]);
        assert_eq!(c[2].path, "/usr/share");
    }
}

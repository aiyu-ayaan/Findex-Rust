//! User settings, stored as `key=value` lines in the config dir (`$XDG_CONFIG_HOME/findex`, `%APPDATA%\findex`).

use std::path::PathBuf;

use findex_ui::Options;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// 0 system, 1 light, 2 dark
    pub theme: i32,
    pub transparency: bool,
    pub show_hidden: bool,
    pub compact: bool,
    pub view: i32,
    pub details_pane: bool,
    // Folder Options: General
    pub open_to_this_pc: bool,
    pub folders_in_new_tab: bool,
    pub single_click: bool,
    // Folder Options: View
    pub never_thumbnails: bool,
    pub full_path_title: bool,
    pub hide_empty_drives: bool,
    pub hide_extensions: bool,
    pub restore_tabs: bool,
    pub show_mount_paths: bool,
    pub show_status_bar: bool,
    pub check_boxes: bool,
    pub typing_searches: bool,
    pub nav_places: bool,
    pub nav_this_pc: bool,
    pub nav_network: bool,
    pub nav_trash: bool,
    // Folder Options: Search
    pub search_subfolders: bool,
    pub search_hidden: bool,
    /// Open tabs at exit, for "Restore previous folder tabs at startup".
    pub tabs: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: 0,
            transparency: true,
            show_hidden: false,
            compact: false,
            view: 5,
            details_pane: false,
            open_to_this_pc: false,
            folders_in_new_tab: false,
            single_click: false,
            never_thumbnails: false,
            full_path_title: false,
            hide_empty_drives: true,
            hide_extensions: false,
            restore_tabs: false,
            show_mount_paths: true,
            show_status_bar: true,
            check_boxes: false,
            typing_searches: false,
            nav_places: true,
            nav_this_pc: true,
            nav_network: true,
            nav_trash: true,
            search_subfolders: true,
            search_hidden: false,
            tabs: Vec::new(),
        }
    }
}

fn file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("findex/settings.ini"))
}

/// Every boolean setting, so parsing and writing share one list.
macro_rules! bool_keys {
    ($m:ident) => {
        $m!(
            transparency,
            show_hidden,
            compact,
            details_pane,
            open_to_this_pc,
            folders_in_new_tab,
            single_click,
            never_thumbnails,
            full_path_title,
            hide_empty_drives,
            hide_extensions,
            restore_tabs,
            show_mount_paths,
            show_status_bar,
            check_boxes,
            typing_searches,
            nav_places,
            nav_this_pc,
            nav_network,
            nav_trash,
            search_subfolders,
            search_hidden
        )
    };
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let b = v == "1" || v == "true";
            macro_rules! set_bool {
                ($($f:ident),*) => {
                    match k {
                        $(stringify!($f) => s.$f = b,)*
                        "theme" => s.theme = v.parse().unwrap_or(0).clamp(0, 2),
                        "view" => s.view = v.parse().unwrap_or(5).clamp(0, 7),
                        "tab" => s.tabs.push(PathBuf::from(v)),
                        _ => {}
                    }
                };
            }
            bool_keys!(set_bool);
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut t = format!("theme={}\nview={}\n", self.theme, self.view);
        macro_rules! put_bool {
            ($($f:ident),*) => { $( t.push_str(&format!("{}={}\n", stringify!($f), self.$f as u8)); )* };
        }
        bool_keys!(put_bool);
        for tab in &self.tabs {
            t.push_str(&format!("tab={}\n", tab.display()));
        }
        t
    }

    pub fn load() -> Settings {
        Settings::parse(
            &file()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .unwrap_or_default(),
        )
    }

    /// Writes on a background thread (the UI thread never does I/O).
    pub fn save(&self) {
        let text = self.to_text();
        std::thread::spawn(move || write(text));
    }

    /// Synchronous save, used once at exit when no event loop remains.
    pub fn save_now(&self) {
        write(self.to_text());
    }

    pub fn to_options(&self) -> Options {
        Options {
            open_to_this_pc: self.open_to_this_pc,
            folders_in_new_tab: self.folders_in_new_tab,
            single_click: self.single_click,
            never_thumbnails: self.never_thumbnails,
            full_path_title: self.full_path_title,
            show_hidden: self.show_hidden,
            hide_empty_drives: self.hide_empty_drives,
            hide_extensions: self.hide_extensions,
            restore_tabs: self.restore_tabs,
            show_mount_paths: self.show_mount_paths,
            show_status_bar: self.show_status_bar,
            check_boxes: self.check_boxes,
            typing_searches: self.typing_searches,
            nav_places: self.nav_places,
            nav_this_pc: self.nav_this_pc,
            nav_network: self.nav_network,
            nav_trash: self.nav_trash,
            search_subfolders: self.search_subfolders,
            search_hidden: self.search_hidden,
            theme: self.theme,
            transparency: self.transparency,
            compact: self.compact,
        }
    }

    pub fn set_options(&mut self, o: &Options) {
        self.open_to_this_pc = o.open_to_this_pc;
        self.folders_in_new_tab = o.folders_in_new_tab;
        self.single_click = o.single_click;
        self.never_thumbnails = o.never_thumbnails;
        self.full_path_title = o.full_path_title;
        self.show_hidden = o.show_hidden;
        self.hide_empty_drives = o.hide_empty_drives;
        self.hide_extensions = o.hide_extensions;
        self.restore_tabs = o.restore_tabs;
        self.show_mount_paths = o.show_mount_paths;
        self.show_status_bar = o.show_status_bar;
        self.check_boxes = o.check_boxes;
        self.typing_searches = o.typing_searches;
        self.nav_places = o.nav_places;
        self.nav_this_pc = o.nav_this_pc;
        self.nav_network = o.nav_network;
        self.nav_trash = o.nav_trash;
        self.search_subfolders = o.search_subfolders;
        self.search_hidden = o.search_hidden;
        self.theme = o.theme.clamp(0, 2);
        self.transparency = o.transparency;
        self.compact = o.compact;
    }
}

fn write(text: String) {
    if let Some(f) = file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_setting() {
        let s = Settings {
            theme: 2,
            view: 1,
            single_click: true,
            nav_trash: false,
            tabs: vec![PathBuf::from("/tmp/a b"), PathBuf::from("/home")],
            ..Default::default()
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let mut o = Settings::default();
        o.set_options(&s.to_options());
        assert!(o.single_click && !o.nav_trash && o.theme == 2);
    }
}

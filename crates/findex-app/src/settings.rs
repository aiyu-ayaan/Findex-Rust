//! User settings, stored as `key=value` lines in the config dir (`$XDG_CONFIG_HOME/findex`, `%APPDATA%\findex`).

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// 0 system, 1 light, 2 dark
    pub theme: i32,
    pub transparency: bool,
    pub show_hidden: bool,
    pub compact: bool,
    pub view: i32,
    pub details_pane: bool,
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
        }
    }
}

fn file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("findex/settings.ini"))
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let text = file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let b = v == "1" || v == "true";
            match k {
                "theme" => s.theme = v.parse().unwrap_or(0).clamp(0, 2),
                "transparency" => s.transparency = b,
                "show_hidden" => s.show_hidden = b,
                "compact" => s.compact = b,
                "view" => s.view = v.parse().unwrap_or(5).clamp(0, 7),
                "details_pane" => s.details_pane = b,
                _ => {}
            }
        }
        s
    }

    /// Writes on a background thread (the UI thread never does I/O).
    pub fn save(&self) {
        let text = format!(
            "theme={}\ntransparency={}\nshow_hidden={}\ncompact={}\nview={}\ndetails_pane={}\n",
            self.theme,
            self.transparency as u8,
            self.show_hidden as u8,
            self.compact as u8,
            self.view,
            self.details_pane as u8
        );
        std::thread::spawn(move || {
            if let Some(f) = file() {
                let _ = std::fs::create_dir_all(f.parent().unwrap());
                let _ = std::fs::write(f, text);
            }
        });
    }
}

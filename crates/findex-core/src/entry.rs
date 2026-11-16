use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Broad file category. Drives the icon glyph, the icon tint and the "Type" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    Folder = 0,
    File = 1,
    Image = 2,
    Video = 3,
    Audio = 4,
    Document = 5,
    Pdf = 6,
    Archive = 7,
    Code = 8,
    Executable = 9,
}

impl Kind {
    pub fn from_name(name: &str) -> Kind {
        let Some((_, ext)) = name.rsplit_once('.') else {
            return Kind::File;
        };
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "svg"
            | "heic" | "heif" | "avif" | "raw" | "cr2" | "nef" | "psd" | "xcf" => Kind::Image,
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg"
            | "3gp" => Kind::Video,
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "wma" | "aiff" => Kind::Audio,
            "pdf" => Kind::Pdf,
            "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "xls" | "xlsx" | "ods" | "csv"
            | "ppt" | "pptx" | "odp" | "epub" => Kind::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "iso" | "deb"
            | "rpm" | "cab" => Kind::Archive,
            "rs" | "c" | "h" | "cpp" | "hpp" | "py" | "js" | "ts" | "tsx" | "jsx" | "java"
            | "kt" | "go" | "rb" | "php" | "cs" | "swift" | "sh" | "toml" | "json" | "yaml"
            | "yml" | "xml" | "html" | "css" | "slint" | "lua" | "sql" => Kind::Code,
            "exe" | "msi" | "bat" | "cmd" | "appimage" | "bin" | "run" => Kind::Executable,
            _ => Kind::File,
        }
    }

    /// Text for the Details "Type" column.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "File folder",
            Kind::File => "File",
            Kind::Image => "Image",
            Kind::Video => "Video",
            Kind::Audio => "Audio",
            Kind::Document => "Document",
            Kind::Pdf => "PDF document",
            Kind::Archive => "Compressed archive",
            Kind::Code => "Source file",
            Kind::Executable => "Application",
        }
    }
}

pub const FLAG_HIDDEN: u8 = 1;
pub const FLAG_SYMLINK: u8 = 2;

/// One directory entry. Kept small: listings can hold millions.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Display name (lossy UTF-8).
    pub name: Box<str>,
    /// Exact on-disk name, only when it is not valid UTF-8.
    pub raw: Option<Box<OsStr>>,
    /// Containing folder, set only for search results that live outside the tab's folder.
    pub parent: Option<Arc<Path>>,
    pub size: u64,
    /// Modification time, Unix seconds. 0 means unknown.
    pub mtime: i64,
    pub kind: Kind,
    pub flags: u8,
}

impl Entry {
    pub fn new(name: OsString, is_dir: bool, size: u64, mtime: i64, flags: u8) -> Entry {
        let (name, raw) = match name.into_string() {
            Ok(s) => (s.into_boxed_str(), None),
            Err(os) => (
                os.to_string_lossy().into_owned().into_boxed_str(),
                Some(os.into_boxed_os_str()),
            ),
        };
        let mut flags = flags;
        if name.starts_with('.') {
            flags |= FLAG_HIDDEN;
        }
        let kind = if is_dir {
            Kind::Folder
        } else {
            Kind::from_name(&name)
        };
        Entry {
            name,
            raw,
            parent: None,
            size,
            mtime,
            kind,
            flags,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Folder
    }

    pub fn is_hidden(&self) -> bool {
        self.flags & FLAG_HIDDEN != 0
    }

    pub fn os_name(&self) -> &OsStr {
        self.raw.as_deref().unwrap_or(OsStr::new(&*self.name))
    }

    /// Full path, given the folder the listing was made from.
    pub fn path_in(&self, folder: &Path) -> PathBuf {
        self.parent
            .as_deref()
            .unwrap_or(folder)
            .join(self.os_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitive() {
        assert_eq!(Kind::from_name("a.JPG"), Kind::Image);
        assert_eq!(Kind::from_name("archive.tar.gz"), Kind::Archive);
        assert_eq!(Kind::from_name("Makefile"), Kind::File);
    }

    #[test]
    fn dotfiles_are_hidden() {
        let e = Entry::new(".bashrc".into(), false, 1, 0, 0);
        assert!(e.is_hidden());
    }
}

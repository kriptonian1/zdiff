//! The settings file: saved keys, the startup layout, and general options.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::{CONTEXT_LINES, View};
use crate::ui::Theme;

/// Everything zdiff remembers between runs; missing parts take their defaults.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    /// Written only by Save layout as default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    /// Action id to its keys, only for actions changed from the built-in keys.
    pub keys: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a file format: each is an independent setting"
)]
#[serde(default)]
pub struct General {
    pub syntax: bool,
    /// Watch for changes when started without `--watch`.
    pub watch: bool,
    /// Checkboxes, Stage, and Commit in the sidebar.
    pub staging: bool,
    /// Pictures for changed images, when the terminal can draw them.
    pub images: bool,
    /// SVGs open as rendered pictures instead of code.
    pub svg_preview: bool,
    /// The footer badge with our own memory and CPU.
    pub usage: bool,
    pub theme: Theme,
}

impl Default for General {
    fn default() -> Self {
        Self {
            syntax: true,
            watch: false,
            staging: true,
            images: true,
            svg_preview: false,
            usage: true,
            theme: Theme::default(),
        }
    }
}

/// How the window looks at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a file format: each is an independent setting"
)]
#[serde(default)]
pub struct Layout {
    /// Left out for Auto, which follows the pane width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
    pub all_files: bool,
    pub sidebar_right: bool,
    pub sidebar_hidden: bool,
    /// Left out when the sidebar was never dragged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u16>,
    pub context: u32,
    /// Every unchanged line shown, instead of `context` lines.
    pub all_lines: bool,
    pub word_highlights: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            view: None,
            all_files: false,
            sidebar_right: false,
            sidebar_hidden: false,
            sidebar_width: None,
            context: CONTEXT_LINES,
            all_lines: false,
            word_highlights: true,
        }
    }
}

/// `$XDG_CONFIG_HOME/zdiff/config.toml`, else `$HOME/.config/zdiff/config.toml`.
pub fn path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn path_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = (xdg.filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .or_else(|| home.map(|home| Path::new(&home).join(".config")))?;
    Some(base.join("zdiff").join("config.toml"))
}

impl Settings {
    /// The settings at `path`; a missing file gives the defaults, a broken one the defaults
    /// plus a warning saying where it broke.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(settings) => (settings, Vec::new()),
                Err(e) => (
                    Self::default(),
                    vec![format!("{}: {}", path.display(), e.message())],
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (Self::default(), vec![format!("{}: {e}", path.display())]),
        }
    }

    /// Writes a temporary file, then renames it over `path`, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (settings, warnings) = Settings::load(&dir.path().join("config.toml"));
        assert_eq!(settings, Settings::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn settings_survive_a_save_and_load_and_leave_no_temp_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested/config.toml");
        let settings = Settings {
            general: General {
                syntax: false,
                watch: true,
                staging: false,
                images: false,
                svg_preview: true,
                usage: false,
                theme: Theme::GithubLight,
            },
            layout: Some(Layout {
                view: Some(View::Unified),
                sidebar_right: true,
                sidebar_width: Some(40),
                all_lines: true,
                ..Layout::default()
            }),
            keys: BTreeMap::from([("next_file".into(), vec!["m".into(), "Ctrl+N".into()])]),
        };
        settings.save(&path).expect("saved");
        assert_eq!(Settings::load(&path), (settings, Vec::new()));
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1, "only config.toml");
    }

    #[test]
    fn a_broken_file_gives_the_defaults_and_says_what_broke() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[layout\nview = 1\n").unwrap();
        let (settings, warnings) = Settings::load(&path);
        assert_eq!(settings, Settings::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("config.toml"), "{warnings:?}");
    }

    #[test]
    fn the_path_prefers_xdg_then_home() {
        let path = |xdg: Option<&str>, home: Option<&str>| {
            path_from(xdg.map(Into::into), home.map(Into::into))
        };
        assert_eq!(
            path(Some("/x"), Some("/h")),
            Some("/x/zdiff/config.toml".into())
        );
        assert_eq!(
            path(Some(""), Some("/h")),
            Some("/h/.config/zdiff/config.toml".into())
        );
        assert_eq!(path(None, None), None);
    }
}

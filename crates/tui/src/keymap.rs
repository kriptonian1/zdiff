//! Which key runs which [`Action`], and where.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::menu::Action;

/// Where a binding applies; `Global` is checked before the focused pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Global,
    Sidebar,
    Diff,
}

impl Context {
    /// How the shortcuts screen names this context.
    pub fn place(self) -> &'static str {
        match self {
            Self::Global => "everywhere",
            Self::Sidebar => "sidebar",
            Self::Diff => "diff",
        }
    }
}

/// A key and its modifiers, normalized so a terminal's spelling doesn't matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Chord {
    /// Letters and symbols carry Shift in the character itself (`G`, `+`), so Shift is dropped
    /// unless Ctrl or Alt is held; then the letter is lowercased and Shift kept, since
    /// terminals send Ctrl+Shift+F as either `F` or `f`.
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let command = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let held = mods & (command | KeyModifiers::SHIFT);
        let (code, mods) = match code {
            KeyCode::Char(c) if held.intersects(command) => {
                let shift = held.contains(KeyModifiers::SHIFT) || c.is_uppercase();
                let shift = if shift {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                (
                    KeyCode::Char(c.to_ascii_lowercase()),
                    (held & command) | shift,
                )
            }
            KeyCode::Char(_) | KeyCode::BackTab => (code, held - KeyModifiers::SHIFT),
            _ => (code, held),
        };
        Self { code, mods }
    }

    pub fn from_event(key: KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }

    /// Esc and Ctrl+C always cancel or quit, so they can't be captured or bound.
    pub fn is_locked(self) -> bool {
        self == Self::new(KeyCode::Esc, KeyModifiers::NONE)
            || self == Self::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    /// Ctrl, Alt, or an F-key: these reach bindings even while a text input is open.
    pub fn is_command(self) -> bool {
        self.mods
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || matches!(self.code, KeyCode::F(_))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (KeyModifiers::CONTROL, "Ctrl+"),
            (KeyModifiers::ALT, "Alt+"),
            (KeyModifiers::SHIFT, "Shift+"),
        ] {
            if self.mods.contains(flag) {
                f.write_str(name)?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) if self.is_command() => write!(f, "{}", c.to_ascii_uppercase()),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::BackTab => f.write_str("Shift+Tab"),
            KeyCode::F(n) => write!(f, "F{n}"),
            code => write!(f, "{code:?}"),
        }
    }
}

/// Reads the names [`Chord`]'s `Display` writes, plus `Up`, `Down`, `Left`, and `Right`.
impl FromStr for Chord {
    /// A warning naming the key, like `unknown key "Ctrl+Nope"`.
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("unknown key {text:?}");
        // `+` alone, or a trailing `+` as in `Ctrl++`, is the plus key itself.
        let (prefix, key) = match text.strip_suffix("++") {
            Some(prefix) => (prefix, "+"),
            None if text == "+" => ("", "+"),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        let mut mods = KeyModifiers::NONE;
        for part in prefix.split('+').filter(|p| !p.is_empty()) {
            mods |= match part.to_ascii_lowercase().as_str() {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(bad()),
            };
        }
        let mut chars = key.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => match c {
                '↑' => KeyCode::Up,
                '↓' => KeyCode::Down,
                '←' => KeyCode::Left,
                '→' => KeyCode::Right,
                // `Display` capitalizes Ctrl/Alt letters; Shift must be written out.
                c if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    KeyCode::Char(c.to_ascii_lowercase())
                }
                c => KeyCode::Char(c),
            },
            _ => match key.to_ascii_lowercase().as_str() {
                "space" => KeyCode::Char(' '),
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "backspace" => KeyCode::Backspace,
                "delete" => KeyCode::Delete,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                f => match f.strip_prefix('f').and_then(|n| n.parse().ok()) {
                    Some(n @ 1..=12) => KeyCode::F(n),
                    _ => return Err(bad()),
                },
            },
        };
        Ok(Self::new(code, mods))
    }
}

/// Every binding, in the order the shortcuts screen lists them.
#[derive(Debug)]
pub struct Keymap {
    bindings: Vec<(Context, Chord, Action)>,
}

impl Keymap {
    /// The built-in keys.
    pub fn defaults() -> Self {
        use Action as A;
        use Context::{Diff, Global, Sidebar};
        use KeyCode::{Char, Down, End, Enter, Esc, Home, Left, PageDown, PageUp, Right, Tab, Up};
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let table = [
            (Global, KeyCode::F(10), none, A::OpenMenu),
            (Global, Char('k'), ctrl, A::Shortcuts),
            (Global, Char('p'), ctrl, A::GoToFile),
            (Global, Char('g'), ctrl, A::GoToLine),
            (Global, Char('f'), ctrl, A::Find),
            (Global, Char('f'), ctrl_shift, A::SearchAll),
            (Global, Char('b'), ctrl, A::Sidebar),
            (Global, Char('r'), ctrl, A::Reload),
            (Global, Char('q'), none, A::Quit),
            (Global, Char('r'), none, A::SvgView),
            (Global, Char('i'), none, A::FocusCommit),
            (Global, Enter, ctrl, A::Commit),
            (Global, Esc, none, A::Quit),
            (Global, Tab, none, A::ToggleFocus),
            (Global, Char('n'), none, A::NextFile),
            (Global, Char('p'), none, A::PrevFile),
            (Global, Char('v'), none, A::ToggleView),
            (Global, Char('a'), none, A::ToggleScope),
            (Global, Char('e'), none, A::ExpandAll),
            (Global, Char('c'), none, A::CollapseAll),
            (Global, Char('+'), none, A::MoreContext),
            (Global, Char('='), none, A::MoreContext),
            (Global, Char('-'), none, A::LessContext),
            (Global, Char('w'), none, A::WordHighlights),
            (Diff, Char('j'), none, A::ScrollDown),
            (Diff, Down, none, A::ScrollDown),
            (Diff, Char('k'), none, A::ScrollUp),
            (Diff, Up, none, A::ScrollUp),
            (Diff, Char('d'), ctrl, A::HalfPageDown),
            (Diff, Char('u'), ctrl, A::HalfPageUp),
            (Diff, PageDown, none, A::PageDown),
            (Diff, PageUp, none, A::PageUp),
            (Diff, Char('g'), none, A::Top),
            (Diff, Home, none, A::Top),
            (Diff, Char('G'), none, A::Bottom),
            (Diff, End, none, A::Bottom),
            (Diff, Char(']'), none, A::NextChange),
            (Diff, Char('['), none, A::PrevChange),
            (Diff, Enter, none, A::ExpandFold),
            (Diff, Char('o'), none, A::ImageCompare),
            (Diff, Char('h'), none, A::ScrollLeft),
            (Diff, Left, none, A::ScrollLeft),
            (Diff, Char('l'), none, A::ScrollRight),
            (Diff, Right, none, A::ScrollRight),
            (Sidebar, Char('j'), none, A::SelectDown),
            (Sidebar, Down, none, A::SelectDown),
            (Sidebar, Char('k'), none, A::SelectUp),
            (Sidebar, Up, none, A::SelectUp),
            (Sidebar, PageDown, none, A::SelectPageDown),
            (Sidebar, PageUp, none, A::SelectPageUp),
            (Sidebar, Char('g'), none, A::SelectFirst),
            (Sidebar, Home, none, A::SelectFirst),
            (Sidebar, Char('G'), none, A::SelectLast),
            (Sidebar, End, none, A::SelectLast),
            (Sidebar, Enter, none, A::ToggleFolder),
            (Sidebar, Char(' '), none, A::ToggleStage),
            (Sidebar, Char('s'), none, A::Stage),
            (Sidebar, Char('h'), none, A::CloseFolder),
            (Sidebar, Left, none, A::CloseFolder),
            (Sidebar, Char('l'), none, A::OpenFolder),
            (Sidebar, Right, none, A::OpenFolder),
        ];
        Self {
            bindings: (table.into_iter())
                .map(|(context, code, mods, action)| (context, Chord::new(code, mods), action))
                .collect(),
        }
    }

    /// The action `chord` runs in `context`, if any.
    // ponytail: linear scan of ~60 bindings per key; use a map if bindings reach the hundreds.
    pub fn lookup(&self, context: Context, chord: Chord) -> Option<Action> {
        (self.bindings.iter())
            .find(|&&(c, k, _)| c == context && k == chord)
            .map(|&(_, _, action)| action)
    }

    /// Every binding of `action`, in table order; the first is the one menus show.
    pub fn keys(&self, action: Action) -> impl Iterator<Item = (Context, Chord)> + '_ {
        (self.bindings.iter())
            .filter(move |&&(_, _, a)| a == action)
            .map(|&(context, chord, _)| (context, chord))
    }

    /// Every binding, in table order.
    pub fn bindings(&self) -> impl Iterator<Item = (Context, Chord, Action)> + '_ {
        self.bindings.iter().copied()
    }

    /// Where `action`'s keys work: where its default keys do, else everywhere.
    fn context_of(action: Action) -> Context {
        (Self::defaults().keys(action).next()).map_or(Context::Global, |(context, _)| context)
    }

    /// Makes `chords` the only keys of `action`.
    pub fn set(&mut self, action: Action, chords: &[Chord]) {
        let context = Self::context_of(action);
        self.bindings.retain(|&(_, _, a)| a != action);
        (self.bindings).extend(chords.iter().map(|&chord| (context, chord, action)));
    }

    /// Gives `action` one more key, after the ones it has.
    pub fn add(&mut self, action: Action, chord: Chord) {
        self.bindings
            .push((Self::context_of(action), chord, action));
    }

    /// Takes `chord` away from `action`, leaving its other keys.
    pub fn remove(&mut self, action: Action, chord: Chord) {
        self.bindings
            .retain(|&(_, c, a)| !(a == action && c == chord));
    }

    /// Puts `action` back on its built-in keys.
    pub fn reset(&mut self, action: Action) {
        let defaults: Vec<Chord> = Self::defaults().keys(action).map(|(_, c)| c).collect();
        self.set(action, &defaults);
    }

    /// Another action `chord` would clash with if given to `action`: one in the same pane,
    /// or anywhere when either of them works everywhere.
    pub fn holder(&self, action: Action, chord: Chord) -> Option<Action> {
        let context = Self::context_of(action);
        (self.bindings.iter())
            .find(|&&(c, k, a)| {
                a != action
                    && k == chord
                    && (c == context || c == Context::Global || context == Context::Global)
            })
            .map(|&(_, _, a)| a)
    }

    /// Whether `action`'s keys differ from the built-in ones.
    pub fn is_edited(&self, action: Action) -> bool {
        let chords = |map: &Self| map.keys(action).map(|(_, c)| c).collect::<Vec<_>>();
        chords(self) != chords(&Self::defaults())
    }

    /// The actions whose keys differ from the defaults, by id, for the settings file.
    pub fn overrides(&self) -> BTreeMap<String, Vec<String>> {
        (Action::known().into_iter())
            .filter(|&action| self.is_edited(action))
            .map(|action| {
                let keys = self.keys(action).map(|(_, c)| c.to_string()).collect();
                (action.id().to_owned(), keys)
            })
            .collect()
    }

    /// The defaults with `overrides` from the settings file applied; bad entries are skipped
    /// and described in the returned warnings.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> (Self, Vec<String>) {
        let known = Action::known();
        let mut map = Self::defaults();
        let mut warnings = Vec::new();
        for (id, keys) in overrides {
            let Some(&action) = known.iter().find(|a| a.id() == id) else {
                warnings.push(format!("unknown action {id:?}"));
                continue;
            };
            let mut chords = Vec::with_capacity(keys.len());
            for key in keys {
                match key.parse::<Chord>() {
                    Ok(chord) if chord.is_locked() => {
                        warnings.push(format!("{key} can't be bound"));
                    }
                    Ok(chord) => match map.holder(action, chord) {
                        Some(other) if !overrides.contains_key(other.id()) => {
                            warnings.push(format!("{key} is already {:?}", other.label()));
                        }
                        _ => chords.push(chord),
                    },
                    Err(e) => warnings.push(e),
                }
            }
            map.set(action, &chords);
        }
        (map, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MENUS;

    #[test]
    fn defaults_bind_each_chord_once_per_context_and_never_twice_over_a_pane() {
        let map = Keymap::defaults();
        for (i, (context, chord, _)) in map.bindings().enumerate() {
            let clash = map.bindings().skip(i + 1).find(|&(c, k, _)| {
                k == chord && (c == context || c == Context::Global || context == Context::Global)
            });
            assert!(clash.is_none(), "{chord} bound twice: {clash:?}");
        }
    }

    #[test]
    fn chords_ignore_how_the_terminal_spells_shift() {
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            Chord::new(KeyCode::Char('G'), shift),
            Chord::new(KeyCode::Char('G'), KeyModifiers::NONE)
        );
        assert_eq!(
            Chord::new(KeyCode::Char('F'), ctrl | shift),
            Chord::new(KeyCode::Char('f'), ctrl | shift)
        );
        assert_eq!(
            Chord::new(KeyCode::Char('F'), ctrl),
            Chord::new(KeyCode::Char('f'), ctrl | shift),
            "an uppercase letter implies Shift"
        );
        assert_ne!(
            Chord::new(KeyCode::Char('f'), ctrl),
            Chord::new(KeyCode::Char('f'), ctrl | shift)
        );
    }

    #[test]
    fn chords_print_like_the_keys_on_the_keyboard() {
        let ctrl = KeyModifiers::CONTROL;
        let none = KeyModifiers::NONE;
        let cases = [
            (
                KeyCode::Char('f'),
                ctrl | KeyModifiers::SHIFT,
                "Ctrl+Shift+F",
            ),
            (KeyCode::Char('b'), ctrl, "Ctrl+B"),
            (KeyCode::PageDown, none, "PageDown"),
            (KeyCode::Down, none, "↓"),
            (KeyCode::Char(']'), none, "]"),
            (KeyCode::Char('G'), none, "G"),
            (KeyCode::Char(' '), none, "Space"),
            (KeyCode::F(10), none, "F10"),
        ];
        for (code, mods, text) in cases {
            assert_eq!(Chord::new(code, mods).to_string(), text);
        }
    }

    #[test]
    fn every_action_has_a_key_or_a_menu_entry() {
        let map = Keymap::defaults();
        let in_menu = |action| MENUS.iter().any(|(_, items)| items.contains(&action));
        for (_, _, action) in map.bindings() {
            assert!(!action.label().is_empty(), "{action:?} has a label");
        }
        for (_, items) in MENUS {
            for &action in items.iter().filter(|&&a| a != Action::Separator) {
                assert!(map.keys(action).next().is_some() || in_menu(action));
                assert!(!action.label().is_empty(), "{action:?} has a label");
            }
        }
    }

    fn key(text: &str) -> Chord {
        text.parse().expect("a key")
    }

    #[test]
    fn every_default_key_prints_and_parses_back() {
        for (_, chord, _) in Keymap::defaults().bindings() {
            assert_eq!(key(&chord.to_string()), chord, "{chord}");
        }
        assert_eq!(
            key("ctrl+shift+f"),
            Chord::new(
                KeyCode::Char('f'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
        );
        assert_eq!(key("Down"), key("↓"));
        assert_eq!(key("+"), Chord::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(
            key("Ctrl++"),
            Chord::new(KeyCode::Char('+'), KeyModifiers::CONTROL)
        );
        assert!("Ctrl+Nope".parse::<Chord>().is_err());
        assert!("Hyper+x".parse::<Chord>().is_err());
    }

    #[test]
    fn set_add_and_reset_change_one_action() {
        let mut map = Keymap::defaults();
        map.set(Action::NextFile, &[key("m")]);
        let keys = |map: &Keymap| {
            map.keys(Action::NextFile)
                .map(|(_, c)| c.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&map), ["m"]);
        assert_eq!(map.lookup(Context::Global, key("n")), None);
        map.add(Action::NextFile, key("x"));
        assert_eq!(keys(&map), ["m", "x"]);
        assert_eq!(
            map.overrides(),
            BTreeMap::from([("next_file".into(), vec!["m".into(), "x".into()])])
        );
        map.reset(Action::NextFile);
        assert_eq!(keys(&map), ["n"]);
        assert!(map.overrides().is_empty());
    }

    #[test]
    fn holder_finds_clashes_in_the_same_pane_or_with_everywhere() {
        let map = Keymap::defaults();
        assert_eq!(
            map.holder(Action::SelectDown, key("Ctrl+B")),
            Some(Action::Sidebar)
        );
        assert_eq!(
            map.holder(Action::NextFile, key("j")),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            map.holder(Action::SelectDown, key("]")),
            None,
            "a diff key is free in the sidebar"
        );
        assert_eq!(map.holder(Action::NextFile, key("n")), None, "its own key");
    }

    #[test]
    fn with_overrides_skips_bad_entries_with_a_warning() {
        let overrides = BTreeMap::from([
            ("nope".into(), vec!["x".into()]),
            (
                "next_file".into(),
                vec!["Ctrl+Nope".into(), "Esc".into(), "q".into(), "m".into()],
            ),
        ]);
        let (map, warnings) = Keymap::with_overrides(&overrides);
        let keys: Vec<_> = map
            .keys(Action::NextFile)
            .map(|(_, c)| c.to_string())
            .collect();
        assert_eq!(keys, ["m"]);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(
            map.lookup(Context::Global, key("q")),
            Some(Action::Quit),
            "q stays Quit"
        );
    }
}

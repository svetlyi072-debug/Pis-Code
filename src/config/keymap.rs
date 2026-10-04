//! Key bindings: parsing chord strings like `ctrl+shift+s` from the config,
//! normalizing what a terminal actually sends, and mapping chords to actions.

use super::KeysConfig;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Everything a key can be bound to. Cursor-motion actions also have a
/// selecting form that needs no binding of its own: a chord with Shift
/// added (`shift+left`, `ctrl+shift+left`) selects as it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Save,
    SaveAndCheck,
    Quit,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Newline,
    Backspace,
    Delete,
    DeleteWordBackward,
    DeleteWordForward,
    Indent,
    Dedent,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveHome,
    MoveEnd,
    PageUp,
    PageDown,
    WordLeft,
    WordRight,
}

impl Action {
    /// Whether adding Shift to this action's chord turns it into a selection.
    pub fn is_motion(self) -> bool {
        matches!(
            self,
            Action::MoveLeft
                | Action::MoveRight
                | Action::MoveUp
                | Action::MoveDown
                | Action::MoveHome
                | Action::MoveEnd
                | Action::PageUp
                | Action::PageDown
                | Action::WordLeft
                | Action::WordRight
        )
    }
}

/// A key plus modifiers, in a canonical form so a binding written in the
/// config compares equal to the event a terminal produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MOD_MASK: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT)
    .union(KeyModifiers::SUPER);

impl Chord {
    /// Canonicalizes a terminal key event: letters under Ctrl/Alt are
    /// lowercased with the case folded into an explicit Shift (terminals
    /// report Ctrl+Shift+S as either `S`+Ctrl or `s`+Ctrl+Shift), and
    /// Shift+Tab (`BackTab`) drops its redundant Shift.
    pub fn from_event(key: &KeyEvent) -> Chord {
        let mut mods = key.modifiers & MOD_MASK;
        let code = match key.code {
            KeyCode::Char(c)
                if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && c.is_alphabetic() =>
            {
                if c.is_uppercase() {
                    mods |= KeyModifiers::SHIFT;
                }
                KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
            }
            KeyCode::BackTab => {
                mods -= KeyModifiers::SHIFT;
                KeyCode::BackTab
            }
            other => other,
        };
        Chord { code, mods }
    }

    pub fn without_shift(self) -> Chord {
        Chord {
            code: self.code,
            mods: self.mods - KeyModifiers::SHIFT,
        }
    }

    /// Parses `ctrl+shift+s`, `alt+backspace`, `f2`, `pageup`...
    pub fn parse(spec: &str) -> Result<Chord, String> {
        let normalized = spec.trim().to_lowercase();
        if normalized.is_empty() {
            return Err("empty key".to_string());
        }
        // "ctrl++" means Ctrl and the plus key.
        let (mod_part, key_part) = match normalized.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None => match normalized.rsplit_once('+') {
                Some((m, k)) if !k.is_empty() => (m, k),
                _ => ("", normalized.as_str()),
            },
        };

        let mut mods = KeyModifiers::empty();
        for m in mod_part.split('+').filter(|m| !m.is_empty()) {
            mods |= match m.trim() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "shift" => KeyModifiers::SHIFT,
                "alt" | "option" | "opt" => KeyModifiers::ALT,
                "super" | "cmd" | "command" | "win" => KeyModifiers::SUPER,
                other => return Err(format!("unknown modifier '{other}'")),
            };
        }

        let code = match key_part {
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "plus" => KeyCode::Char('+'),
            "minus" => KeyCode::Char('-'),
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
            f if f.len() >= 2
                && f.starts_with('f')
                && f[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                match f[1..].parse::<u8>() {
                    Ok(n) if (1..=24).contains(&n) => KeyCode::F(n),
                    _ => return Err(format!("no such function key '{f}'")),
                }
            }
            c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().unwrap()),
            other => return Err(format!("unknown key '{other}'")),
        };

        // `shift+tab` is how terminals report BackTab.
        let (code, mods) = match code {
            KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => {
                (KeyCode::BackTab, mods - KeyModifiers::SHIFT)
            }
            KeyCode::BackTab => (KeyCode::BackTab, mods - KeyModifiers::SHIFT),
            other => (other, mods),
        };
        Ok(Chord { code, mods })
    }

    /// Short form for the status-bar hint: `^S`, `^⇧S`, `F2`, `Alt+Bksp`.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push('^');
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push_str("Cmd+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push('⇧');
        }
        match self.code {
            KeyCode::Char(c) => out.extend(c.to_uppercase()),
            KeyCode::Enter => out.push_str("Enter"),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::BackTab => out.push_str("⇧Tab"),
            KeyCode::Backspace => out.push_str("Bksp"),
            KeyCode::Delete => out.push_str("Del"),
            KeyCode::Esc => out.push_str("Esc"),
            KeyCode::Left => out.push('←'),
            KeyCode::Right => out.push('→'),
            KeyCode::Up => out.push('↑'),
            KeyCode::Down => out.push('↓'),
            KeyCode::Home => out.push_str("Home"),
            KeyCode::End => out.push_str("End"),
            KeyCode::PageUp => out.push_str("PgUp"),
            KeyCode::PageDown => out.push_str("PgDn"),
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            other => out.push_str(&format!("{other:?}")),
        }
        out
    }
}

/// Chord -> action, built from the `[keys]` config table.
pub struct Keymap {
    map: HashMap<Chord, Action>,
    /// The first chord of each action, for the hint line.
    primary: HashMap<Action, Chord>,
}

impl Keymap {
    /// Builds the map, collecting a warning for every chord that doesn't
    /// parse or is already bound to another action (the first binding wins,
    /// in the order actions are listed in the config).
    pub fn from_config(keys: &KeysConfig) -> (Keymap, Vec<String>) {
        let mut map = HashMap::new();
        let mut primary = HashMap::new();
        let mut warnings = Vec::new();

        for (name, action, specs) in keys.bindings() {
            for spec in specs {
                match Chord::parse(spec) {
                    Err(why) => warnings.push(format!("keys.{name}: {why} ({spec:?})")),
                    Ok(chord) => match map.get(&chord) {
                        Some(&existing) if existing != action => warnings.push(format!(
                            "keys.{name}: {spec:?} is already bound to {}",
                            keys.name_of(existing)
                        )),
                        Some(_) => {}
                        None => {
                            map.insert(chord, action);
                            primary.entry(action).or_insert(chord);
                        }
                    },
                }
            }
        }
        (Keymap { map, primary }, warnings)
    }

    pub fn action_for(&self, chord: Chord) -> Option<Action> {
        self.map.get(&chord).copied()
    }

    /// The action for a key event, including the Shift-selects-while-moving
    /// rule: returns the action and whether it should extend the selection.
    pub fn resolve(&self, key: &KeyEvent) -> Option<(Action, bool)> {
        let chord = Chord::from_event(key);
        if let Some(action) = self.action_for(chord) {
            return Some((action, false));
        }
        if chord.mods.contains(KeyModifiers::SHIFT) {
            // `shift+left` selects; Shift+Backspace/Enter/Delete are just
            // those keys.
            if let Some(action) = self.action_for(chord.without_shift()) {
                return Some((action, action.is_motion()));
            }
        }
        None
    }

    /// How the action's first chord is written (`^S`), if it has one.
    pub fn label(&self, action: Action) -> Option<String> {
        self.primary.get(&action).map(Chord::display)
    }

    /// `^S save  ^⇧S save+check ...` built from the actual bindings, so it
    /// stays truthful after remapping.
    pub fn hint(&self) -> String {
        const SHOWN: &[(Action, &str)] = &[
            (Action::Save, "save"),
            (Action::SaveAndCheck, "save+check"),
            (Action::Quit, "quit"),
            (Action::Undo, "undo"),
            (Action::Redo, "redo"),
            (Action::SelectAll, "select all"),
            (Action::Copy, "copy"),
            (Action::Cut, "cut"),
            (Action::Paste, "paste"),
        ];
        SHOWN
            .iter()
            .filter_map(|(action, label)| {
                self.primary
                    .get(action)
                    .map(|chord| format!("{} {label}", chord.display()))
            })
            .collect::<Vec<_>>()
            .join("  ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn event(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_modifiers_keys_and_aliases() {
        assert_eq!(
            chord("ctrl+shift+s"),
            Chord {
                code: KeyCode::Char('s'),
                mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT
            }
        );
        assert_eq!(chord("Ctrl+S"), chord("control+s"));
        assert_eq!(chord("alt+backspace").code, KeyCode::Backspace);
        assert_eq!(chord("f12").code, KeyCode::F(12));
        assert_eq!(chord("pgup"), chord("pageup"));
        assert_eq!(
            chord("ctrl++"),
            Chord {
                code: KeyCode::Char('+'),
                mods: KeyModifiers::CONTROL
            }
        );
        assert_eq!(chord("space").code, KeyCode::Char(' '));
    }

    #[test]
    fn shift_tab_is_backtab() {
        let want = Chord {
            code: KeyCode::BackTab,
            mods: KeyModifiers::empty(),
        };
        assert_eq!(chord("shift+tab"), want);
        assert_eq!(chord("backtab"), want);
        // ...and a BackTab event (which terminals send with Shift set)
        // canonicalizes to the same thing.
        assert_eq!(
            Chord::from_event(&event(KeyCode::BackTab, KeyModifiers::SHIFT)),
            want
        );
    }

    #[test]
    fn rejects_nonsense_with_a_reason() {
        for bad in ["", "ctrl+", "hyper+s", "ctrl+banana", "f0", "f99"] {
            assert!(Chord::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Chord::parse("hyper+s").unwrap_err().contains("hyper"));
    }

    #[test]
    fn terminal_reports_of_ctrl_shift_letter_all_match() {
        let want = chord("ctrl+shift+s");
        // Uppercase letter with Ctrl only (no Shift bit)...
        assert_eq!(
            Chord::from_event(&event(KeyCode::Char('S'), KeyModifiers::CONTROL)),
            want
        );
        // ...lowercase with both bits...
        assert_eq!(
            Chord::from_event(&event(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            want
        );
        // ...and plain Ctrl+S is distinct.
        assert_ne!(
            Chord::from_event(&event(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            want
        );
    }

    #[test]
    fn default_bindings_resolve_like_the_old_hardcoded_ones() {
        let (km, warnings) = Keymap::from_config(&KeysConfig::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let act = |c: KeyCode, m: KeyModifiers| km.resolve(&event(c, m));
        let ctrl = KeyModifiers::CONTROL;

        assert_eq!(act(KeyCode::Char('s'), ctrl), Some((Action::Save, false)));
        assert_eq!(
            act(KeyCode::Char('S'), ctrl),
            Some((Action::SaveAndCheck, false))
        );
        assert_eq!(act(KeyCode::Char('q'), ctrl), Some((Action::Quit, false)));
        assert_eq!(
            act(KeyCode::Enter, KeyModifiers::NONE),
            Some((Action::Newline, false))
        );
        assert_eq!(
            act(KeyCode::Tab, KeyModifiers::NONE),
            Some((Action::Indent, false))
        );
        assert_eq!(
            act(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some((Action::Dedent, false))
        );
        // All four spellings of delete-word-backward.
        for (c, m) in [
            (KeyCode::Backspace, ctrl),
            (KeyCode::Backspace, KeyModifiers::ALT),
            (KeyCode::Char('h'), ctrl),
            (KeyCode::Char('w'), ctrl),
        ] {
            assert_eq!(
                act(c, m),
                Some((Action::DeleteWordBackward, false)),
                "{c:?} {m:?}"
            );
        }
        // Typing a letter is not a binding.
        assert_eq!(act(KeyCode::Char('a'), KeyModifiers::NONE), None);
        assert_eq!(act(KeyCode::Char('A'), KeyModifiers::SHIFT), None);
    }

    #[test]
    fn shift_selects_while_moving_without_extra_bindings() {
        let (km, _) = Keymap::from_config(&KeysConfig::default());
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            km.resolve(&event(KeyCode::Left, KeyModifiers::NONE)),
            Some((Action::MoveLeft, false))
        );
        assert_eq!(
            km.resolve(&event(KeyCode::Left, shift)),
            Some((Action::MoveLeft, true))
        );
        assert_eq!(
            km.resolve(&event(KeyCode::Left, ctrl | shift)),
            Some((Action::WordLeft, true))
        );
        assert_eq!(
            km.resolve(&event(KeyCode::PageDown, shift)),
            Some((Action::PageDown, true))
        );
        // Non-motion actions ignore a stray Shift rather than selecting.
        assert_eq!(
            km.resolve(&event(KeyCode::Backspace, shift)),
            Some((Action::Backspace, false))
        );
        assert_eq!(
            km.resolve(&event(KeyCode::Enter, shift)),
            Some((Action::Newline, false))
        );
    }

    #[test]
    fn remapping_replaces_an_actions_chords_and_empty_disables_it() {
        let keys = KeysConfig {
            save: vec!["f2".into()],
            quit: vec![],
            ..KeysConfig::default()
        };
        let (km, warnings) = Keymap::from_config(&keys);
        assert!(warnings.is_empty(), "{warnings:?}");
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            km.resolve(&event(KeyCode::F(2), KeyModifiers::NONE)),
            Some((Action::Save, false))
        );
        assert_eq!(
            km.resolve(&event(KeyCode::Char('s'), ctrl)),
            None,
            "old chord freed"
        );
        assert_eq!(
            km.resolve(&event(KeyCode::Char('q'), ctrl)),
            None,
            "quit unbound"
        );
    }

    #[test]
    fn bad_and_conflicting_chords_become_warnings_not_failures() {
        let keys = KeysConfig {
            copy: vec!["ctrl+banana".into(), "ctrl+c".into()],
            paste: vec!["ctrl+c".into()], // already taken by copy
            ..KeysConfig::default()
        };
        let (km, warnings) = Keymap::from_config(&keys);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("banana")));
        assert!(warnings.iter().any(|w| w.contains("already bound to copy")));
        // The valid copy chord still works, and copy keeps it.
        assert_eq!(
            km.resolve(&event(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some((Action::Copy, false))
        );
    }

    #[test]
    fn hint_is_built_from_the_real_bindings() {
        let (km, _) = Keymap::from_config(&KeysConfig::default());
        assert_eq!(
            km.hint(),
            "^S save  ^⇧S save+check  ^Q quit  ^Z undo  ^Y redo  ^A select all  ^C copy  ^X cut  ^V paste"
        );
        let remapped = KeysConfig {
            save: vec!["f2".into()],
            quit: vec![],
            ..KeysConfig::default()
        };
        let (km, _) = Keymap::from_config(&remapped);
        assert!(
            km.hint().starts_with("F2 save  ^⇧S save+check  ^Z undo"),
            "{}",
            km.hint()
        );
        assert!(
            !km.hint().contains("quit"),
            "unbound actions drop out of the hint"
        );
    }
}

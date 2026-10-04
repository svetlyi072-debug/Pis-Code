use crate::config::{Action, Keymap};
use crate::editor::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the main loop should do after a key was handled.
pub enum Outcome {
    Continue,
    Quit,
    /// A save just succeeded and a background check should start.
    TriggerCheck,
}

/// Handles one key press. `keymap` says which action (if any) the key is
/// bound to; `check_on_save` is the `check.on_save` setting.
pub fn handle_key(
    editor: &mut Editor,
    key: KeyEvent,
    keymap: &Keymap,
    check_on_save: bool,
) -> Outcome {
    let resolved = keymap.resolve(&key);

    // While a "quit with unsaved changes?" prompt is showing, only quit,
    // save and Esc mean anything.
    if editor.confirm_quit {
        match resolved {
            Some((Action::Quit, _)) => return Outcome::Quit,
            Some((Action::Save, _)) => {
                editor.save();
                if !editor.modified {
                    return Outcome::Quit;
                }
            }
            _ if key.code == KeyCode::Esc => {
                editor.confirm_quit = false;
                editor.status_message = "Cancelled".to_string();
                editor.status_is_error = false;
            }
            _ => {}
        }
        return Outcome::Continue;
    }

    let Some((action, select)) = resolved else {
        // Not a binding: ordinary typing.
        if let KeyCode::Char(c) = key.code {
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                editor.insert_char(c);
            }
        }
        return Outcome::Continue;
    };

    match action {
        Action::Save => {
            if editor.save() && check_on_save && editor.settings.check {
                return Outcome::TriggerCheck;
            }
        }
        Action::SaveAndCheck => {
            if editor.save() {
                if editor.settings.check {
                    return Outcome::TriggerCheck;
                }
                editor.status_message = if editor.language.has_checker() {
                    "Saved (checks are turned off in the config)".to_string()
                } else {
                    "Saved (no checker for this file type)".to_string()
                };
                editor.status_is_error = false;
            }
        }
        Action::Quit => {
            if editor.modified {
                editor.confirm_quit = true;
                editor.status_message = unsaved_prompt(keymap);
                editor.status_is_error = true;
            } else {
                return Outcome::Quit;
            }
        }
        Action::Undo => editor.undo(),
        Action::Redo => editor.redo(),
        Action::SelectAll => editor.select_all(),
        Action::Copy => editor.copy(),
        Action::Cut => editor.cut(),
        Action::Paste => editor.paste(),
        Action::Newline => editor.newline(),
        Action::Backspace => editor.backspace(),
        Action::Delete => editor.delete_forward(),
        Action::DeleteWordBackward => editor.delete_word_backward(),
        Action::DeleteWordForward => editor.delete_word_forward(),
        Action::Indent => editor.insert_tab(),
        Action::Dedent => editor.dedent(),
        Action::MoveLeft if select => editor.move_left_select(),
        Action::MoveLeft => editor.move_left(),
        Action::MoveRight if select => editor.move_right_select(),
        Action::MoveRight => editor.move_right(),
        Action::MoveUp if select => editor.move_up_select(),
        Action::MoveUp => editor.move_up(),
        Action::MoveDown if select => editor.move_down_select(),
        Action::MoveDown => editor.move_down(),
        Action::MoveHome if select => editor.move_home_select(),
        Action::MoveHome => editor.move_home(),
        Action::MoveEnd if select => editor.move_end_select(),
        Action::MoveEnd => editor.move_end(),
        Action::PageUp if select => editor.page_up_select(),
        Action::PageUp => editor.page_up(),
        Action::PageDown if select => editor.page_down_select(),
        Action::PageDown => editor.page_down(),
        Action::WordLeft if select => editor.move_word_left_select(),
        Action::WordLeft => editor.move_word_left(),
        Action::WordRight if select => editor.move_word_right_select(),
        Action::WordRight => editor.move_word_right(),
    }

    Outcome::Continue
}

/// The "unsaved changes" prompt, naming the keys as they're really bound.
fn unsaved_prompt(keymap: &Keymap) -> String {
    let quit = keymap.label(Action::Quit).unwrap_or_else(|| "quit".into());
    let mut msg = format!("Unsaved changes! {quit} again to discard");
    if let Some(save) = keymap.label(Action::Save) {
        msg.push_str(&format!(", {save} to save"));
    }
    msg.push_str(", Esc to cancel");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeysConfig;
    use std::path::PathBuf;

    fn keymap() -> Keymap {
        Keymap::from_config(&KeysConfig::default()).0
    }

    fn editor_with(text: &str) -> Editor {
        let mut ed = Editor::open(PathBuf::from("__input_test__.txt")).unwrap();
        for c in text.chars() {
            ed.insert_char(c);
        }
        ed
    }

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn handle(ed: &mut Editor, key: KeyEvent) -> Outcome {
        handle_key(ed, key, &keymap(), false)
    }

    #[test]
    fn every_encoding_of_delete_word_backward_works() {
        for (name, key) in [
            (
                "Ctrl+Backspace (kitty protocol)",
                press(KeyCode::Backspace, KeyModifiers::CONTROL),
            ),
            (
                "Ctrl+H (byte 0x08, e.g. xterm.js)",
                press(KeyCode::Char('h'), KeyModifiers::CONTROL),
            ),
            (
                "Ctrl+W (shell convention)",
                press(KeyCode::Char('w'), KeyModifiers::CONTROL),
            ),
            (
                "Alt+Backspace (ESC DEL)",
                press(KeyCode::Backspace, KeyModifiers::ALT),
            ),
        ] {
            let mut ed = editor_with("foo bar");
            handle(&mut ed, key);
            assert_eq!(ed.lines[0], "foo ", "{name}");
        }
    }

    #[test]
    fn plain_backspace_still_deletes_one_character() {
        let mut ed = editor_with("foo bar");
        handle(&mut ed, press(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ed.lines[0], "foo ba");
    }

    #[test]
    fn unbound_printable_keys_are_typed_and_unbound_chords_are_ignored() {
        let mut ed = editor_with("");
        handle(&mut ed, press(KeyCode::Char('x'), KeyModifiers::NONE));
        handle(&mut ed, press(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        handle(&mut ed, press(KeyCode::Char('k'), KeyModifiers::CONTROL)); // unbound chord
        handle(&mut ed, press(KeyCode::F(9), KeyModifiers::NONE)); // unbound key
        assert_eq!(ed.lines[0], "xY");
    }

    #[test]
    fn shift_with_a_motion_selects() {
        let mut ed = editor_with("hello");
        ed.move_home();
        handle(&mut ed, press(KeyCode::Right, KeyModifiers::SHIFT));
        handle(&mut ed, press(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(ed.has_selection());
        assert_eq!(ed.cursor_col, 2);
        handle(&mut ed, press(KeyCode::Right, KeyModifiers::NONE));
        assert!(!ed.has_selection(), "plain motion drops the selection");
    }

    #[test]
    fn a_remapped_key_works_and_the_old_one_stops() {
        let keys = KeysConfig {
            undo: vec!["f5".into()],
            ..KeysConfig::default()
        };
        let km = Keymap::from_config(&keys).0;
        let mut ed = editor_with("abc");
        handle_key(
            &mut ed,
            press(KeyCode::Char('z'), KeyModifiers::CONTROL),
            &km,
            false,
        );
        assert_eq!(ed.lines[0], "abc", "Ctrl+Z no longer undoes");
        handle_key(
            &mut ed,
            press(KeyCode::F(5), KeyModifiers::NONE),
            &km,
            false,
        );
        assert_eq!(ed.lines[0], "", "F5 does");
    }

    fn ctrl(c: char) -> KeyEvent {
        press(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn save_triggers_a_check_only_when_on_save_is_set() {
        let path = std::env::temp_dir().join(format!("pc_input_save_{}.rs", std::process::id()));
        let mk = || {
            let mut ed = Editor::open(path.clone()).unwrap();
            ed.insert_char('x');
            ed
        };
        let km = keymap();

        let mut ed = mk();
        assert!(matches!(
            handle_key(&mut ed, ctrl('s'), &km, false),
            Outcome::Continue
        ));
        let mut ed = mk();
        assert!(matches!(
            handle_key(&mut ed, ctrl('s'), &km, true),
            Outcome::TriggerCheck
        ));
        // Ctrl+Shift+S always checks (when a checker exists), on_save or not.
        let mut ed = mk();
        let ctrl_shift_s = press(KeyCode::Char('S'), KeyModifiers::CONTROL);
        assert!(matches!(
            handle_key(&mut ed, ctrl_shift_s, &km, false),
            Outcome::TriggerCheck
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_and_check_explains_when_there_is_nothing_to_run() {
        let path = std::env::temp_dir().join(format!("pc_input_nochk_{}.txt", std::process::id()));
        let mut ed = Editor::open(path.clone()).unwrap();
        ed.insert_char('x');
        let ctrl_shift_s = press(KeyCode::Char('S'), KeyModifiers::CONTROL);
        let out = handle_key(&mut ed, ctrl_shift_s, &keymap(), false);
        assert!(matches!(out, Outcome::Continue));
        assert!(
            ed.status_message.contains("no checker"),
            "{}",
            ed.status_message
        );

        // A language that has a checker, with checks switched off.
        let rs = std::env::temp_dir().join(format!("pc_input_off_{}.rs", std::process::id()));
        let mut config = crate::config::Config::default();
        config.check.enabled = false;
        let lang = crate::language::Language::Rust;
        let mut ed = Editor::open_with(
            rs.clone(),
            lang,
            crate::config::Settings::resolve(&config, lang),
        )
        .unwrap();
        ed.insert_char('x');
        handle_key(&mut ed, ctrl_shift_s, &keymap(), false);
        assert!(
            ed.status_message.contains("turned off"),
            "{}",
            ed.status_message
        );
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(rs);
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_using_the_real_key_names() {
        let km = keymap();
        let mut ed = editor_with("x");
        assert!(matches!(
            handle_key(&mut ed, ctrl('q'), &km, false),
            Outcome::Continue
        ));
        assert!(ed.confirm_quit);
        assert_eq!(
            ed.status_message,
            "Unsaved changes! ^Q again to discard, ^S to save, Esc to cancel"
        );
        // Esc backs out; the second Ctrl+Q discards.
        handle_key(&mut ed, press(KeyCode::Esc, KeyModifiers::NONE), &km, false);
        assert!(!ed.confirm_quit);
        handle_key(&mut ed, ctrl('q'), &km, false);
        assert!(matches!(
            handle_key(&mut ed, ctrl('q'), &km, false),
            Outcome::Quit
        ));

        // Remapped: the prompt names the new keys, and they work.
        let keys = KeysConfig {
            quit: vec!["f10".into()],
            save: vec!["f2".into()],
            ..KeysConfig::default()
        };
        let km = Keymap::from_config(&keys).0;
        let mut ed = editor_with("x");
        handle_key(
            &mut ed,
            press(KeyCode::F(10), KeyModifiers::NONE),
            &km,
            false,
        );
        assert_eq!(
            ed.status_message,
            "Unsaved changes! F10 again to discard, F2 to save, Esc to cancel"
        );
        assert!(matches!(
            handle_key(
                &mut ed,
                press(KeyCode::F(10), KeyModifiers::NONE),
                &km,
                false
            ),
            Outcome::Quit
        ));
    }

    #[test]
    fn a_clean_buffer_quits_immediately() {
        let mut ed = editor_with("");
        ed.modified = false;
        assert!(matches!(handle(&mut ed, ctrl('q')), Outcome::Quit));
    }
}

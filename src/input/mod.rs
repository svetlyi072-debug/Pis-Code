use crate::editor::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub enum Action {
    Continue,
    Quit,
    /// A save just succeeded; the caller should kick off a background
    /// Ctrl+S diagnostics check.
    TriggerCheck,
}

const PAGE_SIZE: usize = 20;

pub fn handle_key(editor: &mut Editor, key: KeyEvent) -> Action {
    // While a "quit with unsaved changes?" prompt is showing, only a small
    // set of keys are meaningful.
    if editor.confirm_quit {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Quit;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                editor.save();
                if !editor.modified {
                    return Action::Quit;
                }
            }
            KeyCode::Esc => {
                editor.confirm_quit = false;
                editor.status_message = "Cancelled".to_string();
                editor.status_is_error = false;
            }
            _ => {}
        }
        return Action::Continue;
    }

    match (key.code, key.modifiers) {
        // Ctrl+Shift+S: save and also run the Ctrl+S-style background
        // diagnostics check. Plain Ctrl+S just saves. Terminals vary in
        // whether they report the Shift as a modifier bit or as the
        // uppercase char itself, so both count.
        (KeyCode::Char(c @ ('s' | 'S')), m)
            if m.contains(KeyModifiers::CONTROL)
                && (m.contains(KeyModifiers::SHIFT) || c == 'S') =>
        {
            if editor.save() {
                if !editor.language.has_checker() {
                    editor.status_message = "Saved (no checker for this file type)".to_string();
                    editor.status_is_error = false;
                } else {
                    return Action::TriggerCheck;
                }
            }
        }
        (KeyCode::Char('s'), m) if m.contains(KeyModifiers::CONTROL) => {
            editor.save();
        }
        (KeyCode::Char('q'), m) if m.contains(KeyModifiers::CONTROL) => {
            if editor.modified {
                editor.confirm_quit = true;
                editor.status_message =
                    "Unsaved changes! Ctrl+Q again to discard, Ctrl+S to save, Esc to cancel"
                        .to_string();
                editor.status_is_error = true;
            } else {
                return Action::Quit;
            }
        }
        (KeyCode::Char('z'), m) if m.contains(KeyModifiers::CONTROL) => editor.undo(),
        (KeyCode::Char('y'), m) if m.contains(KeyModifiers::CONTROL) => editor.redo(),

        (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => editor.select_all(),
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => editor.copy(),
        (KeyCode::Char('x'), m) if m.contains(KeyModifiers::CONTROL) => editor.cut(),
        (KeyCode::Char('v'), m) if m.contains(KeyModifiers::CONTROL) => editor.paste(),

        (KeyCode::Enter, _) => editor.newline(),
        // "Delete word backward" arrives in different forms depending on
        // the terminal: a real Ctrl+Backspace only where the Kitty keyboard
        // protocol is active (Ghostty, Kitty, WezTerm); as Ctrl+H (byte
        // 0x08) in terminals like xterm.js; as Alt+Backspace (ESC DEL)
        // from readline-style setups; and as Ctrl+W, the classic shell
        // binding. Accept all of them.
        (KeyCode::Backspace, m)
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
        {
            editor.delete_word_backward()
        }
        (KeyCode::Char('h' | 'w'), m) if m.contains(KeyModifiers::CONTROL) => {
            editor.delete_word_backward()
        }
        (KeyCode::Backspace, _) => editor.backspace(),
        (KeyCode::Delete, m) if m.contains(KeyModifiers::CONTROL) => editor.delete_word_forward(),
        (KeyCode::Delete, _) => editor.delete_forward(),
        (KeyCode::Tab, _) => editor.insert_tab(),
        (KeyCode::BackTab, _) => editor.dedent(),

        (KeyCode::Left, m)
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            editor.move_word_left_select()
        }
        (KeyCode::Left, m) if m.contains(KeyModifiers::CONTROL) => editor.move_word_left(),
        (KeyCode::Left, m) if m.contains(KeyModifiers::SHIFT) => editor.move_left_select(),
        (KeyCode::Left, _) => editor.move_left(),
        (KeyCode::Right, m)
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            editor.move_word_right_select()
        }
        (KeyCode::Right, m) if m.contains(KeyModifiers::CONTROL) => editor.move_word_right(),
        (KeyCode::Right, m) if m.contains(KeyModifiers::SHIFT) => editor.move_right_select(),
        (KeyCode::Right, _) => editor.move_right(),
        (KeyCode::Up, m) if m.contains(KeyModifiers::SHIFT) => editor.move_up_select(),
        (KeyCode::Up, _) => editor.move_up(),
        (KeyCode::Down, m) if m.contains(KeyModifiers::SHIFT) => editor.move_down_select(),
        (KeyCode::Down, _) => editor.move_down(),
        (KeyCode::Home, m) if m.contains(KeyModifiers::SHIFT) => editor.move_home_select(),
        (KeyCode::Home, _) => editor.move_home(),
        (KeyCode::End, m) if m.contains(KeyModifiers::SHIFT) => editor.move_end_select(),
        (KeyCode::End, _) => editor.move_end(),
        (KeyCode::PageUp, m) if m.contains(KeyModifiers::SHIFT) => editor.page_up_select(PAGE_SIZE),
        (KeyCode::PageUp, _) => editor.page_up(PAGE_SIZE),
        (KeyCode::PageDown, m) if m.contains(KeyModifiers::SHIFT) => {
            editor.page_down_select(PAGE_SIZE)
        }
        (KeyCode::PageDown, _) => editor.page_down(PAGE_SIZE),

        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            editor.insert_char(c);
        }
        _ => {}
    }

    Action::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
            handle_key(&mut ed, key);
            assert_eq!(ed.lines[0], "foo ", "{name}");
        }
    }

    #[test]
    fn plain_backspace_still_deletes_one_character() {
        let mut ed = editor_with("foo bar");
        handle_key(&mut ed, press(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ed.lines[0], "foo ba");
    }
}

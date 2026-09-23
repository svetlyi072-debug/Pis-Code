use crate::editor::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub enum Action {
    Continue,
    Quit,
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

        (KeyCode::Enter, _) => editor.newline(),
        (KeyCode::Backspace, _) => editor.backspace(),
        (KeyCode::Delete, _) => editor.delete_forward(),
        (KeyCode::Tab, _) => editor.insert_tab(),

        (KeyCode::Left, _) => editor.move_left(),
        (KeyCode::Right, _) => editor.move_right(),
        (KeyCode::Up, _) => editor.move_up(),
        (KeyCode::Down, _) => editor.move_down(),
        (KeyCode::Home, _) => editor.move_home(),
        (KeyCode::End, _) => editor.move_end(),
        (KeyCode::PageUp, _) => editor.page_up(PAGE_SIZE),
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

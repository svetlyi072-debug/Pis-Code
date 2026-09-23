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

        (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => editor.select_all(),
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => editor.copy(),
        (KeyCode::Char('x'), m) if m.contains(KeyModifiers::CONTROL) => editor.cut(),
        (KeyCode::Char('v'), m) if m.contains(KeyModifiers::CONTROL) => editor.paste(),

        (KeyCode::Enter, _) => editor.newline(),
        (KeyCode::Backspace, m) if m.contains(KeyModifiers::CONTROL) => {
            editor.delete_word_backward()
        }
        (KeyCode::Backspace, _) => editor.backspace(),
        (KeyCode::Delete, m) if m.contains(KeyModifiers::CONTROL) => editor.delete_word_forward(),
        (KeyCode::Delete, _) => editor.delete_forward(),
        (KeyCode::Tab, _) => editor.insert_tab(),

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

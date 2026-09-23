use crate::file_io;
use anyhow::Result;
use std::path::PathBuf;

const INDENT: &str = "    ";

/// What kind of edit was last performed, used to decide whether a new
/// keystroke should be coalesced into the current undo step or start a
/// fresh one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditGroup {
    None,
    Insert,
    Delete,
    Newline,
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

pub struct Editor {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub file_path: PathBuf,
    pub modified: bool,

    /// Top-left visible cell, in (line, char) space.
    pub scroll: usize,
    pub col_scroll: usize,

    pub confirm_quit: bool,
    pub status_message: String,
    pub status_is_error: bool,

    desired_col: usize,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    last_edit_group: EditGroup,
}

impl Editor {
    pub fn open(path: PathBuf) -> Result<Self> {
        let lines = file_io::load_or_create(&path)?;
        Ok(Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            file_path: path,
            modified: false,
            scroll: 0,
            col_scroll: 0,
            confirm_quit: false,
            status_message: String::from("Ctrl+S save  Ctrl+Q quit  Ctrl+Z undo  Ctrl+Y redo"),
            status_is_error: false,
            desired_col: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_group: EditGroup::None,
        })
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn current_line(&self) -> &str {
        &self.lines[self.cursor_row]
    }

    fn current_line_char_len(&self) -> usize {
        self.current_line().chars().count()
    }

    fn char_before_cursor(&self) -> Option<char> {
        if self.cursor_col == 0 {
            None
        } else {
            self.current_line().chars().nth(self.cursor_col - 1)
        }
    }

    fn char_at_cursor(&self) -> Option<char> {
        self.current_line().chars().nth(self.cursor_col)
    }

    fn byte_idx(line: &str, char_idx: usize) -> usize {
        line.char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    fn leading_whitespace(line: &str) -> String {
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    // ---- low level buffer mutation (no undo bookkeeping) ----

    fn insert_raw(&mut self, c: char) {
        let row = self.cursor_row;
        let bi = Self::byte_idx(&self.lines[row], self.cursor_col);
        self.lines[row].insert(bi, c);
        self.cursor_col += 1;
    }

    fn remove_chars(&mut self, row: usize, start: usize, end: usize) {
        let line = &self.lines[row];
        let bstart = Self::byte_idx(line, start);
        let bend = Self::byte_idx(line, end);
        self.lines[row].replace_range(bstart..bend, "");
    }

    // ---- undo bookkeeping ----

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
        }
    }

    fn begin_edit(&mut self, group: EditGroup) {
        if self.last_edit_group != group {
            self.undo_stack.push(self.snapshot());
            self.redo_stack.clear();
            self.last_edit_group = group;
        }
    }

    /// Breaks undo coalescing; call this on cursor moves and other
    /// non-edit actions so the next edit starts a fresh undo step.
    fn reset_edit_group(&mut self) {
        self.last_edit_group = EditGroup::None;
    }

    pub fn undo(&mut self) {
        if let Some(state) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.lines = state.lines;
            self.cursor_row = state.cursor_row.min(self.lines.len().saturating_sub(1));
            self.cursor_col = state.cursor_col;
            self.clamp_cursor();
            self.last_edit_group = EditGroup::None;
            self.modified = true;
            self.status_message = "Undo".to_string();
            self.status_is_error = false;
        } else {
            self.status_message = "Nothing to undo".to_string();
            self.status_is_error = false;
        }
    }

    pub fn redo(&mut self) {
        if let Some(state) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.lines = state.lines;
            self.cursor_row = state.cursor_row.min(self.lines.len().saturating_sub(1));
            self.cursor_col = state.cursor_col;
            self.clamp_cursor();
            self.last_edit_group = EditGroup::None;
            self.modified = true;
            self.status_message = "Redo".to_string();
            self.status_is_error = false;
        } else {
            self.status_message = "Nothing to redo".to_string();
            self.status_is_error = false;
        }
    }

    fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len() - 1;
        }
        let len = self.current_line_char_len();
        if self.cursor_col > len {
            self.cursor_col = len;
        }
    }

    // ---- editing ----

    /// Insert a typed character, applying VS Code-style smart bracket
    /// pairing for `{`, `(`, `[` and type-over for their closers.
    pub fn insert_char(&mut self, c: char) {
        self.confirm_quit = false;
        match c {
            '{' | '(' | '[' => {
                self.begin_edit(EditGroup::Insert);
                let close = match c {
                    '{' => '}',
                    '(' => ')',
                    '[' => ']',
                    _ => unreachable!(),
                };
                self.insert_raw(c);
                self.insert_raw(close);
                self.cursor_col -= 1;
            }
            '}' | ')' | ']' if self.char_at_cursor() == Some(c) => {
                // Type over an already-present closing bracket instead of
                // inserting a duplicate.
                self.begin_edit(EditGroup::Insert);
                self.cursor_col += 1;
            }
            _ => {
                self.begin_edit(EditGroup::Insert);
                self.insert_raw(c);
            }
        }
        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    pub fn insert_tab(&mut self) {
        self.confirm_quit = false;
        self.begin_edit(EditGroup::Insert);
        for _ in 0..INDENT.len() {
            self.insert_raw(' ');
        }
        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    /// Enter key: implements the "smart" C#-block split. If the cursor
    /// sits directly between `{` and `}`, expand into three lines with the
    /// closing brace re-indented and an empty, deeper-indented line for the
    /// cursor. Otherwise, do a normal newline that copies (and possibly
    /// deepens) the current line's indentation.
    pub fn newline(&mut self) {
        self.confirm_quit = false;
        self.begin_edit(EditGroup::Newline);

        let row = self.cursor_row;
        let line = self.lines[row].clone();
        let indent = Self::leading_whitespace(&line);

        let before = self.char_before_cursor();
        let after = self.char_at_cursor();

        if before == Some('{') && after == Some('}') {
            let bi = Self::byte_idx(&line, self.cursor_col);
            let left = line[..bi].to_string();
            let right = line[bi..].to_string();
            let inner_indent = format!("{indent}{INDENT}");

            self.lines[row] = left;
            self.lines.insert(row + 1, inner_indent.clone());
            self.lines.insert(row + 2, format!("{indent}{right}"));

            self.cursor_row = row + 1;
            self.cursor_col = inner_indent.chars().count();
        } else {
            let bi = Self::byte_idx(&line, self.cursor_col);
            let left = line[..bi].to_string();
            let right = line[bi..].to_string();

            let trimmed_left = left.trim_end();
            let deepen = trimmed_left.ends_with('{')
                || trimmed_left.ends_with('(')
                || trimmed_left.ends_with('[');

            let new_indent = if deepen {
                format!("{indent}{INDENT}")
            } else {
                indent
            };

            self.lines[row] = left;
            self.lines.insert(row + 1, format!("{new_indent}{right}"));

            self.cursor_row = row + 1;
            self.cursor_col = new_indent.chars().count();
        }

        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    /// Backspace: deletes an adjacent auto-paired `{}`/`()`/`[]` as a unit,
    /// otherwise removes one character, otherwise merges with the previous
    /// line.
    pub fn backspace(&mut self) {
        self.confirm_quit = false;
        if self.cursor_col == 0 {
            if self.cursor_row == 0 {
                return;
            }
            self.begin_edit(EditGroup::Delete);
            let row = self.cursor_row;
            let current = self.lines.remove(row);
            let prev_len = self.lines[row - 1].chars().count();
            self.lines[row - 1].push_str(&current);
            self.cursor_row = row - 1;
            self.cursor_col = prev_len;
        } else {
            self.begin_edit(EditGroup::Delete);
            let before = self.char_before_cursor();
            let after = self.char_at_cursor();
            let is_pair = matches!(
                (before, after),
                (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))
            );
            if is_pair {
                let row = self.cursor_row;
                self.remove_chars(row, self.cursor_col - 1, self.cursor_col + 1);
                self.cursor_col -= 1;
            } else {
                let row = self.cursor_row;
                self.remove_chars(row, self.cursor_col - 1, self.cursor_col);
                self.cursor_col -= 1;
            }
        }
        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    /// Delete key: removes the character under the cursor, or merges with
    /// the next line if at end of line.
    pub fn delete_forward(&mut self) {
        self.confirm_quit = false;
        let len = self.current_line_char_len();
        if self.cursor_col >= len {
            if self.cursor_row + 1 >= self.lines.len() {
                return;
            }
            self.begin_edit(EditGroup::Delete);
            let row = self.cursor_row;
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
        } else {
            self.begin_edit(EditGroup::Delete);
            let row = self.cursor_row;
            self.remove_chars(row, self.cursor_col, self.cursor_col + 1);
        }
        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    // ---- cursor movement ----

    pub fn move_left(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_char_len();
        }
        self.desired_col = self.cursor_col;
    }

    pub fn move_right(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        let len = self.current_line_char_len();
        if self.cursor_col < len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
        self.desired_col = self.cursor_col;
    }

    pub fn move_up(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.desired_col.min(self.current_line_char_len());
        }
    }

    pub fn move_down(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.desired_col.min(self.current_line_char_len());
        }
    }

    pub fn move_home(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        self.cursor_col = 0;
        self.desired_col = 0;
    }

    pub fn move_end(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        self.cursor_col = self.current_line_char_len();
        self.desired_col = self.cursor_col;
    }

    pub fn page_up(&mut self, page: usize) {
        self.confirm_quit = false;
        self.reset_edit_group();
        self.cursor_row = self.cursor_row.saturating_sub(page);
        self.cursor_col = self.desired_col.min(self.current_line_char_len());
    }

    pub fn page_down(&mut self, page: usize) {
        self.confirm_quit = false;
        self.reset_edit_group();
        self.cursor_row = (self.cursor_row + page).min(self.lines.len() - 1);
        self.cursor_col = self.desired_col.min(self.current_line_char_len());
    }

    pub fn ensure_visible(&mut self, width: usize, height: usize) {
        if height > 0 {
            if self.cursor_row < self.scroll {
                self.scroll = self.cursor_row;
            } else if self.cursor_row >= self.scroll + height {
                self.scroll = self.cursor_row + 1 - height;
            }
        }
        if width > 0 {
            if self.cursor_col < self.col_scroll {
                self.col_scroll = self.cursor_col;
            } else if self.cursor_col >= self.col_scroll + width {
                self.col_scroll = self.cursor_col + 1 - width;
            }
        }
    }

    // ---- persistence ----

    pub fn save(&mut self) {
        match file_io::save(&self.file_path, &self.lines) {
            Ok(()) => {
                self.modified = false;
                self.confirm_quit = false;
                self.status_message = format!("Saved {}", self.file_path.display());
                self.status_is_error = false;
            }
            Err(e) => {
                self.status_message = format!("Save failed: {e}");
                self.status_is_error = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_editor() -> Editor {
        Editor::open(PathBuf::from("__test_buffer__.cs")).unwrap()
    }

    fn type_str(ed: &mut Editor, s: &str) {
        for c in s.chars() {
            ed.insert_char(c);
        }
    }

    #[test]
    fn curly_brace_autocloses() {
        let mut ed = new_editor();
        ed.insert_char('{');
        assert_eq!(ed.lines[0], "{}");
        assert_eq!(ed.cursor_col, 1);
    }

    #[test]
    fn paren_and_bracket_autoclose() {
        let mut ed = new_editor();
        ed.insert_char('(');
        assert_eq!(ed.lines[0], "()");
        assert_eq!(ed.cursor_col, 1);

        let mut ed = new_editor();
        ed.insert_char('[');
        assert_eq!(ed.lines[0], "[]");
        assert_eq!(ed.cursor_col, 1);
    }

    #[test]
    fn typing_closer_types_over_instead_of_duplicating() {
        let mut ed = new_editor();
        ed.insert_char('(');
        // cursor is between ( and ), typing ')' should just move over it
        ed.insert_char(')');
        assert_eq!(ed.lines[0], "()");
        assert_eq!(ed.cursor_col, 2);
    }

    #[test]
    fn smart_enter_splits_curly_block_into_three_lines() {
        let mut ed = new_editor();
        type_str(&mut ed, "void Foo() {");
        // cursor sits between the auto-inserted { and }
        assert_eq!(ed.lines[0], "void Foo() {}");
        ed.newline();

        assert_eq!(ed.lines.len(), 3);
        assert_eq!(ed.lines[0], "void Foo() {");
        assert_eq!(ed.lines[1], "    ");
        assert_eq!(ed.lines[2], "}");
        assert_eq!(ed.cursor_row, 1);
        assert_eq!(ed.cursor_col, 4);
    }

    #[test]
    fn smart_enter_respects_existing_indent() {
        let mut ed = new_editor();
        type_str(&mut ed, "class C ");
        ed.insert_char('{');
        ed.newline(); // -> class C {\n    \n}
                      // now inside the class body, open a method block at one level deep
        type_str(&mut ed, "void Foo() ");
        ed.insert_char('{');
        ed.newline();

        assert_eq!(
            ed.lines,
            vec![
                "class C {".to_string(),
                "    void Foo() {".to_string(),
                "        ".to_string(),
                "    }".to_string(),
                "}".to_string(),
            ]
        );
        assert_eq!(ed.cursor_row, 2);
        assert_eq!(ed.cursor_col, 8);
    }

    #[test]
    fn plain_enter_autoindents_without_splitting() {
        let mut ed = new_editor();
        type_str(&mut ed, "    int x = 1;");
        ed.newline();
        assert_eq!(ed.lines[0], "    int x = 1;");
        assert_eq!(ed.lines[1], "    ");
        assert_eq!(ed.cursor_col, 4);
    }

    #[test]
    fn enter_after_open_paren_without_matching_close_just_deepens_indent() {
        let mut ed = new_editor();
        type_str(&mut ed, "foo(");
        ed.move_right(); // step past the auto-inserted ')'
        ed.backspace(); // remove it, simulating no matching closer present
        ed.newline();
        assert_eq!(ed.lines[0], "foo(");
        assert_eq!(ed.lines[1], "    ");
    }

    #[test]
    fn backspace_deletes_empty_pair_as_unit() {
        let mut ed = new_editor();
        ed.insert_char('{');
        ed.backspace();
        assert_eq!(ed.lines[0], "");
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn backspace_merges_with_previous_line() {
        let mut ed = new_editor();
        type_str(&mut ed, "abc");
        ed.newline();
        type_str(&mut ed, "def");
        ed.move_home();
        ed.backspace();
        assert_eq!(ed.lines.len(), 1);
        assert_eq!(ed.lines[0], "abcdef");
        assert_eq!(ed.cursor_col, 3);
    }

    #[test]
    fn undo_redo_roundtrip_for_typing() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello");
        assert_eq!(ed.lines[0], "hello");
        ed.undo();
        assert_eq!(ed.lines[0], "");
        ed.redo();
        assert_eq!(ed.lines[0], "hello");
    }

    #[test]
    fn undo_restores_pre_smart_enter_state() {
        let mut ed = new_editor();
        type_str(&mut ed, "void Foo() {");
        ed.newline();
        assert_eq!(ed.lines.len(), 3);
        ed.undo();
        assert_eq!(ed.lines.len(), 1);
        assert_eq!(ed.lines[0], "void Foo() {}");
    }

    #[test]
    fn tab_inserts_four_spaces() {
        let mut ed = new_editor();
        ed.insert_tab();
        assert_eq!(ed.lines[0], "    ");
        assert_eq!(ed.cursor_col, 4);
    }

    #[test]
    fn delete_forward_merges_next_line() {
        let mut ed = new_editor();
        type_str(&mut ed, "abc");
        ed.newline();
        type_str(&mut ed, "def");
        ed.move_up();
        ed.move_end();
        ed.delete_forward();
        assert_eq!(ed.lines.len(), 1);
        assert_eq!(ed.lines[0], "abcdef");
    }
}

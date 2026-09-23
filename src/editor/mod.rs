use crate::file_io;
use anyhow::Result;
use std::ops::Range;
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
    Paste,
}

// The real OS clipboard is global, external, mutable state shared with
// everything else on the machine — reading/writing it from unit tests
// would make them flaky (parallel test threads racing on it, or picking up
// whatever a developer last copied). Tests always go through the in-app
// `Editor::clipboard` fallback instead; the real integration is exercised
// manually.
#[cfg(not(test))]
fn set_system_clipboard(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(text.to_string())?;
    Ok(())
}

#[cfg(test)]
fn set_system_clipboard(_text: &str) -> Result<()> {
    anyhow::bail!("system clipboard disabled in tests")
}

#[cfg(not(test))]
fn get_system_clipboard() -> Result<String> {
    let mut cb = arboard::Clipboard::new()?;
    Ok(cb.get_text()?)
}

#[cfg(test)]
fn get_system_clipboard() -> Result<String> {
    anyhow::bail!("system clipboard disabled in tests")
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

    /// The other end of an active selection; `None` means no selection.
    /// The current cursor position is always the live end.
    pub selection_anchor: Option<(usize, usize)>,
    clipboard: String,

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
            status_message: String::from(
                "^S save  ^Q quit  ^Z undo  ^Y redo  ^A select all  ^C copy  ^X cut  ^V paste",
            ),
            status_is_error: false,
            selection_anchor: None,
            clipboard: String::new(),
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

    // ---- selection ----

    /// Returns `(start, end)` in row-major order, or `None` if there is no
    /// active (non-empty) selection.
    fn selection_bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let cursor = (self.cursor_row, self.cursor_col);
        if anchor == cursor {
            return None;
        }
        Some(if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    pub fn has_selection(&self) -> bool {
        self.selection_bounds().is_some()
    }

    /// The selected character-column range within `row`, for rendering.
    pub fn selection_col_range(&self, row: usize) -> Option<Range<usize>> {
        let (start, end) = self.selection_bounds()?;
        if row < start.0 || row > end.0 {
            return None;
        }
        let line_len = self.lines.get(row)?.chars().count();
        let s = if row == start.0 { start.1 } else { 0 };
        let e = if row == end.0 { end.1 } else { line_len };
        Some(s..e)
    }

    fn extract_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        if start.0 == end.0 {
            let line = &self.lines[start.0];
            let bs = Self::byte_idx(line, start.1);
            let be = Self::byte_idx(line, end.1);
            line[bs..be].to_string()
        } else {
            let mut out = String::new();
            let first = &self.lines[start.0];
            out.push_str(&first[Self::byte_idx(first, start.1)..]);
            for r in start.0 + 1..end.0 {
                out.push('\n');
                out.push_str(&self.lines[r]);
            }
            out.push('\n');
            let last = &self.lines[end.0];
            out.push_str(&last[..Self::byte_idx(last, end.1)]);
            out
        }
    }

    /// Deletes the active selection as part of edit `group`, moving the
    /// cursor to where the selection started. Returns `false` if there was
    /// nothing selected.
    fn delete_selection_as(&mut self, group: EditGroup) -> bool {
        let Some((start, end)) = self.selection_bounds() else {
            return false;
        };
        self.begin_edit(group);
        if start.0 == end.0 {
            self.remove_chars(start.0, start.1, end.1);
        } else {
            let end_line_tail = {
                let last = &self.lines[end.0];
                last[Self::byte_idx(last, end.1)..].to_string()
            };
            let bstart = Self::byte_idx(&self.lines[start.0], start.1);
            self.lines[start.0].truncate(bstart);
            self.lines[start.0].push_str(&end_line_tail);
            self.lines.drain(start.0 + 1..=end.0);
        }
        self.cursor_row = start.0;
        self.cursor_col = start.1;
        self.selection_anchor = None;
        self.modified = true;
        true
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

    /// Counts (unescaped) occurrences of `quote` before the cursor on the
    /// current line — an odd count means we're sitting inside an open
    /// string, which is how we tell "close this string" apart from "open a
    /// new one" when the same character is used for both ends.
    fn quote_count_before_cursor(&self, quote: char) -> usize {
        let mut count = 0;
        let mut escaped = false;
        for (i, ch) in self.current_line().chars().enumerate() {
            if i >= self.cursor_col {
                break;
            }
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                count += 1;
            }
        }
        count
    }

    /// Insert a typed character, applying VS Code-style smart pairing for
    /// `{`, `(`, `[`, `"`, `'` and type-over for their closers.
    pub fn insert_char(&mut self, c: char) {
        self.confirm_quit = false;
        if self.has_selection() {
            self.delete_selection_as(EditGroup::Insert);
        }
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
            '"' | '\'' => {
                self.begin_edit(EditGroup::Insert);
                if self.char_at_cursor() == Some(c) {
                    // Type over an already-present closing quote.
                    self.cursor_col += 1;
                } else if self.quote_count_before_cursor(c) % 2 == 1 {
                    // We're inside an open string; this closes it, so
                    // don't also autoclose a fresh pair.
                    self.insert_raw(c);
                } else {
                    self.insert_raw(c);
                    self.insert_raw(c);
                    self.cursor_col -= 1;
                }
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
        if self.has_selection() {
            self.delete_selection_as(EditGroup::Insert);
        }
        self.begin_edit(EditGroup::Insert);
        for _ in 0..INDENT.len() {
            self.insert_raw(' ');
        }
        self.desired_col = self.cursor_col;
        self.modified = true;
    }

    /// Enter key: implements the "smart" C#-block split, Allman-style (the
    /// way Visual Studio formats C#). If the cursor sits directly between
    /// `{` and `}`, expand into four lines: the code before `{` stays on
    /// its own line, the opening brace moves to its own line at the same
    /// indentation, an empty line one level deeper is inserted for the
    /// cursor, and the closing brace goes on its own line matching the
    /// opening brace's indentation. Otherwise, do a normal newline that
    /// copies (and possibly deepens) the current line's indentation.
    pub fn newline(&mut self) {
        self.confirm_quit = false;
        if self.has_selection() {
            self.delete_selection_as(EditGroup::Newline);
        }
        self.begin_edit(EditGroup::Newline);

        let row = self.cursor_row;
        let line = self.lines[row].clone();
        let indent = Self::leading_whitespace(&line);

        let before = self.char_before_cursor();
        let after = self.char_at_cursor();

        if before == Some('{') && after == Some('}') {
            let bi = Self::byte_idx(&line, self.cursor_col);
            // `left` ends with the '{' itself since the cursor sits right
            // after it; strip that brace back off to get the code before it.
            let before_brace = line[..bi - 1].trim_end().to_string();
            let right = line[bi..].to_string();
            let inner_indent = format!("{indent}{INDENT}");

            self.lines[row] = before_brace;
            self.lines.insert(row + 1, format!("{indent}{{"));
            self.lines.insert(row + 2, inner_indent.clone());
            self.lines.insert(row + 3, format!("{indent}{right}"));

            self.cursor_row = row + 2;
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
        if self.delete_selection_as(EditGroup::Delete) {
            self.desired_col = self.cursor_col;
            return;
        }
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
                (Some('{'), Some('}'))
                    | (Some('('), Some(')'))
                    | (Some('['), Some(']'))
                    | (Some('"'), Some('"'))
                    | (Some('\''), Some('\''))
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
        if self.delete_selection_as(EditGroup::Delete) {
            self.desired_col = self.cursor_col;
            return;
        }
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

    /// Call at the top of every movement method: breaks undo coalescing,
    /// and either starts a selection (if `select`) or drops one.
    fn begin_move(&mut self, select: bool) {
        self.confirm_quit = false;
        self.reset_edit_group();
        if select {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some((self.cursor_row, self.cursor_col));
            }
        } else {
            self.selection_anchor = None;
        }
    }

    fn move_left_impl(&mut self, select: bool) {
        self.begin_move(select);
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_char_len();
        }
        self.desired_col = self.cursor_col;
    }

    fn move_right_impl(&mut self, select: bool) {
        self.begin_move(select);
        let len = self.current_line_char_len();
        if self.cursor_col < len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
        self.desired_col = self.cursor_col;
    }

    fn move_up_impl(&mut self, select: bool) {
        self.begin_move(select);
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.desired_col.min(self.current_line_char_len());
        }
    }

    fn move_down_impl(&mut self, select: bool) {
        self.begin_move(select);
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.desired_col.min(self.current_line_char_len());
        }
    }

    fn move_home_impl(&mut self, select: bool) {
        self.begin_move(select);
        self.cursor_col = 0;
        self.desired_col = 0;
    }

    fn move_end_impl(&mut self, select: bool) {
        self.begin_move(select);
        self.cursor_col = self.current_line_char_len();
        self.desired_col = self.cursor_col;
    }

    fn page_up_impl(&mut self, page: usize, select: bool) {
        self.begin_move(select);
        self.cursor_row = self.cursor_row.saturating_sub(page);
        self.cursor_col = self.desired_col.min(self.current_line_char_len());
    }

    fn page_down_impl(&mut self, page: usize, select: bool) {
        self.begin_move(select);
        self.cursor_row = (self.cursor_row + page).min(self.lines.len() - 1);
        self.cursor_col = self.desired_col.min(self.current_line_char_len());
    }

    pub fn move_left(&mut self) {
        self.move_left_impl(false);
    }
    pub fn move_left_select(&mut self) {
        self.move_left_impl(true);
    }
    pub fn move_right(&mut self) {
        self.move_right_impl(false);
    }
    pub fn move_right_select(&mut self) {
        self.move_right_impl(true);
    }
    pub fn move_up(&mut self) {
        self.move_up_impl(false);
    }
    pub fn move_up_select(&mut self) {
        self.move_up_impl(true);
    }
    pub fn move_down(&mut self) {
        self.move_down_impl(false);
    }
    pub fn move_down_select(&mut self) {
        self.move_down_impl(true);
    }
    pub fn move_home(&mut self) {
        self.move_home_impl(false);
    }
    pub fn move_home_select(&mut self) {
        self.move_home_impl(true);
    }
    pub fn move_end(&mut self) {
        self.move_end_impl(false);
    }
    pub fn move_end_select(&mut self) {
        self.move_end_impl(true);
    }
    pub fn page_up(&mut self, page: usize) {
        self.page_up_impl(page, false);
    }
    pub fn page_up_select(&mut self, page: usize) {
        self.page_up_impl(page, true);
    }
    pub fn page_down(&mut self, page: usize) {
        self.page_down_impl(page, false);
    }
    pub fn page_down_select(&mut self, page: usize) {
        self.page_down_impl(page, true);
    }

    pub fn select_all(&mut self) {
        self.confirm_quit = false;
        self.reset_edit_group();
        self.selection_anchor = Some((0, 0));
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.current_line_char_len();
        self.desired_col = self.cursor_col;
    }

    // ---- clipboard ----

    /// Copies the selection, or the current line if nothing is selected
    /// (matching VS Code's "empty selection" convention). Mirrors to the OS
    /// clipboard on a best-effort basis; always keeps an in-app copy too so
    /// paste still works when the system clipboard is unavailable.
    pub fn copy(&mut self) {
        self.confirm_quit = false;
        let text = if let Some((start, end)) = self.selection_bounds() {
            self.extract_range(start, end)
        } else {
            format!("{}\n", self.lines[self.cursor_row])
        };
        self.clipboard = text.clone();
        self.status_is_error = false;
        self.status_message = match set_system_clipboard(&text) {
            Ok(()) => "Copied".to_string(),
            Err(_) => "Copied (in-app clipboard only)".to_string(),
        };
    }

    pub fn cut(&mut self) {
        self.confirm_quit = false;
        if let Some((start, end)) = self.selection_bounds() {
            let text = self.extract_range(start, end);
            self.clipboard = text.clone();
            let _ = set_system_clipboard(&text);
            self.delete_selection_as(EditGroup::Delete);
            self.status_message = "Cut".to_string();
        } else {
            self.begin_edit(EditGroup::Delete);
            let text = format!("{}\n", self.lines[self.cursor_row]);
            self.clipboard = text.clone();
            let _ = set_system_clipboard(&text);
            if self.lines.len() > 1 {
                self.lines.remove(self.cursor_row);
                if self.cursor_row >= self.lines.len() {
                    self.cursor_row = self.lines.len() - 1;
                }
            } else {
                self.lines[0].clear();
            }
            self.cursor_col = 0;
            self.desired_col = 0;
            self.modified = true;
            self.status_message = "Cut line".to_string();
        }
        self.status_is_error = false;
    }

    /// Pastes from the OS clipboard, falling back to the in-app clipboard
    /// if the system one can't be read. Handles multi-line text.
    pub fn paste(&mut self) {
        self.confirm_quit = false;
        let text = get_system_clipboard().unwrap_or_else(|_| self.clipboard.clone());
        if text.is_empty() {
            self.status_message = "Clipboard empty".to_string();
            self.status_is_error = false;
            return;
        }

        self.last_edit_group = EditGroup::None;
        self.begin_edit(EditGroup::Paste);
        self.delete_selection_as(EditGroup::Paste);

        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() == 1 {
            for c in parts[0].chars() {
                self.insert_raw(c);
            }
        } else {
            let row = self.cursor_row;
            let bi = Self::byte_idx(&self.lines[row], self.cursor_col);
            let tail = self.lines[row][bi..].to_string();
            self.lines[row].truncate(bi);
            self.lines[row].push_str(parts[0]);
            for (i, part) in parts[1..parts.len() - 1].iter().enumerate() {
                self.lines.insert(row + 1 + i, part.to_string());
            }
            let last_idx = row + parts.len() - 1;
            let mut last_line = parts[parts.len() - 1].to_string();
            last_line.push_str(&tail);
            self.lines.insert(last_idx, last_line);
            self.cursor_row = last_idx;
            self.cursor_col = parts[parts.len() - 1].chars().count();
        }

        self.desired_col = self.cursor_col;
        self.modified = true;
        self.status_message = "Pasted".to_string();
        self.status_is_error = false;
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
    fn double_and_single_quotes_autoclose() {
        let mut ed = new_editor();
        ed.insert_char('"');
        assert_eq!(ed.lines[0], "\"\"");
        assert_eq!(ed.cursor_col, 1);

        let mut ed = new_editor();
        ed.insert_char('\'');
        assert_eq!(ed.lines[0], "''");
        assert_eq!(ed.cursor_col, 1);
    }

    #[test]
    fn typing_quote_over_existing_close_types_over() {
        let mut ed = new_editor();
        ed.insert_char('"');
        ed.insert_char('"');
        assert_eq!(ed.lines[0], "\"\"");
        assert_eq!(ed.cursor_col, 2);
    }

    #[test]
    fn typing_quote_to_close_an_open_string_does_not_autoclose_again() {
        let mut ed = new_editor();
        type_str(&mut ed, "\"hello");
        // cursor is now after "hello, i.e. not adjacent to a matching
        // closer — closing the string should just insert one quote.
        ed.insert_char('"');
        assert_eq!(ed.lines[0], "\"hello\"");
        assert_eq!(ed.cursor_col, 7);
    }

    #[test]
    fn backspace_deletes_empty_quote_pair_as_unit() {
        let mut ed = new_editor();
        ed.insert_char('"');
        ed.backspace();
        assert_eq!(ed.lines[0], "");
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn smart_enter_splits_curly_block_allman_style() {
        let mut ed = new_editor();
        type_str(&mut ed, "void Foo() {");
        // cursor sits between the auto-inserted { and }
        assert_eq!(ed.lines[0], "void Foo() {}");
        ed.newline();

        assert_eq!(ed.lines.len(), 4);
        assert_eq!(ed.lines[0], "void Foo()");
        assert_eq!(ed.lines[1], "{");
        assert_eq!(ed.lines[2], "    ");
        assert_eq!(ed.lines[3], "}");
        assert_eq!(ed.cursor_row, 2);
        assert_eq!(ed.cursor_col, 4);
    }

    #[test]
    fn smart_enter_respects_existing_indent() {
        let mut ed = new_editor();
        type_str(&mut ed, "class C ");
        ed.insert_char('{');
        ed.newline(); // -> class C\n{\n    \n}
                      // cursor is already on the indented empty line; type the method there
        type_str(&mut ed, "void Foo() ");
        ed.insert_char('{');
        ed.newline();

        assert_eq!(
            ed.lines,
            vec![
                "class C".to_string(),
                "{".to_string(),
                "    void Foo()".to_string(),
                "    {".to_string(),
                "        ".to_string(),
                "    }".to_string(),
                "}".to_string(),
            ]
        );
        assert_eq!(ed.cursor_row, 4);
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
        assert_eq!(ed.lines.len(), 4);
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

    #[test]
    fn shift_arrows_build_a_selection_not_including_gutter() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello");
        ed.move_home();
        ed.move_right_select();
        ed.move_right_select();
        ed.move_right_select();
        // selection only ever indexes into ed.lines char content, never the
        // rendered line-number gutter, so it can only ever cover "hel".
        assert_eq!(ed.selection_col_range(0), Some(0..3));
        assert_eq!(ed.selection_bounds(), Some(((0, 0), (0, 3))));
    }

    #[test]
    fn plain_arrow_key_clears_selection() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello");
        ed.move_home();
        ed.move_right_select();
        assert!(ed.has_selection());
        ed.move_right();
        assert!(!ed.has_selection());
    }

    #[test]
    fn select_all_selects_entire_buffer() {
        let mut ed = new_editor();
        type_str(&mut ed, "abc");
        ed.newline();
        type_str(&mut ed, "de");
        ed.select_all();
        assert_eq!(ed.selection_bounds(), Some(((0, 0), (1, 2))));
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello world");
        ed.move_home();
        for _ in 0..5 {
            ed.move_right_select();
        }
        ed.insert_char('X');
        assert_eq!(ed.lines[0], "X world");
        assert!(!ed.has_selection());
    }

    #[test]
    fn backspace_with_selection_deletes_whole_selection() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello world");
        ed.move_home();
        for _ in 0..5 {
            ed.move_right_select();
        }
        ed.backspace();
        assert_eq!(ed.lines[0], " world");
    }

    #[test]
    fn copy_then_paste_roundtrips_selection() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello world");
        ed.move_home();
        for _ in 0..5 {
            ed.move_right_select();
        }
        ed.copy();
        assert!(
            ed.has_selection(),
            "copy should not clear the active selection"
        );
        ed.move_end();
        ed.paste();
        assert_eq!(ed.lines[0], "hello worldhello");
    }

    #[test]
    fn cut_removes_selection_and_fills_clipboard() {
        let mut ed = new_editor();
        type_str(&mut ed, "hello world");
        ed.move_home();
        for _ in 0..5 {
            ed.move_right_select();
        }
        ed.cut();
        assert_eq!(ed.lines[0], " world");
        ed.move_end();
        ed.paste();
        assert_eq!(ed.lines[0], " worldhello");
    }

    #[test]
    fn paste_multiline_splits_into_lines() {
        let mut ed = new_editor();
        ed.clipboard = "foo\nbar".to_string();
        type_str(&mut ed, "()");
        ed.move_home();
        ed.paste();
        assert_eq!(ed.lines[0], "foo");
        assert_eq!(ed.lines[1], "bar()");
    }
}

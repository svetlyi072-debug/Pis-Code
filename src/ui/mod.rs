use crate::check::Severity;
use crate::editor::Editor;
use crate::syntax::Highlighter;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::ops::Range;

const GUTTER_MIN: usize = 4;

pub fn draw(f: &mut Frame, editor: &mut Editor, highlighter: &Highlighter) {
    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let text_area = chunks[0];
    let status_area = chunks[1];

    let gutter_width = editor.line_count().to_string().len().max(GUTTER_MIN - 1) + 1; // +1 for the space after the number
    let content_width = (text_area.width as usize).saturating_sub(gutter_width);
    let content_height = text_area.height as usize;

    editor.ensure_visible(content_width, content_height);

    let highlighted = highlighter.highlight(&editor.lines, editor.scroll + content_height);

    let mut lines: Vec<Line> = Vec::with_capacity(content_height);
    for screen_row in 0..content_height {
        let file_row = editor.scroll + screen_row;
        if file_row >= editor.lines.len() {
            lines.push(Line::from(""));
            continue;
        }

        let num = format!("{:>width$} ", file_row + 1, width = gutter_width - 1);
        let gutter_color = match editor
            .diagnostics_for_line(file_row)
            .map(|d| d.severity)
            .max()
        {
            Some(Severity::Error) => Color::Red,
            Some(Severity::Warning) => Color::Yellow,
            None => Color::DarkGray,
        };
        let mut spans = vec![Span::styled(num, Style::default().fg(gutter_color))];

        if let Some(row_spans) = highlighted.get(file_row) {
            let visible = slice_spans(row_spans, editor.col_scroll, content_width);
            let sel = editor.selection_col_range(file_row);
            let with_selection = apply_selection(visible, editor.col_scroll, sel);

            let line_char_len = editor.lines[file_row].chars().count();
            let severities =
                diagnostic_severity_map(editor.diagnostics_for_line(file_row), line_char_len);
            spans.extend(apply_diagnostics(
                with_selection,
                editor.col_scroll,
                &severities,
            ));
        }

        lines.push(Line::from(spans));
    }

    // No explicit background here: leaving cells unstyled lets the
    // terminal's own background show through (including a transparent /
    // wallpapered background in terminals like Ghostty, Kitty, WezTerm).
    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, text_area);

    draw_status(f, status_area, editor);

    let cursor_screen_row = editor.cursor_row - editor.scroll;
    let cursor_screen_col = gutter_width + (editor.cursor_col - editor.col_scroll);
    let cx = text_area.x + cursor_screen_col as u16;
    let cy = text_area.y + cursor_screen_row as u16;
    if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
        f.set_cursor(cx, cy);
    }
}

fn draw_status(f: &mut Frame, area: Rect, editor: &Editor) {
    let modified = if editor.modified { " [+]" } else { "" };
    let left = format!(
        " {}{}  Ln {}, Col {}  {} chars, {} lines",
        editor.file_path.display(),
        modified,
        editor.cursor_row + 1,
        editor.cursor_col + 1,
        editor.char_count(),
        editor.line_count()
    );
    let right = editor.status_message.clone();

    let style = if editor.status_is_error {
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(0x8b, 0x2f, 0x2f))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(0x7a, 0xa2, 0xf7))
    };

    // Widths are measured in characters, not bytes — status messages can
    // contain non-ASCII text (e.g. a localized compiler diagnostic), where
    // byte length and display width diverge.
    let width = area.width as usize;
    let left_len = left.chars().count();
    let right_len = right.chars().count();

    // Always keep at least one separating space between the two halves;
    // if they still don't both fit, the right side gets clipped instead
    // of being smashed directly against the left with no separator.
    let min_needed = left_len + 1 + right_len;
    let sep = if min_needed <= width {
        width - left_len - right_len
    } else {
        1
    };

    let mut text = left;
    text.push_str(&" ".repeat(sep));
    text.push_str(&right);
    text.push(' ');
    let text: String = text.chars().take(width).collect();

    let paragraph = Paragraph::new(Line::from(Span::styled(text, style))).style(style);
    f.render_widget(paragraph, area);
}

/// Slice a highlighted line's spans to the visible window
/// [start, start+width) in character coordinates, preserving styles.
fn slice_spans<'a>(spans: &[(Style, String)], start: usize, width: usize) -> Vec<Span<'a>> {
    let mut result = Vec::new();
    let mut pos = 0usize;
    let mut remaining = width;

    for (style, text) in spans {
        if remaining == 0 {
            break;
        }
        let char_count = text.chars().count();
        if pos + char_count <= start {
            pos += char_count;
            continue;
        }
        let skip = start.saturating_sub(pos);
        let take = (char_count - skip).min(remaining);
        if take > 0 {
            let sliced: String = text.chars().skip(skip).take(take).collect();
            result.push(Span::styled(sliced, *style));
            remaining -= take;
        }
        pos += char_count;
    }

    result
}

/// Splits `spans` (already the visible-window slice starting at absolute
/// column `col_scroll`) wherever a selection boundary falls, reversing
/// video on the selected run so it reads as highlighted text.
fn apply_selection<'a>(
    spans: Vec<Span<'a>>,
    col_scroll: usize,
    sel: Option<Range<usize>>,
) -> Vec<Span<'a>> {
    let Some(sel) = sel else {
        return spans;
    };
    if sel.is_empty() {
        return spans;
    }

    let mut result = Vec::new();
    let mut abs_col = col_scroll;
    for span in spans {
        let style = span.style;
        let text = span.content.into_owned();
        let mut buf = String::new();
        let mut buf_selected: Option<bool> = None;

        for ch in text.chars() {
            let is_sel = sel.contains(&abs_col);
            if buf_selected == Some(!is_sel) {
                push_run(&mut result, &buf, style, buf_selected == Some(true));
                buf.clear();
            }
            buf_selected = Some(is_sel);
            buf.push(ch);
            abs_col += 1;
        }
        if !buf.is_empty() {
            push_run(&mut result, &buf, style, buf_selected == Some(true));
        }
    }
    result
}

fn push_run<'a>(out: &mut Vec<Span<'a>>, text: &str, style: Style, selected: bool) {
    let style = if selected {
        style.add_modifier(Modifier::REVERSED)
    } else {
        style
    };
    out.push(Span::styled(text.to_string(), style));
}

/// Maps each character column that has a diagnostic on this line to its
/// (strongest, if several land on the same column) severity. A compiler
/// diagnostic is a single point, not a range, so out-of-range columns
/// (e.g. "expected ;" at end of line) clamp to the last real character.
fn diagnostic_severity_map<'a>(
    diags: impl Iterator<Item = &'a crate::check::Diagnostic>,
    line_char_len: usize,
) -> HashMap<usize, Severity> {
    let mut map = HashMap::new();
    if line_char_len == 0 {
        return map;
    }
    for d in diags {
        let col = if d.col < line_char_len {
            d.col
        } else {
            line_char_len - 1
        };
        map.entry(col)
            .and_modify(|s: &mut Severity| *s = (*s).max(d.severity))
            .or_insert(d.severity);
    }
    map
}

/// Underlines (in red for errors, yellow for warnings) the characters at
/// columns present in `severities`.
fn apply_diagnostics<'a>(
    spans: Vec<Span<'a>>,
    col_scroll: usize,
    severities: &HashMap<usize, Severity>,
) -> Vec<Span<'a>> {
    if severities.is_empty() {
        return spans;
    }

    let mut result = Vec::new();
    let mut abs_col = col_scroll;
    for span in spans {
        let style = span.style;
        let text = span.content.into_owned();
        let mut buf = String::new();
        let mut buf_sev: Option<Option<Severity>> = None;

        for ch in text.chars() {
            let sev = severities.get(&abs_col).copied();
            if let Some(prev) = buf_sev {
                if prev != sev {
                    push_diagnostic_run(&mut result, &buf, style, prev);
                    buf.clear();
                }
            }
            buf_sev = Some(sev);
            buf.push(ch);
            abs_col += 1;
        }
        if let Some(prev) = buf_sev {
            if !buf.is_empty() {
                push_diagnostic_run(&mut result, &buf, style, prev);
            }
        }
    }
    result
}

fn push_diagnostic_run<'a>(
    out: &mut Vec<Span<'a>>,
    text: &str,
    style: Style,
    sev: Option<Severity>,
) {
    let style = match sev {
        Some(Severity::Error) => style.fg(Color::Red).add_modifier(Modifier::UNDERLINED),
        Some(Severity::Warning) => style.fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
        None => style,
    };
    out.push(Span::styled(text.to_string(), style));
}

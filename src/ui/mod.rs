use crate::check::Severity;
use crate::config::{SelectionStyle, StatusPosition, UiSettings};
use crate::editor::{display_columns, Editor};
use crate::syntax::Highlighter;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::ops::Range;

pub fn draw(f: &mut Frame, editor: &mut Editor, highlighter: &Highlighter, ui: &UiSettings) {
    let area = f.size();
    let (text_constraint, status_constraint) = (Constraint::Min(1), Constraint::Length(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(match ui.status_position {
            StatusPosition::Bottom => [text_constraint, status_constraint],
            StatusPosition::Top => [status_constraint, text_constraint],
        })
        .split(area);
    let (text_area, status_area) = match ui.status_position {
        StatusPosition::Bottom => (chunks[0], chunks[1]),
        StatusPosition::Top => (chunks[1], chunks[0]),
    };

    // Digits of the largest line number, at least `gutter_min_width - 1`
    // wide, plus one space after; no gutter at all without line numbers.
    let gutter_width = if ui.line_numbers {
        editor
            .line_count()
            .to_string()
            .len()
            .max(ui.gutter_min_width.saturating_sub(1))
            + 1
    } else {
        0
    };
    let content_width = (text_area.width as usize).saturating_sub(gutter_width);
    let content_height = text_area.height as usize;

    editor.ensure_visible(content_width, content_height);

    let highlighted = highlighter.highlight(&editor.lines, editor.scroll + content_height);
    let tab_width = editor.tab_width();

    let mut lines: Vec<Line> = Vec::with_capacity(content_height);
    for screen_row in 0..content_height {
        let file_row = editor.scroll + screen_row;
        if file_row >= editor.lines.len() {
            lines.push(Line::from(""));
            continue;
        }

        let mut spans = Vec::new();
        if ui.line_numbers {
            let is_current = file_row == editor.cursor_row;
            let shown = if ui.relative_line_numbers && !is_current {
                file_row.abs_diff(editor.cursor_row)
            } else {
                file_row + 1
            };
            let num = format!("{:>width$} ", shown, width = gutter_width - 1);
            let marked = if ui.mark_gutter {
                editor
                    .diagnostics_for_line(file_row)
                    .map(|d| d.severity)
                    .max()
            } else {
                None
            };
            let gutter_color = match marked {
                Some(Severity::Error) => ui.gutter_error,
                Some(Severity::Warning) => ui.gutter_warning,
                None if is_current => ui.gutter_current_line,
                None => ui.gutter,
            };
            spans.push(Span::styled(num, Style::default().fg(gutter_color)));
        }

        if let Some(row_spans) = highlighted.get(file_row) {
            // Everything below works in screen cells, not characters:
            // tabs are shown as spaces up to the next tab stop, so a
            // character's index no longer equals its on-screen column.
            let line = &editor.lines[file_row];
            let cols = display_columns(line, tab_width);
            let last = cols.len() - 1;
            let to_cell = |char_idx: usize| cols[char_idx.min(last)];

            let expanded = expand_tabs(row_spans, tab_width);
            let visible = slice_spans(&expanded, editor.col_scroll, content_width);

            let sel = editor
                .selection_col_range(file_row)
                .map(|r| to_cell(r.start)..to_cell(r.end));
            let with_selection = apply_selection(visible, editor.col_scroll, sel, ui.selection);

            let line_char_len = line.chars().count();
            let severities: HashMap<usize, Severity> = if ui.underline_diagnostics {
                diagnostic_severity_map(editor.diagnostics_for_line(file_row), line_char_len)
                    .into_iter()
                    .map(|(char_idx, sev)| (to_cell(char_idx), sev))
                    .collect()
            } else {
                HashMap::new()
            };
            spans.extend(apply_diagnostics(
                with_selection,
                editor.col_scroll,
                &severities,
                (ui.diagnostic_error, ui.diagnostic_warning),
            ));
        }

        lines.push(Line::from(spans));
    }

    // No explicit background here: leaving cells unstyled lets the
    // terminal's own background show through (including a transparent /
    // wallpapered background in terminals like Ghostty, Kitty, WezTerm).
    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, text_area);

    draw_status(f, status_area, editor, ui);

    let cursor_screen_row = editor.cursor_row - editor.scroll;
    let cursor_screen_col = gutter_width + (editor.cursor_display_col() - editor.col_scroll);
    let cx = text_area.x + cursor_screen_col as u16;
    let cy = text_area.y + cursor_screen_row as u16;
    if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
        f.set_cursor(cx, cy);
    }
}

/// The left half of the status bar: whichever of file name, modified
/// marker, cursor position and counts are switched on.
fn status_left(editor: &Editor, ui: &UiSettings) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut name = String::new();
    if ui.show_file_name {
        name.push_str(&editor.file_path.display().to_string());
    }
    if ui.show_modified_marker && editor.modified {
        name.push_str(" [+]");
    }
    if !name.trim().is_empty() {
        parts.push(name.trim_start().to_string());
    }
    if ui.show_cursor_position {
        parts.push(format!(
            "Ln {}, Col {}",
            editor.cursor_row + 1,
            editor.cursor_col + 1
        ));
    }
    match (ui.show_char_count, ui.show_line_count) {
        (true, true) => parts.push(format!(
            "{} chars, {} lines",
            editor.char_count(),
            editor.line_count()
        )),
        (true, false) => parts.push(format!("{} chars", editor.char_count())),
        (false, true) => parts.push(format!("{} lines", editor.line_count())),
        (false, false) => {}
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join("  "))
    }
}

fn draw_status(f: &mut Frame, area: Rect, editor: &Editor, ui: &UiSettings) {
    let left = status_left(editor, ui);
    let right = editor.status_message.clone();

    let style = if editor.status_is_error {
        Style::default()
            .fg(ui.status_error_fg)
            .bg(ui.status_error_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui.status_fg).bg(ui.status_bg)
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

/// Replaces each tab with the spaces that reach the next tab stop, so the
/// terminal never sees a raw `\t` (whose width it, and ratatui's diffing,
/// would otherwise disagree about).
fn expand_tabs(spans: &[(Style, String)], tab_width: usize) -> Vec<(Style, String)> {
    let mut col = 0usize;
    spans
        .iter()
        .map(|(style, text)| {
            let mut out = String::with_capacity(text.len());
            for ch in text.chars() {
                if ch == '\t' {
                    let n = tab_width - col % tab_width;
                    out.push_str(&" ".repeat(n));
                    col += n;
                } else {
                    out.push(ch);
                    col += 1;
                }
            }
            (*style, out)
        })
        .collect()
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
/// the selection style (reverse video by default) on the selected run.
fn apply_selection<'a>(
    spans: Vec<Span<'a>>,
    col_scroll: usize,
    sel: Option<Range<usize>>,
    style_of_selection: SelectionStyle,
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
                push_run(
                    &mut result,
                    &buf,
                    style,
                    (buf_selected == Some(true)).then_some(style_of_selection),
                );
                buf.clear();
            }
            buf_selected = Some(is_sel);
            buf.push(ch);
            abs_col += 1;
        }
        if !buf.is_empty() {
            push_run(
                &mut result,
                &buf,
                style,
                (buf_selected == Some(true)).then_some(style_of_selection),
            );
        }
    }
    result
}

fn push_run<'a>(
    out: &mut Vec<Span<'a>>,
    text: &str,
    style: Style,
    selected: Option<SelectionStyle>,
) {
    let style = match selected {
        Some(sel) => sel.apply(style),
        None => style,
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
    colors: (Color, Color),
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
                    push_diagnostic_run(&mut result, &buf, style, prev, colors);
                    buf.clear();
                }
            }
            buf_sev = Some(sev);
            buf.push(ch);
            abs_col += 1;
        }
        if let Some(prev) = buf_sev {
            if !buf.is_empty() {
                push_diagnostic_run(&mut result, &buf, style, prev, colors);
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
    (error, warning): (Color, Color),
) {
    let style = match sev {
        Some(Severity::Error) => style.fg(error).add_modifier(Modifier::UNDERLINED),
        Some(Severity::Warning) => style.fg(warning).add_modifier(Modifier::UNDERLINED),
        None => style,
    };
    out.push(Span::styled(text.to_string(), style));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Diagnostic;
    use crate::config::Config;
    use crate::language::Language;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn ui_with(tweak: impl FnOnce(&mut Config)) -> UiSettings {
        let mut config = Config::default();
        tweak(&mut config);
        UiSettings::from_config(&config).0
    }

    fn editor_with(lines: &[&str]) -> Editor {
        let mut ed = Editor::open(PathBuf::from("__ui_test__/a.txt")).unwrap();
        ed.lines = lines.iter().map(|l| l.to_string()).collect();
        ed
    }

    /// Draws one frame; also returns where the cursor ended up.
    fn render(ed: &mut Editor, ui: &UiSettings, width: u16, height: u16) -> (Buffer, (u16, u16)) {
        let plain = Highlighter::new(Language::Other, false, "base16-ocean.dark").0;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, ed, &plain, ui)).unwrap();
        let cursor = terminal.get_cursor().unwrap();
        (terminal.backend().buffer().clone(), cursor)
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf.get(x, y).symbol())
            .collect()
    }

    #[test]
    fn the_default_look_has_line_numbers_and_a_bottom_status_bar() {
        let mut ed = editor_with(&["alpha", "beta"]);
        let (buf, cursor) = render(&mut ed, &UiSettings::default(), 60, 5);
        assert!(row(&buf, 0).starts_with("  1 alpha"), "{:?}", row(&buf, 0));
        assert!(row(&buf, 1).starts_with("  2 beta"));
        let status = row(&buf, 4);
        assert!(status.contains("__ui_test__/a.txt"), "{status:?}");
        assert!(status.contains("Ln 1, Col 1"), "{status:?}");
        assert!(status.contains("9 chars, 2 lines"), "{status:?}");
        assert_eq!(cursor, (4, 0), "cursor sits after the 4-cell gutter");
    }

    #[test]
    fn line_numbers_can_be_hidden() {
        let ui = ui_with(|c| c.display.line_numbers = false);
        let mut ed = editor_with(&["alpha"]);
        let (buf, cursor) = render(&mut ed, &ui, 40, 4);
        assert!(row(&buf, 0).starts_with("alpha"), "{:?}", row(&buf, 0));
        assert_eq!(cursor, (0, 0), "no gutter to skip");
    }

    #[test]
    fn gutter_min_width_widens_the_gutter() {
        let ui = ui_with(|c| c.display.gutter_min_width = 8);
        let mut ed = editor_with(&["alpha"]);
        let (buf, cursor) = render(&mut ed, &ui, 40, 4);
        assert!(
            row(&buf, 0).starts_with("      1 alpha"),
            "{:?}",
            row(&buf, 0)
        );
        assert_eq!(cursor.0, 8);

        // A long file outgrows the minimum rather than being cut off.
        let many: Vec<String> = (0..12_345).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let mut ed = editor_with(&refs);
        let (buf, _) = render(&mut ed, &UiSettings::default(), 40, 4);
        assert!(
            row(&buf, 0).starts_with("    1 0"),
            "five digits + space: {:?}",
            row(&buf, 0)
        );
    }

    #[test]
    fn relative_line_numbers_count_from_the_cursor() {
        let ui = ui_with(|c| c.display.relative_line_numbers = true);
        let mut ed = editor_with(&["a", "b", "c", "d", "e"]);
        ed.cursor_row = 2;
        let (buf, _) = render(&mut ed, &ui, 30, 7);
        let numbers: Vec<String> = (0..5)
            .map(|y| {
                row(&buf, y)
                    .chars()
                    .take(4)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect();
        assert_eq!(
            numbers,
            ["2", "1", "3", "1", "2"],
            "the current line keeps its real number"
        );
    }

    #[test]
    fn the_status_bar_can_go_on_top() {
        let ui = ui_with(|c| c.status_bar.position = StatusPosition::Top);
        let mut ed = editor_with(&["alpha"]);
        let (buf, cursor) = render(&mut ed, &ui, 60, 5);
        assert!(row(&buf, 0).contains("Ln 1, Col 1"), "{:?}", row(&buf, 0));
        assert!(
            row(&buf, 1).starts_with("  1 alpha"),
            "text starts below the bar"
        );
        assert!(!row(&buf, 4).contains("Ln 1"));
        assert_eq!(cursor, (4, 1));
    }

    #[test]
    fn each_status_bar_part_can_be_hidden() {
        let mut ed = editor_with(&["alpha"]);
        ed.modified = true;
        let mut status = |tweak: fn(&mut Config)| {
            let (buf, _) = render(&mut ed, &ui_with(tweak), 80, 3);
            row(&buf, 2)
        };
        let all = status(|_| {});
        assert!(
            all.contains("a.txt [+]")
                && all.contains("Ln 1, Col 1")
                && all.contains("chars")
                && all.contains("lines")
        );

        let no_name = status(|c| c.status_bar.show_file_name = false);
        assert!(
            !no_name.contains("a.txt") && no_name.contains("[+]"),
            "{no_name:?}"
        );
        let no_marker = status(|c| c.status_bar.show_modified_marker = false);
        assert!(
            no_marker.contains("a.txt") && !no_marker.contains("[+]"),
            "{no_marker:?}"
        );
        let no_pos = status(|c| c.status_bar.show_cursor_position = false);
        assert!(
            !no_pos.contains("Ln 1") && no_pos.contains("chars"),
            "{no_pos:?}"
        );
        let no_chars = status(|c| c.status_bar.show_char_count = false);
        assert!(
            !no_chars.contains("chars") && no_chars.contains("1 lines"),
            "{no_chars:?}"
        );
        let no_lines = status(|c| c.status_bar.show_line_count = false);
        assert!(
            !no_lines.contains("lines") && no_lines.contains("5 chars"),
            "{no_lines:?}"
        );
        let nothing = status(|c| {
            c.status_bar.show_file_name = false;
            c.status_bar.show_modified_marker = false;
            c.status_bar.show_cursor_position = false;
            c.status_bar.show_char_count = false;
            c.status_bar.show_line_count = false;
        });
        assert!(
            !nothing.contains("a.txt") && !nothing.contains("Ln") && !nothing.contains("chars"),
            "{nothing:?}"
        );
    }

    #[test]
    fn status_messages_are_shown_on_the_right() {
        let mut ed = editor_with(&["alpha"]);
        ed.status_message = "hello there".to_string();
        let (buf, _) = render(&mut ed, &UiSettings::default(), 70, 3);
        assert!(
            row(&buf, 2).trim_end().ends_with("hello there"),
            "{:?}",
            row(&buf, 2)
        );
    }

    #[test]
    fn gutter_and_status_colors_come_from_the_config() {
        let ui = ui_with(|c| {
            c.colors.gutter = "blue".into();
            c.colors.gutter_current_line = "#ffcc00".into();
            c.colors.status_fg = "white".into();
            c.colors.status_bg = "#102030".into();
            c.colors.status_error_fg = "yellow".into();
            c.colors.status_error_bg = "magenta".into();
        });
        let mut ed = editor_with(&["a", "b"]);
        let (buf, _) = render(&mut ed, &ui, 40, 4);
        assert_eq!(
            buf.get(2, 0).fg,
            Color::Rgb(0xff, 0xcc, 0x00),
            "current line number"
        );
        assert_eq!(buf.get(2, 1).fg, Color::Blue, "other line numbers");
        assert_eq!(buf.get(0, 3).bg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(buf.get(0, 3).fg, Color::White);

        ed.status_is_error = true;
        let (buf, _) = render(&mut ed, &ui, 40, 4);
        assert_eq!(buf.get(0, 3).bg, Color::Magenta);
        assert_eq!(buf.get(0, 3).fg, Color::Yellow);
    }

    fn with_error_on_first_line(ed: &mut Editor) {
        ed.set_diagnostics(vec![
            Diagnostic {
                line: 0,
                col: 1,
                severity: Severity::Error,
                code: "E1".into(),
                message: "bad".into(),
            },
            Diagnostic {
                line: 1,
                col: 0,
                severity: Severity::Warning,
                code: String::new(),
                message: "meh".into(),
            },
        ]);
    }

    #[test]
    fn diagnostics_use_the_configured_colors_in_gutter_and_text() {
        let ui = ui_with(|c| {
            c.colors.gutter_error = "#aa0000".into();
            c.colors.gutter_warning = "#aaaa00".into();
            c.colors.diagnostic_error = "#ff0000".into();
            c.colors.diagnostic_warning = "#ffff00".into();
        });
        let mut ed = editor_with(&["abc", "def"]);
        with_error_on_first_line(&mut ed);
        let (buf, _) = render(&mut ed, &ui, 40, 5);
        assert_eq!(buf.get(2, 0).fg, Color::Rgb(0xaa, 0, 0));
        assert_eq!(buf.get(2, 1).fg, Color::Rgb(0xaa, 0xaa, 0));
        let bad = buf.get(5, 0); // the "b" of "abc"
        assert_eq!(bad.symbol(), "b");
        assert_eq!(bad.fg, Color::Rgb(0xff, 0, 0));
        assert!(bad.modifier.contains(Modifier::UNDERLINED));
        assert_eq!(
            buf.get(4, 0).fg,
            Color::Reset,
            "the neighbors are untouched"
        );
        assert_eq!(buf.get(4, 1).fg, Color::Rgb(0xff, 0xff, 0));
    }

    #[test]
    fn gutter_marks_and_underlines_can_be_turned_off_separately() {
        let mut ed = editor_with(&["abc", "def"]);
        with_error_on_first_line(&mut ed);

        let ui = ui_with(|c| c.check.mark_gutter = false);
        let (buf, _) = render(&mut ed, &ui, 40, 5);
        assert_eq!(buf.get(2, 0).fg, Color::DarkGray, "no red number");
        assert!(
            buf.get(5, 0).modifier.contains(Modifier::UNDERLINED),
            "underline stays"
        );

        let ui = ui_with(|c| c.check.underline = false);
        let (buf, _) = render(&mut ed, &ui, 40, 5);
        assert_eq!(buf.get(2, 0).fg, Color::Red, "red number stays");
        assert!(!buf.get(5, 0).modifier.contains(Modifier::UNDERLINED));
        assert_eq!(buf.get(5, 0).fg, Color::Reset);
    }

    #[test]
    fn selection_is_drawn_in_the_configured_style() {
        let selected = |style: &str| {
            let ui = ui_with(|c| c.colors.selection = style.into());
            let mut ed = editor_with(&["abc"]);
            ed.select_all();
            let (buf, _) = render(&mut ed, &ui, 30, 3);
            let cell = buf.get(4, 0).clone();
            let outside = buf.get(10, 0).clone();
            (cell, outside)
        };
        let (cell, outside) = selected("reverse");
        assert!(
            cell.modifier.contains(Modifier::REVERSED)
                && !outside.modifier.contains(Modifier::REVERSED)
        );
        let (cell, _) = selected("underline");
        assert!(
            cell.modifier.contains(Modifier::UNDERLINED)
                && !cell.modifier.contains(Modifier::REVERSED)
        );
        let (cell, _) = selected("#264f78");
        assert_eq!(cell.bg, Color::Rgb(0x26, 0x4f, 0x78));
        assert!(!cell.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn tab_width_changes_how_tabs_are_drawn_and_where_the_cursor_goes() {
        let mut ed = editor_with(&["\tx"]);
        ed.cursor_col = 2;
        ed.settings.tab_width = 2;
        let (buf, cursor) = render(&mut ed, &UiSettings::default(), 30, 3);
        assert!(
            row(&buf, 0).starts_with("  1   x"),
            "tab is two cells: {:?}",
            row(&buf, 0)
        );
        assert_eq!(cursor.0, 4 + 3);

        ed.settings.tab_width = 8;
        let (buf, cursor) = render(&mut ed, &UiSettings::default(), 30, 3);
        assert!(
            row(&buf, 0).starts_with(&format!("  1 {}x", " ".repeat(8))),
            "tab is eight cells: {:?}",
            row(&buf, 0)
        );
        assert_eq!(cursor.0, 4 + 9);
    }

    #[test]
    fn the_cursor_sits_on_the_right_cell_even_after_horizontal_scrolling() {
        let long = "x".repeat(100);
        let mut ed = editor_with(&[long.as_str()]);
        ed.cursor_col = 100;
        let (_, cursor) = render(&mut ed, &UiSettings::default(), 40, 3);
        assert!(cursor.0 < 40, "{cursor:?}");
        assert_eq!(cursor.1, 0);
    }
}

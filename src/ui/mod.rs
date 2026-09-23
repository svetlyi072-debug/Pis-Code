use crate::editor::Editor;
use crate::syntax::Highlighter;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

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
        let mut spans = vec![Span::styled(num, Style::default().fg(Color::DarkGray))];

        if let Some(row_spans) = highlighted.get(file_row) {
            spans.extend(slice_spans(row_spans, editor.col_scroll, content_width));
        }

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Rgb(0x1b, 0x1e, 0x28)));
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
        " {}{}  Ln {}, Col {}",
        editor.file_path.display(),
        modified,
        editor.cursor_row + 1,
        editor.cursor_col + 1
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

    let width = area.width as usize;
    let mut text = left.clone();
    let pad = width.saturating_sub(left.len() + right.len() + 1);
    text.push_str(&" ".repeat(pad));
    text.push_str(&right);
    text.push(' ');
    if text.len() > width {
        text.truncate(width);
    }

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

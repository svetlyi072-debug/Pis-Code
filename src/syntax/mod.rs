use ratatui::style::{Color, Modifier, Style as RtStyle};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
    syntax: SyntaxReference,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .unwrap_or_else(|| theme_set.themes.values().next().unwrap().clone());
        let syntax = syntax_set
            .find_syntax_by_extension("cs")
            .or_else(|| syntax_set.find_syntax_by_name("C#"))
            .cloned()
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text().clone());

        Self {
            syntax_set,
            theme,
            syntax,
        }
    }

    /// Highlight `lines[0..upto]` from the start of the buffer so
    /// multi-line constructs (block comments, verbatim strings) stay
    /// consistent, and return styled spans per line.
    pub fn highlight(&self, lines: &[String], upto: usize) -> Vec<Vec<(RtStyle, String)>> {
        let mut h = HighlightLines::new(&self.syntax, &self.theme);
        let upto = upto.min(lines.len());
        let mut out = Vec::with_capacity(upto);
        for line in &lines[..upto] {
            let with_nl = format!("{line}\n");
            let ranges: Vec<(SynStyle, &str)> = h
                .highlight_line(&with_nl, &self.syntax_set)
                .unwrap_or_default();
            let spans = ranges
                .into_iter()
                .map(|(style, text)| {
                    (
                        to_ratatui_style(style),
                        text.trim_end_matches('\n').to_string(),
                    )
                })
                .filter(|(_, text)| !text.is_empty())
                .collect();
            out.push(spans);
        }
        out
    }
}

fn to_ratatui_style(style: SynStyle) -> RtStyle {
    let fg = style.foreground;
    let mut rt = RtStyle::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        rt = rt.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        rt = rt.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        rt = rt.add_modifier(Modifier::UNDERLINED);
    }
    rt
}

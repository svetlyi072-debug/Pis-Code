use crate::language::Language;
use ratatui::style::{Color, Modifier, Style as RtStyle};
use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet, SyntaxSetBuilder};

/// The theme used when `syntax.theme` names none that exists.
const DEFAULT_THEME: &str = "base16-ocean.dark";

/// Syntax highlighting for the languages Pis Code knows (C#, JavaScript,
/// Rust, Python, GDScript, HTML, ...). Any other file type — and every
/// file when highlighting is switched off — is deliberately left unstyled
/// so its text renders in the terminal's own configured colors instead of
/// a fixed theme foreground.
pub struct Highlighter {
    /// `None` means plain: no styling at all.
    backend: Option<SyntectBackend>,
}

struct SyntectBackend {
    syntax_set: SyntaxSet,
    theme: Theme,
    syntax: SyntaxReference,
}

/// Names of the color themes that ship with the editor.
pub fn theme_names() -> Vec<String> {
    let mut names: Vec<String> = ThemeSet::load_defaults().themes.into_keys().collect();
    names.sort();
    names
}

/// Finds `name` among the built-in themes, or loads it as a `.tmTheme`
/// file when it looks like a path. On failure returns a message and the
/// default theme.
fn load_theme(name: &str) -> (Theme, Option<String>) {
    let mut themes = ThemeSet::load_defaults().themes;
    let name = name.trim();
    if let Some(theme) = themes.remove(name) {
        return (theme, None);
    }
    // Built-in names are matched case-insensitively as a convenience.
    if let Some(key) = themes
        .keys()
        .find(|k| k.eq_ignore_ascii_case(name))
        .cloned()
    {
        return (themes.remove(&key).expect("key just found"), None);
    }
    let fallback = themes.remove(DEFAULT_THEME).unwrap_or_else(|| {
        ThemeSet::load_defaults()
            .themes
            .into_values()
            .next()
            .unwrap()
    });
    let looks_like_file = name.ends_with(".tmTheme") || Path::new(name).is_file();
    if looks_like_file {
        return match ThemeSet::get_theme(name) {
            Ok(theme) => (theme, None),
            Err(e) => (
                fallback,
                Some(format!(
                    "syntax.theme: can't load {name:?}: {e}; using {DEFAULT_THEME}"
                )),
            ),
        };
    }
    (
        fallback,
        Some(format!(
            "syntax.theme: unknown theme {name:?} (built in: {}; or a path to a .tmTheme file); using {DEFAULT_THEME}",
            theme_names().join(", ")
        )),
    )
}

impl Highlighter {
    /// Highlighter for `language` in the named `theme`. With `enabled`
    /// false the text is left unstyled. The second value is a warning when
    /// the theme couldn't be found or loaded.
    pub fn new(language: Language, enabled: bool, theme: &str) -> (Self, Option<String>) {
        if !enabled {
            return (Self { backend: None }, None);
        }
        let (theme, warning) = load_theme(theme);
        let backend = match language {
            // syntect's bundled grammars have no GDScript, so it ships its
            // own (gdscript.sublime-syntax) in a set of its own.
            Language::GdScript => Some(SyntectBackend::gdscript(theme)),
            // `None` for the types left unstyled (see the struct docs).
            other => other
                .bundled_syntax_extension()
                .map(|ext| SyntectBackend::bundled(ext, theme)),
        };
        (Self { backend }, warning)
    }

    /// Highlight `lines[0..upto]` from the start of the buffer so
    /// multi-line constructs (block comments, verbatim strings) stay
    /// consistent, and return styled spans per line.
    pub fn highlight(&self, lines: &[String], upto: usize) -> Vec<Vec<(RtStyle, String)>> {
        let upto = upto.min(lines.len());

        let Some(backend) = &self.backend else {
            return lines[..upto]
                .iter()
                .map(|line| {
                    if line.is_empty() {
                        Vec::new()
                    } else {
                        vec![(RtStyle::default(), line.clone())]
                    }
                })
                .collect();
        };

        let mut h = HighlightLines::new(&backend.syntax, &backend.theme);
        let mut out = Vec::with_capacity(upto);
        for line in &lines[..upto] {
            let with_nl = format!("{line}\n");
            let ranges: Vec<(SynStyle, &str)> = h
                .highlight_line(&with_nl, &backend.syntax_set)
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

impl SyntectBackend {
    fn bundled(extension: &str, theme: Theme) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax = syntax_set
            .find_syntax_by_extension(extension)
            .cloned()
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text().clone());
        Self {
            syntax_set,
            theme,
            syntax,
        }
    }

    fn gdscript(theme: Theme) -> Self {
        let definition =
            SyntaxDefinition::load_from_str(include_str!("gdscript.sublime-syntax"), true, None)
                .expect("bundled GDScript grammar is valid");
        let mut builder = SyntaxSetBuilder::new();
        builder.add(definition);
        let syntax_set = builder.build();
        let syntax = syntax_set
            .find_syntax_by_extension("gd")
            .cloned()
            .expect("GDScript grammar registers the .gd extension");
        Self {
            syntax_set,
            theme,
            syntax,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_for(path: &str, source: &str) -> Vec<Vec<(RtStyle, String)>> {
        let lines: Vec<String> = source.lines().map(String::from).collect();
        highlighter_for(path).highlight(&lines, lines.len())
    }

    fn highlighter_for(path: &str) -> Highlighter {
        let language = Language::from_path(Path::new(path));
        Highlighter::new(language, true, DEFAULT_THEME).0
    }

    /// The foreground color of the first span whose text, ignoring the
    /// whitespace syntect merges into neighboring same-colored spans, is
    /// exactly `text`.
    fn fg_of(row: &[(RtStyle, String)], text: &str) -> Option<Color> {
        row.iter()
            .find(|(_, t)| t.trim() == text)
            .and_then(|(s, _)| s.fg)
    }

    #[test]
    fn unsupported_file_types_are_unstyled_so_terminal_colors_apply() {
        for path in [
            "notes.txt",
            "data.csv",
            "config.toml",
            "main.go",
            "Makefile",
        ] {
            let rows = spans_for(path, "hello world 123 \"quoted\"");
            assert_eq!(rows.len(), 1, "{path}");
            for (style, _) in &rows[0] {
                assert_eq!(*style, RtStyle::default(), "{path} must not set colors");
            }
            let joined: String = rows[0].iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(joined, "hello world 123 \"quoted\"", "{path}");
        }
    }

    #[test]
    fn supported_languages_are_still_colored() {
        for (path, source) in [
            ("a.cs", "using System;"),
            ("a.js", "const x = 1;"),
            ("a.rs", "fn main() {}"),
            ("a.py", "def f(): pass"),
            ("a.gd", "func _ready():"),
        ] {
            let rows = spans_for(path, source);
            assert!(
                rows[0].iter().any(|(s, _)| s.fg.is_some()),
                "{path} should have colored spans"
            );
        }
    }

    /// How many different colors a highlighted row uses. A grammar that
    /// really loaded colors tokens differently; the plain-text fallback
    /// paints everything one color.
    fn distinct_colors(row: &[(RtStyle, String)]) -> usize {
        let mut colors: Vec<_> = row.iter().filter_map(|(s, _)| s.fg).collect();
        colors.dedup();
        colors.sort_by_key(|c| format!("{c:?}"));
        colors.dedup();
        colors.len()
    }

    #[test]
    fn web_and_markup_languages_have_real_grammars() {
        for (path, source) in [
            ("a.html", "<div class=\"a\">x</div>"),
            ("a.css", "a { color: red; }"),
            ("a.json", "{\"a\": 1, \"b\": \"x\"}"),
            ("a.xml", "<a b=\"c\">d</a>"),
            ("a.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>"),
            ("a.yaml", "key: \"v\" # c"),
            ("a.md", "# Title"),
        ] {
            let rows = spans_for(path, source);
            assert!(
                distinct_colors(&rows[0]) >= 2,
                "{path} fell back to plain text: {:?}",
                rows[0]
            );
        }
    }

    #[test]
    fn gdscript_grammar_loads_and_every_context_compiles() {
        // Touches every context in the grammar (comments, all four string
        // kinds + escapes + placeholders, annotations, declarations,
        // keywords, constants, every number form, node paths, types,
        // calls, operators), so a bad regex anywhere surfaces here rather
        // than as a panic while someone is typing.
        let source = r#"@tool
@export_range(0, 10) var speed: float = 1.5e3
class_name Player extends CharacterBody2D
signal died(reason)
enum State { IDLE, RUN }
const MAX_HP := 0xFF
var mask = 0b1010 + .5 + 1_000
# a comment
func _ready() -> void:
	var s = "esc\n %s %d" % ["a", 1]
	var t = 'single \' quote'
	var n = &"name"
	var raw = r"C:\raw"
	var path = $"Sprite/Child"
	var label = %Unique
	var body = $Body/Collision
	var v = Vector2(1, 2) * 3
	if x and not y or z in w:
		return null
	"""
	triple
	"""
	'''
	triple single
	'''
"#;
        let rows = spans_for("player.gd", source);
        assert!(rows.len() > 20);
    }

    #[test]
    fn gdscript_scopes_get_distinct_colors() {
        let rows = spans_for(
            "a.gd",
            "func _ready():\n\tvar speed = 10\n\treturn \"s\" # c\nextends Node2D\n@export var x\nsignal hit\nvar y = true",
        );
        let default = fg_of(&rows[1], "speed").expect("plain identifier has a color");

        // Every one of these should differ from plain-identifier color.
        for (row, text, what) in [
            (0, "func", "func keyword"),
            (0, "_ready", "function name"),
            (1, "var", "var keyword"),
            (1, "10", "number"),
            (2, "return", "control keyword"),
            (2, "s", "string contents"),
            (3, "extends", "extends"),
            (3, "Node2D", "class name"),
            (4, "@export", "annotation"),
            (5, "signal", "signal keyword"),
            (6, "true", "constant"),
        ] {
            let c = fg_of(&rows[row], text)
                .unwrap_or_else(|| panic!("no span for {what}: {text:?} in {:?}", rows[row]));
            assert_ne!(
                c, default,
                "{what} ({text:?}) should not be the plain color"
            );
        }
        // Comments get their own color too.
        assert!(rows[2]
            .iter()
            .any(|(s, t)| t.contains("# c") && s.fg.is_some() && s.fg != Some(default)));
    }

    #[test]
    fn gdscript_comment_hides_keywords_and_strings_hide_comments() {
        let rows = spans_for("a.gd", "# func var if\nvar s = \"# not a comment\"");

        // The commented-out keywords are one comment span, not
        // individually keyword-colored.
        assert_eq!(rows[0].len(), 1, "{:?}", rows[0]);
        let comment_color = rows[0][0].0.fg;

        // The `#` inside a string is colored as string content, not as a
        // comment.
        let in_string = rows[1]
            .iter()
            .find(|(_, t)| t.contains("not a comment"))
            .expect("string content span");
        assert_ne!(in_string.0.fg, comment_color);
    }

    #[test]
    fn gdscript_unterminated_string_does_not_swallow_following_lines() {
        let rows = spans_for("a.gd", "var s = \"oops\nfunc f():\n\tpass");
        let default = highlighter_for("a.gd").highlight(&["x".to_string()], 1)[0][0]
            .0
            .fg;
        // Line 2's `func` is still a keyword (colored, not default).
        let func = fg_of(&rows[1], "func").expect("func span");
        assert_ne!(Some(func), default);
    }

    fn colors_of(theme: &str, enabled: bool) -> (Vec<Option<Color>>, Option<String>) {
        let (h, warning) = Highlighter::new(Language::Rust, enabled, theme);
        let rows = h.highlight(&["fn main() { let x = 1; }".to_string()], 1);
        (rows[0].iter().map(|(s, _)| s.fg).collect(), warning)
    }

    #[test]
    fn highlighting_can_be_switched_off() {
        let (off, warning) = colors_of(DEFAULT_THEME, false);
        assert!(warning.is_none());
        assert!(off.iter().all(|c| c.is_none()), "unstyled: {off:?}");
        let (on, _) = colors_of(DEFAULT_THEME, true);
        assert!(on.iter().any(|c| c.is_some()));
    }

    #[test]
    fn the_theme_setting_changes_the_colors() {
        let (dark, w1) = colors_of("base16-ocean.dark", true);
        let (light, w2) = colors_of("InspiredGitHub", true);
        assert!(w1.is_none() && w2.is_none());
        assert_ne!(dark, light, "different themes, different colors");
        let (case_insensitive, w3) = colors_of("inspiredgithub", true);
        assert!(w3.is_none());
        assert_eq!(light, case_insensitive);
    }

    #[test]
    fn an_unknown_theme_warns_and_falls_back_to_the_default() {
        let (colors, warning) = colors_of("no-such-theme", true);
        let (default, _) = colors_of(DEFAULT_THEME, true);
        assert_eq!(colors, default);
        let warning = warning.expect("a warning");
        assert!(
            warning.contains("no-such-theme") && warning.contains("base16-ocean.dark"),
            "{warning}"
        );
    }

    #[test]
    fn a_tmtheme_file_can_be_used_as_the_theme() {
        let path = std::env::temp_dir().join(format!("pc_theme_{}.tmTheme", std::process::id()));
        std::fs::write(
            &path,
            r##"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>name</key><string>Test</string>
<key>settings</key><array>
<dict><key>settings</key><dict>
<key>foreground</key><string>#112233</string>
<key>background</key><string>#000000</string>
</dict></dict>
<dict><key>scope</key><string>keyword</string><key>settings</key><dict>
<key>foreground</key><string>#ff0000</string>
</dict></dict>
</array></dict></plist>"##,
        )
        .unwrap();
        let (colors, warning) = colors_of(path.to_str().unwrap(), true);
        assert!(warning.is_none(), "{warning:?}");
        assert!(
            colors.contains(&Some(Color::Rgb(0xff, 0, 0))),
            "keywords red: {colors:?}"
        );
        assert!(
            colors.contains(&Some(Color::Rgb(0x11, 0x22, 0x33))),
            "rest uses the theme foreground: {colors:?}"
        );

        std::fs::write(&path, "not a theme").unwrap();
        let (_, warning) = colors_of(path.to_str().unwrap(), true);
        assert!(warning.expect("warns").contains("can't load"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_language_decides_the_grammar_not_the_file_name() {
        // `[associations]` maps odd extensions to a language; the
        // highlighter follows the language it is given.
        let (h, _) = Highlighter::new(Language::Rust, true, DEFAULT_THEME);
        let rows = h.highlight(&["fn main() {}".to_string()], 1);
        assert!(rows[0].iter().any(|(s, _)| s.fg.is_some()));
    }
}

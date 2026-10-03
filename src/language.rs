//! What Pis Code knows about each file type it supports: which grammar to
//! highlight with, and how the editor's "smart" behaviors apply.

use std::path::Path;

/// How Enter splits an autoclosed bracket pair when the cursor sits
/// between the two halves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BraceSplit {
    /// No split: just the generic deepen-after-an-open-bracket rule. Used
    /// where `{}` is a data literal rather than a block (Python, GDScript).
    None,
    /// The opening bracket stays on its line; an indented line and the
    /// closing bracket on its own line follow. How JSON is written.
    KAndR,
    /// The opening brace moves to its own line, matching the signature's
    /// indent (Visual Studio's C# style — and the one chosen here for every
    /// language whose `{}` is a block).
    Allman,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    CSharp,
    JavaScript,
    Rust,
    Python,
    GdScript,
    Html,
    Css,
    Json,
    Xml,
    Yaml,
    Markdown,
    Other,
}

impl Language {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("cs") => Language::CSharp,
            Some("js" | "mjs" | "cjs" | "jsx") => Language::JavaScript,
            Some("rs") => Language::Rust,
            Some("py" | "py3" | "pyw" | "pyi") => Language::Python,
            Some("gd") => Language::GdScript,
            Some("html" | "htm" | "xhtml") => Language::Html,
            Some("css") => Language::Css,
            Some("json") => Language::Json,
            // Besides plain XML: the XML-based formats a C# project is
            // full of (.csproj and friends) and common SVG/XAML/XSL.
            Some(
                "xml" | "svg" | "xaml" | "xsl" | "xslt" | "xsd" | "csproj" | "vbproj" | "fsproj"
                | "props" | "targets" | "resx" | "plist",
            ) => Language::Xml,
            Some("yaml" | "yml") => Language::Yaml,
            Some("md" | "markdown" | "mdown") => Language::Markdown,
            _ => Language::Other,
        }
    }

    /// The extension to look the grammar up by in syntect's bundled set.
    /// `None` for GDScript (which ships its own grammar) and for types we
    /// deliberately leave unstyled.
    pub fn bundled_syntax_extension(self) -> Option<&'static str> {
        match self {
            Language::CSharp => Some("cs"),
            Language::JavaScript => Some("js"),
            Language::Rust => Some("rs"),
            Language::Python => Some("py"),
            Language::Html => Some("html"),
            Language::Css => Some("css"),
            Language::Json => Some("json"),
            Language::Xml => Some("xml"),
            Language::Yaml => Some("yaml"),
            Language::Markdown => Some("md"),
            Language::GdScript | Language::Other => None,
        }
    }

    /// `{}` is a block in C#, JavaScript, Rust and CSS, so Enter between an
    /// autoclosed pair splits it Allman-style; in JSON it's a data literal
    /// that is conventionally written K&R-style. Elsewhere there's no split.
    pub fn brace_split(self) -> BraceSplit {
        match self {
            Language::CSharp | Language::JavaScript | Language::Rust | Language::Css => {
                BraceSplit::Allman
            }
            Language::Json => BraceSplit::KAndR,
            _ => BraceSplit::None,
        }
    }

    /// Indentation-based blocks: a trailing `:` opens one.
    pub fn colon_deepens_indent(self) -> bool {
        matches!(self, Language::Python | Language::GdScript | Language::Yaml)
    }

    /// `'` is a lifetime marker in Rust and an apostrophe in prose, so
    /// autoclosing it into `''` there is more nuisance than help.
    pub fn autocloses_single_quote(self) -> bool {
        !matches!(self, Language::Rust | Language::Markdown)
    }

    /// Typing `>` after an opening tag inserts the matching closing tag.
    pub fn auto_closes_tags(self) -> bool {
        matches!(self, Language::Html | Language::Xml)
    }

    pub fn is_html(self) -> bool {
        self == Language::Html
    }

    /// Indent unit for a file that doesn't show one yet. GDScript's style
    /// guide (and Godot's editor) uses tabs, and mixing tabs with spaces is
    /// a parse error there. YAML conventionally uses two spaces (and
    /// forbids tabs outright).
    pub fn default_indent(self) -> &'static str {
        match self {
            Language::GdScript => "\t",
            Language::Yaml => "  ",
            _ => "    ",
        }
    }

    /// Tabs aren't legal indentation in YAML.
    pub fn allows_tab_indent(self) -> bool {
        self != Language::Yaml
    }

    /// Whether Ctrl+Shift+S has a checker to run for this file type.
    pub fn has_checker(self) -> bool {
        !matches!(self, Language::Css | Language::Markdown | Language::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(path: &str) -> Language {
        Language::from_path(Path::new(path))
    }

    #[test]
    fn detects_language_from_extension_ignoring_case() {
        assert!(lang("a.cs") == Language::CSharp);
        assert!(lang("a.HTML") == Language::Html);
        assert!(lang("a.htm") == Language::Html);
        assert!(lang("a.css") == Language::Css);
        assert!(lang("a.JSON") == Language::Json);
        assert!(lang("a.yml") == Language::Yaml);
        assert!(lang("a.md") == Language::Markdown);
        assert!(lang("a.gd") == Language::GdScript);
        assert!(lang("a.txt") == Language::Other);
        assert!(lang("Makefile") == Language::Other);
    }

    #[test]
    fn xml_based_project_files_are_xml() {
        for p in ["App.csproj", "a.svg", "Main.xaml", "Directory.Build.props"] {
            assert!(lang(p) == Language::Xml, "{p}");
        }
    }
}

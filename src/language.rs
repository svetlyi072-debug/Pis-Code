//! What Pis Code knows about each file type it supports: which grammar to
//! highlight with, and how the editor's "smart" behaviors apply.

use std::path::Path;

/// How Enter splits an autoclosed bracket pair when the cursor sits
/// between the two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Every language, in the order they're documented.
pub const ALL: &[Language] = &[
    Language::CSharp,
    Language::JavaScript,
    Language::Rust,
    Language::Python,
    Language::GdScript,
    Language::Html,
    Language::Css,
    Language::Json,
    Language::Xml,
    Language::Yaml,
    Language::Markdown,
    Language::Other,
];

impl Language {
    /// The name used for this language in the config file
    /// (`[languages.<id>]`, `[associations]`).
    pub fn id(self) -> &'static str {
        match self {
            Language::CSharp => "csharp",
            Language::JavaScript => "javascript",
            Language::Rust => "rust",
            Language::Python => "python",
            Language::GdScript => "gdscript",
            Language::Html => "html",
            Language::Css => "css",
            Language::Json => "json",
            Language::Xml => "xml",
            Language::Yaml => "yaml",
            Language::Markdown => "markdown",
            Language::Other => "plain",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ALL.iter().copied().find(|l| l.id() == id)
    }

    /// Like [`Language::from_path`], but first consults the user's
    /// `[associations]`: an exact file name (`Jenkinsfile`) or an extension
    /// without the dot (`vue`, case-insensitive) mapped to a language id.
    /// An association naming an unknown language is ignored.
    pub fn from_path_with(
        path: &Path,
        associations: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        let by_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| associations.get(n));
        let by_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .and_then(|e| {
                associations
                    .iter()
                    .find(|(k, _)| k.trim_start_matches('.').eq_ignore_ascii_case(&e))
                    .map(|(_, v)| v)
            });
        by_name
            .or(by_ext)
            .and_then(|id| Language::from_id(id))
            .unwrap_or_else(|| Language::from_path(path))
    }

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

    /// Whether files of this type are made of tags (HTML, XML) — what tag
    /// auto-closing and the Enter-between-tags split work on.
    pub fn has_tags(self) -> bool {
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

    #[test]
    fn ids_round_trip() {
        for &l in ALL {
            assert!(Language::from_id(l.id()) == Some(l), "{}", l.id());
        }
        assert!(Language::from_id("cobol").is_none());
    }

    #[test]
    fn associations_override_detection() {
        let mut assoc = std::collections::BTreeMap::new();
        assoc.insert("vue".to_string(), "html".to_string());
        assoc.insert(".JSONC".to_string(), "json".to_string());
        assoc.insert("Jenkinsfile".to_string(), "python".to_string());
        assoc.insert("cs".to_string(), "plain".to_string()); // beats the built-in
        assoc.insert("zzz".to_string(), "nonsense".to_string()); // ignored

        let l = |p: &str| Language::from_path_with(Path::new(p), &assoc);
        assert!(l("App.vue") == Language::Html);
        assert!(
            l("a/b/settings.jsonc") == Language::Json,
            "leading dot and case ignored"
        );
        assert!(l("Jenkinsfile") == Language::Python, "exact file name");
        assert!(l("Program.cs") == Language::Other, "user association wins");
        assert!(
            l("a.zzz") == Language::Other,
            "unknown language id falls through"
        );
        assert!(l("a.rs") == Language::Rust, "unrelated files unaffected");
    }
}

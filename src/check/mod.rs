use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use regex::Regex;

use crate::editor::Language;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone)]
pub struct Diagnostic {
    /// 0-indexed, to match the editor's own coordinates.
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

pub enum CheckMessage {
    Finished(Vec<Diagnostic>),
    ToolMissing(&'static str),
    Failed(String),
}

/// Runs the appropriate checker for the file's language on a background
/// thread (so the editor never blocks on it) and reports parsed
/// diagnostics back over a channel. Only one check runs at a time.
pub struct Checker {
    receiver: Option<Receiver<CheckMessage>>,
    running: bool,
}

impl Checker {
    pub fn new() -> Self {
        Self {
            receiver: None,
            running: false,
        }
    }

    /// Starts a check for `file_path` unless one is already in flight.
    /// Returns `true` if a new check was started.
    pub fn start(&mut self, file_path: PathBuf, language: Language) -> bool {
        if self.running {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.running = true;
        thread::spawn(move || {
            let _ = tx.send(run_check(&file_path, language));
        });
        true
    }

    /// Non-blocking: returns a message if the running check has finished.
    pub fn poll(&mut self) -> Option<CheckMessage> {
        let rx = self.receiver.as_ref()?;
        match rx.try_recv() {
            Ok(msg) => {
                self.running = false;
                Some(msg)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.running = false;
                self.receiver = None;
                None
            }
        }
    }
}

fn run_check(file_path: &Path, language: Language) -> CheckMessage {
    match language {
        Language::CSharp => run_check_csharp(file_path),
        Language::Rust => run_check_rust(file_path),
        Language::JavaScript => run_check_javascript(file_path),
        Language::Python => run_check_python(file_path),
        Language::GdScript => run_check_gdscript(file_path),
        Language::Other => {
            CheckMessage::Failed("No checker available for this file type".to_string())
        }
    }
}

/// Compares `diag_file` (as printed by a compiler/interpreter, which may
/// be relative, absolute, or from a different working directory) against
/// the file we're actually checking, canonicalizing both sides so
/// differently-formatted-but-equal paths still match.
fn matches_target_file(diag_file: &Path, target: &Path) -> bool {
    match (
        std::fs::canonicalize(target).ok(),
        std::fs::canonicalize(diag_file).ok(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => diag_file.file_name() == target.file_name(),
    }
}

// ==================== C# (dotnet build) ====================

fn run_check_csharp(file_path: &Path) -> CheckMessage {
    let version_output = match Command::new("dotnet").arg("--version").output() {
        Ok(o) if o.status.success() => o,
        _ => return CheckMessage::ToolMissing("dotnet"),
    };
    let version = String::from_utf8_lossy(&version_output.stdout);
    let major = version.split('.').next().unwrap_or("8").trim().to_string();
    let target_framework = format!("net{major}.0");

    let project = match find_ancestor_project(file_path, "csproj") {
        Some(csproj) => csproj,
        None => match write_synthetic_csproj(file_path, &target_framework) {
            Ok(csproj) => csproj,
            Err(e) => return CheckMessage::Failed(e.to_string()),
        },
    };

    let output = match Command::new("dotnet")
        .args(["build", "--nologo", "-v", "q"])
        .arg(&project)
        .output()
    {
        Ok(o) => o,
        Err(e) => return CheckMessage::Failed(e.to_string()),
    };

    CheckMessage::Finished(parse_csharp_diagnostics(
        &combined_output(&output),
        file_path,
    ))
}

/// Looks for the nearest `*.<ext>` project file in `file_path`'s
/// directory or any ancestor, so files that already live in a real
/// project get built/checked with their actual references instead of a
/// bare synthetic one.
fn find_ancestor_project(file_path: &Path, ext: &str) -> Option<PathBuf> {
    let mut dir = file_path.parent()?;
    loop {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                    return Some(path);
                }
            }
        }
        dir = dir.parent()?;
    }
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// For a loose file with no project, builds a minimal throwaway project
/// under a stable temp directory (keyed by the file's own path, so repeat
/// checks reuse the same `obj`/`bin` and stay fast).
fn write_synthetic_csproj(file_path: &Path, target_framework: &str) -> anyhow::Result<PathBuf> {
    let absolute = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let project_dir = stable_temp_dir(&absolute);

    let csproj = format!(
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>{target_framework}</TargetFramework>
    <Nullable>disable</Nullable>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <GenerateAssemblyInfo>false</GenerateAssemblyInfo>
  </PropertyGroup>
  <ItemGroup>
    <Compile Include="{}" />
  </ItemGroup>
</Project>
"#,
        absolute.display()
    );

    let project_path = project_dir.join("check.csproj");
    std::fs::write(&project_path, csproj)?;
    Ok(project_path)
}

/// A stable per-file temp directory (keyed by the file's own absolute
/// path), reused across repeated checks so build caches stay warm.
fn stable_temp_dir(absolute_file: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    absolute_file.hash(&mut hasher);
    let key = hasher.finish();
    let dir = std::env::temp_dir()
        .join("pis-code-check")
        .join(format!("{key:x}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn parse_csharp_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let re = Regex::new(r"^(?P<file>.+)\((?P<line>\d+),(?P<col>\d+)\): (?P<sev>error|warning) (?P<code>[A-Za-z0-9]+): (?P<msg>.*)$")
        .expect("static regex is valid");

    let mut diagnostics = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let Some(caps) = re.captures(line) else {
            continue;
        };
        if !matches_target_file(Path::new(&caps["file"]), file_path) {
            continue;
        }

        let line_no: usize = caps["line"].parse().unwrap_or(1);
        let col_no: usize = caps["col"].parse().unwrap_or(1);
        let severity = if &caps["sev"] == "error" {
            Severity::Error
        } else {
            Severity::Warning
        };

        let mut message = caps["msg"].to_string();
        if let Some(idx) = message.rfind(" [") {
            if message.ends_with(']') {
                message.truncate(idx);
            }
        }

        diagnostics.push(Diagnostic {
            line: line_no.saturating_sub(1),
            col: col_no.saturating_sub(1),
            severity,
            code: caps["code"].to_string(),
            message,
        });
    }
    diagnostics
}

// ==================== Rust (cargo check / rustc) ====================

fn run_check_rust(file_path: &Path) -> CheckMessage {
    if Command::new("rustc").arg("--version").output().is_err() {
        return CheckMessage::ToolMissing("rustc");
    }

    let output = match find_ancestor_project(file_path, "toml")
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("Cargo.toml"))
    {
        Some(manifest) => Command::new("cargo")
            .args(["check", "--message-format=human", "--manifest-path"])
            .arg(&manifest)
            .output(),
        None => {
            let absolute =
                std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
            let out_dir = stable_temp_dir(&absolute);
            Command::new("rustc")
                .args([
                    "--edition",
                    "2021",
                    "--crate-type",
                    "lib",
                    "--emit=metadata",
                ])
                .arg("-o")
                .arg(out_dir.join("check.rmeta"))
                .arg(&absolute)
                .output()
        }
    };

    let output = match output {
        Ok(o) => o,
        Err(e) => return CheckMessage::Failed(e.to_string()),
    };

    CheckMessage::Finished(parse_rustc_diagnostics(
        &combined_output(&output),
        file_path,
    ))
}

/// rustc/cargo print each diagnostic as a header line (`error[E0425]: msg`
/// or `warning: msg`) followed shortly by a `--> file:line:col` location
/// line — unlike C#'s MSBuild format, these are two separate lines.
fn parse_rustc_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let header_re = Regex::new(r"^(error|warning)(?:\[(\w+)\])?: (.*)$").expect("valid regex");
    let loc_re = Regex::new(r"^\s*-->\s*(.+):(\d+):(\d+)$").expect("valid regex");

    let lines: Vec<&str> = output.lines().collect();
    let mut diagnostics = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(header) = header_re.captures(line) else {
            continue;
        };
        let Some(loc) = lines[i + 1..(i + 4).min(lines.len())]
            .iter()
            .find_map(|l| loc_re.captures(l))
        else {
            continue;
        };
        if !matches_target_file(Path::new(&loc[1]), file_path) {
            continue;
        }

        let severity = if &header[1] == "error" {
            Severity::Error
        } else {
            Severity::Warning
        };
        diagnostics.push(Diagnostic {
            line: loc[2].parse::<usize>().unwrap_or(1).saturating_sub(1),
            col: loc[3].parse::<usize>().unwrap_or(1).saturating_sub(1),
            severity,
            code: header
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            message: header[3].to_string(),
        });
    }
    diagnostics
}

// ==================== JavaScript (node --check) ====================

fn run_check_javascript(file_path: &Path) -> CheckMessage {
    let output = match Command::new("node").arg("--check").arg(file_path).output() {
        Ok(o) => o,
        Err(_) => return CheckMessage::ToolMissing("node"),
    };
    CheckMessage::Finished(parse_node_diagnostics(&combined_output(&output), file_path))
}

/// `node --check` prints the offending `path:line`, then the source line,
/// then a caret line, then (after a blank line) `SyntaxError: message`.
/// It stops at the first error, so there's at most one diagnostic.
fn parse_node_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let path_line_re = Regex::new(r"^(.+):(\d+)$").expect("valid regex");
    let lines: Vec<&str> = output.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = path_line_re.captures(line) else {
            continue;
        };
        if !matches_target_file(Path::new(&caps[1]), file_path) {
            continue;
        }
        let line_no: usize = caps[2].parse().unwrap_or(1);

        let mut col = 0usize;
        for l in &lines[i + 1..(i + 6).min(lines.len())] {
            if let Some(pos) = l.find('^') {
                col = pos;
            }
            if let Some(msg) = l.trim_start().strip_prefix_error_message() {
                return vec![Diagnostic {
                    line: line_no.saturating_sub(1),
                    col,
                    severity: Severity::Error,
                    code: String::new(),
                    message: msg,
                }];
            }
        }
    }
    Vec::new()
}

// ==================== Python (py_compile) ====================

fn run_check_python(file_path: &Path) -> CheckMessage {
    let python = if Command::new("python3").arg("--version").output().is_ok() {
        "python3"
    } else if Command::new("python").arg("--version").output().is_ok() {
        "python"
    } else {
        return CheckMessage::ToolMissing("python3");
    };

    let output = match Command::new(python)
        .args(["-m", "py_compile"])
        .arg(file_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return CheckMessage::Failed(e.to_string()),
    };
    CheckMessage::Finished(parse_python_diagnostics(
        &combined_output(&output),
        file_path,
    ))
}

/// `py_compile` prints `  File "path", line N`, then the source line,
/// then a caret line, then `SyntaxError: message` (or IndentationError,
/// TabError, ...). Like node, it stops at the first error.
fn parse_python_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let file_line_re = Regex::new(r#"^\s*File "(.+)", line (\d+)$"#).expect("valid regex");
    let lines: Vec<&str> = output.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = file_line_re.captures(line) else {
            continue;
        };
        if !matches_target_file(Path::new(&caps[1]), file_path) {
            continue;
        }
        let line_no: usize = caps[2].parse().unwrap_or(1);

        let mut col = 0usize;
        for l in &lines[i + 1..(i + 5).min(lines.len())] {
            if let Some(pos) = l.find('^') {
                col = pos;
            }
            if let Some(msg) = l.trim_start().strip_prefix_error_message() {
                return vec![Diagnostic {
                    line: line_no.saturating_sub(1),
                    col,
                    severity: Severity::Error,
                    code: String::new(),
                    message: msg,
                }];
            }
        }
    }
    Vec::new()
}

/// Both Node's and Python's checkers end their message on a line shaped
/// like `SomeKindOfError: the actual message`.
trait StripErrorMessage {
    fn strip_prefix_error_message(&self) -> Option<String>;
}

impl StripErrorMessage for str {
    fn strip_prefix_error_message(&self) -> Option<String> {
        let idx = self.find("Error: ")?;
        // Require the part before "Error: " to look like an identifier
        // (e.g. "Syntax", "Indentation"), not an unrelated line that
        // happens to contain that substring.
        let prefix = &self[..idx];
        if prefix.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(self[idx + "Error: ".len()..].to_string())
        } else {
            None
        }
    }
}

// ==================== GDScript (Godot CLI) ====================
//
// Best-effort: Godot's `--check-only` output format is implemented from
// its documented CLI flags, not verified against a real Godot binary (not
// available in this environment). If it doesn't parse into a clean
// per-line diagnostic, we still surface the raw message rather than
// silently failing.

fn run_check_gdscript(file_path: &Path) -> CheckMessage {
    let godot_bin = ["godot4", "godot"]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok());
    let Some(godot_bin) = godot_bin else {
        return CheckMessage::ToolMissing("godot");
    };

    let output = match Command::new(godot_bin)
        .args(["--headless", "--check-only", "--script"])
        .arg(file_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return CheckMessage::Failed(e.to_string()),
    };

    if output.status.success() {
        return CheckMessage::Finished(Vec::new());
    }

    let combined = combined_output(&output);
    let diagnostics = parse_gdscript_diagnostics(&combined, file_path);
    if diagnostics.is_empty() {
        // Couldn't parse a structured location out of Godot's output;
        // surface whatever it said instead of pretending nothing's wrong.
        let summary: String = combined
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join(" | ");
        CheckMessage::Failed(if summary.is_empty() {
            "Godot check failed with no output".to_string()
        } else {
            summary
        })
    } else {
        CheckMessage::Finished(diagnostics)
    }
}

/// Godot's parse-error banner looks like:
/// `SCRIPT ERROR: Parse Error: <message>` followed by
/// `          at: GDScript::reload (res://path.gd:LINE)`.
fn parse_gdscript_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let err_re =
        Regex::new(r"(?i)^\s*SCRIPT ERROR:\s*(?:Parse Error:\s*)?(.*)$").expect("valid regex");
    let at_re = Regex::new(r"\(([^():]+):(\d+)\)\s*$").expect("valid regex");

    let lines: Vec<&str> = output.lines().collect();
    let mut diagnostics = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(err) = err_re.captures(line) else {
            continue;
        };
        let Some(at) = lines[i + 1..(i + 3).min(lines.len())]
            .iter()
            .find_map(|l| at_re.captures(l))
        else {
            continue;
        };
        if !matches_target_file(Path::new(&at[1]), file_path) {
            continue;
        }
        diagnostics.push(Diagnostic {
            line: at[2].parse::<usize>().unwrap_or(1).saturating_sub(1),
            col: 0,
            severity: Severity::Error,
            code: String::new(),
            message: err[1].trim().to_string(),
        });
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Real files, so `std::fs::canonicalize` in the matcher succeeds —
    // the target-file filter depends on the path actually existing.
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    fn temp_file(name_hint: &str, ext: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pc_check_test_{name_hint}_{n}.{ext}"));
        std::fs::write(&path, "// test\n").unwrap();
        path
    }

    #[test]
    fn parses_csharp_error_and_warning_for_target_file() {
        let file = temp_file("target", "cs");
        let output = format!(
            "{}(4,32): error CS1002: ; expected [/tmp/proj.csproj]\n{}(2,5): warning CS0168: variable declared but never used [/tmp/proj.csproj]",
            file.display(),
            file.display()
        );

        let diags = parse_csharp_diagnostics(&output, &file);
        assert_eq!(diags.len(), 2);

        assert_eq!(diags[0].line, 3); // 1-indexed 4 -> 0-indexed 3
        assert_eq!(diags[0].col, 31);
        assert!(matches!(diags[0].severity, Severity::Error));
        assert_eq!(diags[0].code, "CS1002");
        assert_eq!(diags[0].message, "; expected");

        assert_eq!(diags[1].line, 1);
        assert!(matches!(diags[1].severity, Severity::Warning));
        assert_eq!(diags[1].code, "CS0168");

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn csharp_ignores_diagnostics_for_other_files() {
        let file = temp_file("target2", "cs");
        let other = temp_file("other", "cs");
        let output = format!(
            "{}(1,1): error CS0000: unrelated file's problem",
            other.display()
        );

        let diags = parse_csharp_diagnostics(&output, &file);
        assert!(diags.is_empty());

        std::fs::remove_file(&file).ok();
        std::fs::remove_file(&other).ok();
    }

    #[test]
    fn csharp_ignores_unrelated_output_lines() {
        let file = temp_file("target3", "cs");
        let output =
            "Determining projects to restore...\n  Restored /tmp/proj.csproj\nBuild succeeded.";

        let diags = parse_csharp_diagnostics(output, &file);
        assert!(diags.is_empty());

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn severity_ord_ranks_error_above_warning() {
        assert!(Severity::Error > Severity::Warning);
    }

    #[test]
    fn parses_rustc_error_with_code() {
        let file = temp_file("target", "rs");
        let output = format!(
            "error[E0425]: cannot find value `y` in this scope\n --> {}:3:20\n  |\n",
            file.display()
        );
        let diags = parse_rustc_diagnostics(&output, &file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].col, 19);
        assert!(matches!(diags[0].severity, Severity::Error));
        assert_eq!(diags[0].code, "E0425");
        assert_eq!(diags[0].message, "cannot find value `y` in this scope");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn parses_rustc_warning_without_code() {
        let file = temp_file("target", "rs");
        let output = format!(
            "warning: unused variable: `x`\n --> {}:2:9\n  |\n",
            file.display()
        );
        let diags = parse_rustc_diagnostics(&output, &file);
        assert_eq!(diags.len(), 1);
        assert!(matches!(diags[0].severity, Severity::Warning));
        assert_eq!(diags[0].code, "");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn parses_node_syntax_error() {
        let file = temp_file("target", "js");
        let output = format!(
            "{}:3\n    let x = ;\n            ^\n\nSyntaxError: Unexpected token ';'\n    at wrapSafe (node:internal/modules/cjs/loader:1804:18)\n",
            file.display()
        );
        let diags = parse_node_diagnostics(&output, &file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].col, 12);
        assert_eq!(diags[0].message, "Unexpected token ';'");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn parses_python_syntax_error() {
        let file = temp_file("target", "py");
        let output = format!(
            "  File \"{}\", line 3\n    if x == 1\n             ^\nSyntaxError: expected ':'\n",
            file.display()
        );
        let diags = parse_python_diagnostics(&output, &file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].col, 13);
        assert_eq!(diags[0].message, "expected ':'");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn parses_gdscript_parse_error() {
        let file = temp_file("target", "gd");
        let output = format!(
            "SCRIPT ERROR: Parse Error: Expected end of statement, found \":\" instead.\n          at: GDScript::reload ({}:3)\n",
            file.display()
        );
        let diags = parse_gdscript_diagnostics(&output, &file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 2);
        assert_eq!(
            diags[0].message,
            "Expected end of statement, found \":\" instead."
        );
        std::fs::remove_file(&file).ok();
    }
}

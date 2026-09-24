use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use regex::Regex;

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
    ToolMissing,
    Failed(String),
}

/// Runs `dotnet build` against the edited file on a background thread (so
/// the editor never blocks on it) and reports the parsed diagnostics back
/// over a channel. Only one check runs at a time.
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
    pub fn start(&mut self, file_path: PathBuf) -> bool {
        if self.running {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.running = true;
        thread::spawn(move || {
            let _ = tx.send(run_check(&file_path));
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

fn run_check(file_path: &Path) -> CheckMessage {
    let version_output = match Command::new("dotnet").arg("--version").output() {
        Ok(o) if o.status.success() => o,
        _ => return CheckMessage::ToolMissing,
    };
    let version = String::from_utf8_lossy(&version_output.stdout);
    let major = version.split('.').next().unwrap_or("8").trim().to_string();
    let target_framework = format!("net{major}.0");

    let project = match find_ancestor_csproj(file_path) {
        Some(csproj) => csproj,
        None => match write_synthetic_project(file_path, &target_framework) {
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

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    CheckMessage::Finished(parse_diagnostics(&combined, file_path))
}

/// Looks for the nearest `*.csproj` in `file_path`'s directory or any
/// ancestor, so files that already live in a real project get built with
/// their actual references instead of a bare synthetic one.
fn find_ancestor_csproj(file_path: &Path) -> Option<PathBuf> {
    let mut dir = file_path.parent()?;
    loop {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("csproj") {
                    return Some(path);
                }
            }
        }
        dir = dir.parent()?;
    }
}

/// For a loose file with no project, builds a minimal throwaway project
/// under a stable temp directory (keyed by the file's own path, so repeat
/// checks reuse the same `obj`/`bin` and stay fast).
fn write_synthetic_project(file_path: &Path, target_framework: &str) -> anyhow::Result<PathBuf> {
    let absolute = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());

    let mut hasher = DefaultHasher::new();
    absolute.hash(&mut hasher);
    let key = hasher.finish();

    let project_dir = std::env::temp_dir()
        .join("pis-code-check")
        .join(format!("{key:x}"));
    std::fs::create_dir_all(&project_dir)?;

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

fn parse_diagnostics(output: &str, file_path: &Path) -> Vec<Diagnostic> {
    let re = Regex::new(r"^(?P<file>.+)\((?P<line>\d+),(?P<col>\d+)\): (?P<sev>error|warning) (?P<code>[A-Za-z0-9]+): (?P<msg>.*)$")
        .expect("static regex is valid");

    let target_canonical = std::fs::canonicalize(file_path).ok();
    let target_name = file_path.file_name();

    let mut diagnostics = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let Some(caps) = re.captures(line) else {
            continue;
        };

        let diag_file = Path::new(&caps["file"]);
        let matches_target = match (&target_canonical, std::fs::canonicalize(diag_file).ok()) {
            (Some(a), Some(b)) => *a == b,
            _ => diag_file.file_name() == target_name,
        };
        if !matches_target {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Real files, so `std::fs::canonicalize` in parse_diagnostics succeeds
    // — the target-file filter depends on the path actually existing.
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    fn temp_cs_file(name_hint: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pc_check_test_{name_hint}_{n}.cs"));
        std::fs::write(&path, "// test\n").unwrap();
        path
    }

    #[test]
    fn parses_error_and_warning_for_target_file() {
        let file = temp_cs_file("target");
        let output = format!(
            "{}(4,32): error CS1002: ; expected [/tmp/proj.csproj]\n{}(2,5): warning CS0168: variable declared but never used [/tmp/proj.csproj]",
            file.display(),
            file.display()
        );

        let diags = parse_diagnostics(&output, &file);
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
    fn ignores_diagnostics_for_other_files() {
        let file = temp_cs_file("target2");
        let other = temp_cs_file("other");
        let output = format!(
            "{}(1,1): error CS0000: unrelated file's problem",
            other.display()
        );

        let diags = parse_diagnostics(&output, &file);
        assert!(diags.is_empty());

        std::fs::remove_file(&file).ok();
        std::fs::remove_file(&other).ok();
    }

    #[test]
    fn ignores_unrelated_output_lines() {
        let file = temp_cs_file("target3");
        let output =
            "Determining projects to restore...\n  Restored /tmp/proj.csproj\nBuild succeeded.";

        let diags = parse_diagnostics(output, &file);
        assert!(diags.is_empty());

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn severity_ord_ranks_error_above_warning() {
        assert!(Severity::Error > Severity::Warning);
    }
}

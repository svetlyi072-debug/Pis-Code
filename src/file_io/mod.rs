use anyhow::Result;
use std::fs;
use std::path::Path;

/// Load a file's contents as lines, or return a single empty line if the
/// file does not exist yet (a brand new buffer, like `nano` would give you).
pub fn load_or_create(path: &Path) -> Result<Vec<String>> {
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Ok(lines)
    } else {
        Ok(vec![String::new()])
    }
}

/// Save the buffer to disk, always ending in a trailing newline.
pub fn save(path: &Path, lines: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut contents = lines.join("\n");
    contents.push('\n');
    fs::write(path, contents)?;
    Ok(())
}

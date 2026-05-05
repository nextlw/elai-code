//! NOTE: progress reporting follows the TUI-safe pattern documented at
//! `rust/docs/progress-pattern.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runtime::ProgressReporter;

const RECENT_KEEP: usize = 20;
const SUMMARY_OPEN: &str = "<!-- [COMPRESSED SUMMARY] -->";
const SUMMARY_CLOSE: &str = "<!-- [/COMPRESSED SUMMARY] -->";

/// Result of parsing a memory file into sections.
pub struct MemoryParseResult {
    /// Entries older than the most recent 20 — candidates for compression.
    pub old_entries: Vec<String>,
    /// The most recent entries (up to 20) — preserved intact.
    pub recent_entries: Vec<String>,
    /// Previously compressed summary block, if present.
    pub existing_summary: Option<String>,
}

/// Result of a completed dream compression.
pub struct DreamResult {
    pub entries_compressed: usize,
    pub before_size: usize,
    pub after_size: usize,
    pub summary: String,
}

/// Find the memory file in `cwd`, checking in priority order.
pub fn find_memory_file(cwd: &Path) -> Option<PathBuf> {
    let candidates = [
        "ELAI.md",
        "CLAUDE.md",
        ".elai/ELAI.md",
        ".elai/instructions.md",
    ];
    candidates
        .iter()
        .map(|name| cwd.join(name))
        .find(|p| p.exists())
}

/// Strip the existing compressed summary block from `content` and return both.
///
/// Returns `(content_without_summary, extracted_summary)`.
fn strip_summary(content: &str) -> (String, Option<String>) {
    if let Some(open_pos) = content.find(SUMMARY_OPEN) {
        if let Some(close_pos) = content.find(SUMMARY_CLOSE) {
            let summary_start = open_pos + SUMMARY_OPEN.len();
            let summary = content[summary_start..close_pos].trim().to_string();
            let block_end = close_pos + SUMMARY_CLOSE.len();
            // Remove the block (including surrounding blank lines) from content.
            let before = content[..open_pos].trim_end().to_string();
            let after = content[block_end..].trim_start().to_string();
            let stripped = if before.is_empty() {
                after
            } else if after.is_empty() {
                before
            } else {
                format!("{before}\n\n{after}")
            };
            return (stripped, Some(summary));
        }
    }
    (content.to_string(), None)
}

/// Split markdown content into logical entries.
///
/// An entry boundary is either a `## ` heading or a `---` horizontal rule on
/// its own line.  If neither is found the whole content is treated as one entry.
fn split_entries(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut entries: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        let is_separator = trimmed == "---" || trimmed == "***" || trimmed == "___";
        let is_heading = trimmed.starts_with("## ");

        if (is_separator || is_heading) && !current.is_empty() {
            let entry = current.join("\n").trim().to_string();
            if !entry.is_empty() {
                entries.push(entry);
            }
            current.clear();
            if is_heading {
                current.push(line);
            }
            // If it was a separator, drop it (don't include in next entry).
        } else {
            current.push(line);
        }
    }

    // Push the last accumulated entry.
    let last = current.join("\n").trim().to_string();
    if !last.is_empty() {
        entries.push(last);
    }

    entries
}

/// Parse a memory file content into old / recent / summary sections.
pub fn parse_memory_sections(content: &str) -> MemoryParseResult {
    let (stripped, existing_summary) = strip_summary(content);
    let entries = split_entries(&stripped);
    let total = entries.len();

    if total <= RECENT_KEEP {
        MemoryParseResult {
            old_entries: vec![],
            recent_entries: entries,
            existing_summary,
        }
    } else {
        let split_at = total - RECENT_KEEP;
        let old_entries = entries[..split_at].to_vec();
        let recent_entries = entries[split_at..].to_vec();
        MemoryParseResult {
            old_entries,
            recent_entries,
            existing_summary,
        }
    }
}

/// Build the compression prompt sent to the model.
pub fn build_compression_prompt(entries: &[String], existing_summary: Option<&str>) -> String {
    let entries_text = entries.join("\n\n---\n\n");

    let existing_ctx = existing_summary.map_or_else(String::new, |s| {
        format!("\n\nPreviously compressed summary (merge and update as needed):\n{s}\n")
    });

    format!(
        "You are a memory compression engine. Output only the summary, nothing else.\n\
         Your task is to compress the following memory entries into a concise summary.\n\
         Preserve:\n\
         - Architectural decisions and rationale\n\
         - Modified files and their purpose\n\
         - Errors encountered and their fixes\n\
         - Important context about the project trajectory\n\
         - Key facts the assistant should remember\n\
         Write the summary as bullet points or short paragraphs. Be concise but complete.{existing_ctx}\n\
         \n\
         Memory entries to compress:\n\
         \n\
         {entries_text}"
    )
}

/// Rewrite the memory file with a compressed summary header followed by recent entries.
///
/// The original file is backed up as `<path>.bak` before writing.
pub fn rewrite_memory(path: &Path, summary: &str, recent: &[String]) -> io::Result<()> {
    // Backup the original file.
    let backup_path = path.with_extension("md.bak");
    fs::copy(path, &backup_path)?;

    // Build new content.
    let recent_text = recent.join("\n\n---\n\n");
    let new_content = if recent_text.is_empty() {
        format!("{SUMMARY_OPEN}\n{summary}\n{SUMMARY_CLOSE}\n")
    } else {
        format!("{SUMMARY_OPEN}\n{summary}\n{SUMMARY_CLOSE}\n\n{recent_text}\n")
    };

    fs::write(path, new_content)
}

/// Format the output shown to the user after dream completes.
#[allow(clippy::cast_precision_loss)]
pub fn format_dream_output(result: &DreamResult) -> String {
    let summary_preview = if result.summary.len() > 200 {
        format!("{}…", &result.summary[..200])
    } else {
        result.summary.clone()
    };
    format!(
        "Dream\n  Result           done\n  Compressed       {} entries\n  Before           {} chars\n  After            {} chars\n  Ratio            {:.0}%\n  Summary          {}",
        result.entries_compressed,
        result.before_size,
        result.after_size,
        if result.before_size > 0 {
            (result.after_size as f64 / result.before_size as f64) * 100.0
        } else {
            100.0
        },
        summary_preview,
    )
}

/// Execute the dream compression workflow.
///
/// Accepts a `model_call` closure (so the caller supplies the LLM backend) and a
/// `reporter` for TUI-safe progress messages.  Returns `None` when the run is
/// skipped, or `Some(DreamResult)` on success.
#[allow(clippy::type_complexity)]
pub fn execute_dream(
    cwd: &Path,
    force: bool,
    model_call: &dyn Fn(&str) -> Result<String, Box<dyn std::error::Error>>,
    reporter: &dyn ProgressReporter,
) -> Result<Option<DreamResult>, Box<dyn std::error::Error>> {
    let Some(path) = find_memory_file(cwd) else {
        reporter.report(
            "Dream\n  Result           skipped\n  Reason           no memory file (CLAUDE.md/AGENTS.md/ELAI.md)",
        );
        return Ok(None);
    };
    let content = fs::read_to_string(&path)?;
    let before_size = content.len();
    let parsed = parse_memory_sections(&content);
    let entries_to_compress: Vec<String> = if parsed.old_entries.is_empty() {
        if !force {
            reporter.report(&format!(
                "Dream\n  Result           skipped\n  Reason           <= 20 entries (currently {})",
                parsed.recent_entries.len()
            ));
            return Ok(None);
        }
        if parsed.recent_entries.len() > 20 {
            parsed.recent_entries[..parsed.recent_entries.len() - 20].to_vec()
        } else {
            parsed.recent_entries.clone()
        }
    } else {
        parsed.old_entries.clone()
    };
    reporter.report(&format!(
        "Dream\n  Compressing {} entries from {} ...",
        entries_to_compress.len(),
        path.display()
    ));
    let prompt = build_compression_prompt(&entries_to_compress, parsed.existing_summary.as_deref());
    let summary = model_call(&prompt)?;
    rewrite_memory(&path, &summary, &parsed.recent_entries)?;
    let after_content = fs::read_to_string(&path)?;
    Ok(Some(DreamResult {
        entries_compressed: entries_to_compress.len(),
        before_size,
        after_size: after_content.len(),
        summary,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_heading_content(n: usize) -> String {
        (1..=n)
            .map(|i| format!("## Section {i}\n\nContent of section {i}."))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn make_hr_content(n: usize) -> String {
        (1..=n)
            .map(|i| format!("Entry {i}\n\nContent of entry {i}."))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }

    fn unique_test_dir() -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // Use the thread id for additional uniqueness in parallel tests.
        let dir = std::env::temp_dir().join(format!("elai-dream-test-{ts}"));
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn test_parse_sections_with_headings() {
        let content = make_heading_content(25);
        let result = parse_memory_sections(&content);
        assert_eq!(result.old_entries.len(), 5, "should have 5 old entries");
        assert_eq!(
            result.recent_entries.len(),
            20,
            "should have 20 recent entries"
        );
        assert!(result.existing_summary.is_none());
    }

    #[test]
    fn test_parse_sections_with_hr_separators() {
        let content = make_hr_content(25);
        let result = parse_memory_sections(&content);
        assert_eq!(result.old_entries.len(), 5, "should have 5 old entries");
        assert_eq!(
            result.recent_entries.len(),
            20,
            "should have 20 recent entries"
        );
        assert!(result.existing_summary.is_none());
    }

    #[test]
    fn test_parse_sections_few_entries() {
        let content = make_heading_content(10);
        let result = parse_memory_sections(&content);
        assert_eq!(result.old_entries.len(), 0, "should have 0 old entries");
        assert_eq!(
            result.recent_entries.len(),
            10,
            "should have 10 recent entries"
        );
    }

    #[test]
    fn test_parse_preserves_existing_summary() {
        let content = format!(
            "{SUMMARY_OPEN}\nPrevious summary text.\n{SUMMARY_CLOSE}\n\n{}",
            make_heading_content(5)
        );
        let result = parse_memory_sections(&content);
        assert!(result.existing_summary.is_some());
        assert_eq!(
            result.existing_summary.as_deref().unwrap(),
            "Previous summary text."
        );
        // The 5 heading entries should still be parsed from the remaining content.
        assert_eq!(result.recent_entries.len(), 5);
        assert_eq!(result.old_entries.len(), 0);
    }

    #[test]
    fn test_rewrite_creates_backup() {
        let dir = unique_test_dir();
        let path = dir.join("ELAI.md");
        let original = "## Entry 1\n\nSome content.\n";
        fs::write(&path, original).unwrap();

        let summary = "Compressed summary.";
        rewrite_memory(&path, summary, &["## Entry 1\n\nSome content.".to_string()]).unwrap();

        let backup_path = path.with_extension("md.bak");
        assert!(backup_path.exists(), "backup file should exist");
        let backup_content = fs::read_to_string(&backup_path).unwrap();
        assert_eq!(
            backup_content, original,
            "backup should be identical to original"
        );
    }

    #[test]
    fn test_rewrite_output_format() {
        let dir = unique_test_dir();
        let path = dir.join("ELAI.md");
        let original = "## Entry 1\n\nSome content.\n";
        fs::write(&path, original).unwrap();

        let summary = "This is the compressed summary.";
        let recent = vec![
            "## Recent 1\n\nContent 1.".to_string(),
            "## Recent 2\n\nContent 2.".to_string(),
        ];
        rewrite_memory(&path, summary, &recent).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains(SUMMARY_OPEN),
            "should contain opening marker"
        );
        assert!(
            written.contains(SUMMARY_CLOSE),
            "should contain closing marker"
        );
        assert!(written.contains(summary), "should contain summary text");
        assert!(
            written.contains("## Recent 1"),
            "should contain recent entries"
        );
        assert!(
            written.contains("## Recent 2"),
            "should contain recent entries"
        );
    }

    #[test]
    fn test_build_compression_prompt() {
        let entries = vec![
            "## Entry 1\n\nDecision: use async.".to_string(),
            "## Entry 2\n\nFixed bug in parser.".to_string(),
        ];
        let prompt = build_compression_prompt(&entries, None);
        assert!(prompt.contains("memory compression engine"));
        assert!(prompt.contains("Architectural decisions"));
        assert!(prompt.contains("Entry 1"));
        assert!(prompt.contains("Entry 2"));
    }
}

//! Myers diff computation using the `similar` crate.

use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Keep,
    Add,
    Remove,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffTag,
    pub old_lineno: Option<usize>,
    #[allow(dead_code)]
    pub new_lineno: Option<usize>,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

#[allow(dead_code)]
pub fn is_unchanged(old: &str, new: &str) -> bool {
    old == new
}

/// Computes a unified diff with `context` lines around each change.
pub fn compute_diff(old: &str, new: &str, context: usize) -> Vec<DiffHunk> {
    if old == new {
        return Vec::new();
    }

    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(context) {
        let old_start = group
            .first()
            .map(|op| op.old_range().start + 1)
            .unwrap_or(1);
        let new_start = group
            .first()
            .map(|op| op.new_range().start + 1)
            .unwrap_or(1);

        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let tag = match change.tag() {
                    ChangeTag::Delete => DiffTag::Remove,
                    ChangeTag::Insert => DiffTag::Add,
                    ChangeTag::Equal => DiffTag::Keep,
                };
                let old_lineno = change.old_index().map(|i| i + 1);
                let new_lineno = change.new_index().map(|i| i + 1);
                let value = change.value().trim_end_matches('\n').to_string();
                lines.push(DiffLine { tag, old_lineno, new_lineno, value });
            }
        }

        if !lines.is_empty() {
            hunks.push(DiffHunk { old_start, new_start, lines });
        }
    }

    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unchanged_returns_empty() {
        assert!(compute_diff("a\nb\nc\n", "a\nb\nc\n", 3).is_empty());
    }

    #[test]
    fn test_is_unchanged() {
        assert!(is_unchanged("hello\n", "hello\n"));
        assert!(!is_unchanged("hello\n", "world\n"));
    }

    #[test]
    fn test_single_line_change() {
        let hunks = compute_diff("a\nb\nc\n", "a\nX\nc\n", 3);
        assert_eq!(hunks.len(), 1);
        let tags: Vec<DiffTag> = hunks[0].lines.iter().map(|l| l.tag).collect();
        assert!(tags.contains(&DiffTag::Remove));
        assert!(tags.contains(&DiffTag::Add));
        assert!(tags.contains(&DiffTag::Keep));
    }

    #[test]
    fn test_new_file_all_adds() {
        let hunks = compute_diff("", "line1\nline2\n", 3);
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].lines.iter().all(|l| l.tag == DiffTag::Add));
    }

    #[test]
    fn test_file_deletion_all_removes() {
        let hunks = compute_diff("line1\nline2\n", "", 3);
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].lines.iter().all(|l| l.tag == DiffTag::Remove));
    }

    #[test]
    fn test_line_numbers() {
        let hunks = compute_diff("a\nb\nc\n", "a\nX\nc\n", 3);
        let hunk = &hunks[0];
        let remove = hunk.lines.iter().find(|l| l.tag == DiffTag::Remove).unwrap();
        let add = hunk.lines.iter().find(|l| l.tag == DiffTag::Add).unwrap();
        assert_eq!(remove.old_lineno, Some(2));
        assert_eq!(add.new_lineno, Some(2));
    }
}

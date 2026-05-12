//! Diff types and utilities for comparing files and data.
//!
//! This module provides common types and functions for generating unified diffs
//! between text content, used by both P4K and DataCore comparison features.

use similar::{ChangeTag, TextDiff as SimilarDiff};

/// Result of comparing two items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// Item exists only in the new version
    Added,
    /// Item exists only in the old version
    Removed,
    /// Item exists in both but has changed
    Modified,
    /// Item is identical in both versions
    Unchanged,
}

/// Kind of line in a unified diff
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Unchanged context line (prefix: " ")
    Context,
    /// Added line (prefix: "+")
    Added,
    /// Removed line (prefix: "-")
    Removed,
    /// Header line (e.g., "--- old", "+++ new", "@@ ... @@")
    Header,
}

/// A single line in a unified diff
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// The kind of this line
    pub kind: DiffLineKind,
    /// The content of the line (including prefix for context/added/removed)
    pub content: String,
}

impl DiffLine {
    /// Create a new diff line
    pub fn new(kind: DiffLineKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
        }
    }

    /// Get the prefix character for this line kind
    pub fn prefix(&self) -> &'static str {
        match self.kind {
            DiffLineKind::Context => " ",
            DiffLineKind::Added => "+",
            DiffLineKind::Removed => "-",
            DiffLineKind::Header => "",
        }
    }
}

/// A complete unified diff between two text contents
#[derive(Debug, Clone, Default)]
pub struct TextDiff {
    /// All lines in the diff
    pub lines: Vec<DiffLine>,
    /// Number of added lines
    pub additions: usize,
    /// Number of removed lines
    pub deletions: usize,
}

impl TextDiff {
    /// Create an empty diff
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the diff is empty (no changes)
    pub fn is_empty(&self) -> bool {
        self.additions == 0 && self.deletions == 0
    }

    /// Get total number of changes
    pub fn change_count(&self) -> usize {
        self.additions + self.deletions
    }
}

/// Generate a unified diff from two text contents.
///
/// # Arguments
///
/// * `old_content` - The original text content
/// * `new_content` - The new text content
/// * `old_label` - Label for the old file (e.g., "a/file.txt")
/// * `new_label` - Label for the new file (e.g., "b/file.txt")
/// * `context_lines` - Number of context lines around changes (typically 3)
///
/// # Returns
///
/// A `TextDiff` containing all the diff lines with proper prefixes and coloring hints.
pub fn generate_unified_diff(
    old_content: &str,
    new_content: &str,
    old_label: &str,
    new_label: &str,
    context_lines: usize,
) -> TextDiff {
    // Handle identical content
    if old_content == new_content {
        return TextDiff::new();
    }

    let diff = SimilarDiff::from_lines(old_content, new_content);
    let mut lines = Vec::new();
    let mut additions = 0;
    let mut deletions = 0;

    // File headers
    lines.push(DiffLine::new(
        DiffLineKind::Header,
        format!("--- {}", old_label),
    ));
    lines.push(DiffLine::new(
        DiffLineKind::Header,
        format!("+++ {}", new_label),
    ));

    // Process hunks
    for hunk in diff
        .unified_diff()
        .context_radius(context_lines)
        .iter_hunks()
    {
        // Hunk header
        lines.push(DiffLine::new(
            DiffLineKind::Header,
            hunk.header().to_string().trim_end().to_string(),
        ));

        // Hunk content
        for change in hunk.iter_changes() {
            let value = change.value().trim_end_matches('\n');
            let (kind, prefix) = match change.tag() {
                ChangeTag::Equal => (DiffLineKind::Context, " "),
                ChangeTag::Insert => {
                    additions += 1;
                    (DiffLineKind::Added, "+")
                }
                ChangeTag::Delete => {
                    deletions += 1;
                    (DiffLineKind::Removed, "-")
                }
            };

            lines.push(DiffLine::new(kind, format!("{}{}", prefix, value)));
        }
    }

    TextDiff {
        lines,
        additions,
        deletions,
    }
}

/// Generate a diff showing only the new content (for added items).
///
/// All lines will be marked as added.
pub fn generate_added_diff(content: &str, label: &str) -> TextDiff {
    let mut lines = Vec::new();
    let mut additions = 0;

    lines.push(DiffLine::new(
        DiffLineKind::Header,
        format!("+++ {}", label),
    ));

    for line in content.lines() {
        lines.push(DiffLine::new(DiffLineKind::Added, format!("+{}", line)));
        additions += 1;
    }

    TextDiff {
        lines,
        additions,
        deletions: 0,
    }
}

/// Generate a diff showing only the old content (for removed items).
///
/// All lines will be marked as removed.
pub fn generate_removed_diff(content: &str, label: &str) -> TextDiff {
    let mut lines = Vec::new();
    let mut deletions = 0;

    lines.push(DiffLine::new(
        DiffLineKind::Header,
        format!("--- {}", label),
    ));

    for line in content.lines() {
        lines.push(DiffLine::new(DiffLineKind::Removed, format!("-{}", line)));
        deletions += 1;
    }

    TextDiff {
        lines,
        additions: 0,
        deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_content() {
        let diff = generate_unified_diff("hello\nworld\n", "hello\nworld\n", "a", "b", 3);
        assert!(diff.is_empty());
        assert_eq!(diff.additions, 0);
        assert_eq!(diff.deletions, 0);
    }

    #[test]
    fn test_simple_addition() {
        let diff = generate_unified_diff("line1\nline2\n", "line1\nline2\nline3\n", "a", "b", 3);
        assert!(!diff.is_empty());
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 0);
    }

    #[test]
    fn test_simple_deletion() {
        let diff = generate_unified_diff("line1\nline2\nline3\n", "line1\nline2\n", "a", "b", 3);
        assert!(!diff.is_empty());
        assert_eq!(diff.additions, 0);
        assert_eq!(diff.deletions, 1);
    }

    #[test]
    fn test_modification() {
        let diff = generate_unified_diff("line1\nold\nline3\n", "line1\nnew\nline3\n", "a", "b", 3);
        assert!(!diff.is_empty());
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 1);
    }

    #[test]
    fn test_added_diff() {
        let diff = generate_added_diff("line1\nline2\n", "new_file");
        assert_eq!(diff.additions, 2);
        assert_eq!(diff.deletions, 0);
        assert!(diff.lines.iter().any(|l| l.kind == DiffLineKind::Added));
    }

    #[test]
    fn test_removed_diff() {
        let diff = generate_removed_diff("line1\nline2\n", "old_file");
        assert_eq!(diff.additions, 0);
        assert_eq!(diff.deletions, 2);
        assert!(diff.lines.iter().any(|l| l.kind == DiffLineKind::Removed));
    }

    #[test]
    fn test_diff_status() {
        assert_eq!(DiffStatus::Added, DiffStatus::Added);
        assert_ne!(DiffStatus::Added, DiffStatus::Removed);
    }

    #[test]
    fn test_diff_line_prefix() {
        assert_eq!(DiffLine::new(DiffLineKind::Context, "").prefix(), " ");
        assert_eq!(DiffLine::new(DiffLineKind::Added, "").prefix(), "+");
        assert_eq!(DiffLine::new(DiffLineKind::Removed, "").prefix(), "-");
        assert_eq!(DiffLine::new(DiffLineKind::Header, "").prefix(), "");
    }
}

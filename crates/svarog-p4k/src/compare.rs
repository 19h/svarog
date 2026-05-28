//! P4K archive comparison utilities.
//!
//! This module provides functionality to compare two P4K archives and identify
//! added, removed, and modified files between them.

use std::collections::{HashMap, HashSet};

use svarog_common::DiffStatus;

use crate::P4kArchive;

/// Result of comparing a single file between two archives.
#[derive(Debug, Clone)]
pub struct FileComparisonResult {
    /// File path within the archive
    pub path: String,
    /// Status of this file (Added, Removed, Modified)
    pub status: DiffStatus,
    /// Size in the old archive (None if added)
    pub size_old: Option<u64>,
    /// Size in the new archive (None if removed)
    pub size_new: Option<u64>,
    /// CIG CRC32C in the old archive (None if added)
    pub crc_old: Option<u32>,
    /// CIG CRC32C in the new archive (None if removed)
    pub crc_new: Option<u32>,
}

/// Result of comparing two P4K archives.
#[derive(Debug, Clone)]
pub struct P4kComparisonResult {
    /// Files that exist only in the new archive
    pub added: Vec<FileComparisonResult>,
    /// Files that exist only in the old archive
    pub removed: Vec<FileComparisonResult>,
    /// Files that exist in both but have different content
    pub modified: Vec<FileComparisonResult>,
    /// Total number of files in the old archive
    pub old_count: usize,
    /// Total number of files in the new archive
    pub new_count: usize,
}

impl P4kComparisonResult {
    /// Get total number of changes
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.removed.len() + self.modified.len()
    }

    /// Check if archives are identical
    pub fn is_identical(&self) -> bool {
        self.total_changes() == 0
    }
}

/// Compare two P4K archives and return the differences.
///
/// Files are matched by path (case-insensitive). Modified files are detected
/// by comparing CIG CRC32C checksums.
///
/// # Arguments
///
/// * `old_archive` - The original/base archive
/// * `new_archive` - The new/updated archive
///
/// # Returns
///
/// A `P4kComparisonResult` containing lists of added, removed, and modified files.
pub fn compare_archives(old_archive: &P4kArchive, new_archive: &P4kArchive) -> P4kComparisonResult {
    // Build maps of normalized path -> entry info for both archives
    let old_entries: HashMap<String, EntryInfo> = old_archive
        .iter()
        .map(|e| {
            let key = normalize_path(e.name);
            let info = EntryInfo {
                original_name: e.name.to_string(),
                size: e.uncompressed_size,
                crc32: e.crc32,
            };
            (key, info)
        })
        .collect();

    let new_entries: HashMap<String, EntryInfo> = new_archive
        .iter()
        .map(|e| {
            let key = normalize_path(e.name);
            let info = EntryInfo {
                original_name: e.name.to_string(),
                size: e.uncompressed_size,
                crc32: e.crc32,
            };
            (key, info)
        })
        .collect();

    let old_keys: HashSet<_> = old_entries.keys().cloned().collect();
    let new_keys: HashSet<_> = new_entries.keys().cloned().collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    // Files only in new archive (added)
    for key in new_keys.difference(&old_keys) {
        let info = &new_entries[key];
        added.push(FileComparisonResult {
            path: info.original_name.clone(),
            status: DiffStatus::Added,
            size_old: None,
            size_new: Some(info.size),
            crc_old: None,
            crc_new: Some(info.crc32),
        });
    }

    // Files only in old archive (removed)
    for key in old_keys.difference(&new_keys) {
        let info = &old_entries[key];
        removed.push(FileComparisonResult {
            path: info.original_name.clone(),
            status: DiffStatus::Removed,
            size_old: Some(info.size),
            size_new: None,
            crc_old: Some(info.crc32),
            crc_new: None,
        });
    }

    // Files in both - check if modified (by CIG CRC32C)
    for key in old_keys.intersection(&new_keys) {
        let old_info = &old_entries[key];
        let new_info = &new_entries[key];

        if old_info.crc32 != new_info.crc32 {
            modified.push(FileComparisonResult {
                path: new_info.original_name.clone(),
                status: DiffStatus::Modified,
                size_old: Some(old_info.size),
                size_new: Some(new_info.size),
                crc_old: Some(old_info.crc32),
                crc_new: Some(new_info.crc32),
            });
        }
    }

    // Sort results by path for consistent output
    added.sort_by_key(|a| a.path.to_lowercase());
    removed.sort_by_key(|a| a.path.to_lowercase());
    modified.sort_by_key(|a| a.path.to_lowercase());

    P4kComparisonResult {
        added,
        removed,
        modified,
        old_count: old_archive.entry_count(),
        new_count: new_archive.entry_count(),
    }
}

/// Check if a file path corresponds to a text file that can be diffed.
///
/// Returns true for XML, JSON, Lua, config files, etc.
pub fn is_text_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    let text_extensions = [
        // XML and related
        ".xml",
        ".mtl",
        ".cdf",
        ".chrparams",
        ".adb",
        ".animevents",
        ".bspace",
        ".comb",
        ".eco",
        ".entxml",
        ".ent",
        // Config and data
        ".txt",
        ".cfg",
        ".json",
        ".ini",
        ".csv",
        ".log",
        // Scripts
        ".lua",
        ".js",
        // Documentation
        ".md",
        ".html",
        ".css",
    ];
    text_extensions.iter().any(|ext| lower.ends_with(ext))
}

/// Check if a file is a CryXML binary file that should be decoded before diffing.
///
/// This checks common extensions that Star Citizen uses for CryXML binary format.
pub fn is_cryxml_extension(name: &str) -> bool {
    let lower = name.to_lowercase();
    let cryxml_extensions = [
        ".mtl",
        ".cdf",
        ".chrparams",
        ".adb",
        ".animevents",
        ".bspace",
        ".comb",
        ".eco",
    ];
    cryxml_extensions.iter().any(|ext| lower.ends_with(ext))
}

/// Internal struct for tracking entry info during comparison
struct EntryInfo {
    original_name: String,
    size: u64,
    crc32: u32,
}

/// Normalize a path for case-insensitive comparison.
///
/// Converts to lowercase and normalizes path separators.
fn normalize_path(path: &str) -> String {
    path.to_lowercase().replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_text_file() {
        assert!(is_text_file("file.xml"));
        assert!(is_text_file("FILE.XML"));
        assert!(is_text_file("path/to/file.json"));
        assert!(is_text_file("material.mtl"));
        assert!(is_text_file("script.lua"));

        assert!(!is_text_file("texture.dds"));
        assert!(!is_text_file("model.cgf"));
        assert!(!is_text_file("audio.wem"));
    }

    #[test]
    fn test_is_cryxml_extension() {
        assert!(is_cryxml_extension("material.mtl"));
        assert!(is_cryxml_extension("character.cdf"));
        assert!(is_cryxml_extension("FILE.MTL"));

        assert!(!is_cryxml_extension("file.xml"));
        assert!(!is_cryxml_extension("file.txt"));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(
            normalize_path("Data/Objects/test.xml"),
            "data\\objects\\test.xml"
        );
        assert_eq!(
            normalize_path("DATA\\OBJECTS\\TEST.XML"),
            "data\\objects\\test.xml"
        );
    }

    #[test]
    fn test_comparison_result_methods() {
        let result = P4kComparisonResult {
            added: vec![FileComparisonResult {
                path: "new.xml".to_string(),
                status: DiffStatus::Added,
                size_old: None,
                size_new: Some(100),
                crc_old: None,
                crc_new: Some(0x12345678),
            }],
            removed: vec![],
            modified: vec![],
            old_count: 10,
            new_count: 11,
        };

        assert_eq!(result.total_changes(), 1);
        assert!(!result.is_identical());

        let empty_result = P4kComparisonResult {
            added: vec![],
            removed: vec![],
            modified: vec![],
            old_count: 10,
            new_count: 10,
        };

        assert_eq!(empty_result.total_changes(), 0);
        assert!(empty_result.is_identical());
    }
}

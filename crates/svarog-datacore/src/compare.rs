//! DataCore database comparison utilities.
//!
//! This module provides functionality to compare two DCB databases and identify
//! added, removed, and modified records, structs, and enums between them.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use svarog_common::DiffStatus;

use crate::{CHeaderExporter, DataCoreDatabase, XmlExporter};

/// Strip C-style block comments from content.
/// This removes metadata comments like /* enum_index: ... */ that contain
/// position-dependent information which would create noise in comparisons.
fn strip_comments(content: &str) -> String {
    // Match /* ... */ comments (non-greedy)
    let re = Regex::new(r"/\*[\s\S]*?\*/").unwrap();
    re.replace_all(content, "").to_string()
}

/// Scope of comparison for DCB databases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DcbCompareScope {
    /// Compare only records
    Records,
    /// Compare only struct definitions
    Structs,
    /// Compare only enum definitions
    Enums,
    /// Compare all (records, structs, and enums)
    #[default]
    All,
}

impl DcbCompareScope {
    /// Parse scope from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "records" => Self::Records,
            "structs" => Self::Structs,
            "enums" => Self::Enums,
            _ => Self::All,
        }
    }
}

/// Type of item being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcbItemType {
    /// A record (named data instance)
    Record,
    /// A struct definition
    Struct,
    /// An enum definition
    Enum,
}

impl std::fmt::Display for DcbItemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Record => write!(f, "Record"),
            Self::Struct => write!(f, "Struct"),
            Self::Enum => write!(f, "Enum"),
        }
    }
}

/// Result of comparing a single item between two databases.
#[derive(Debug, Clone)]
pub struct DcbComparisonItem {
    /// Name of the item
    pub name: String,
    /// Type of the item
    pub item_type: DcbItemType,
    /// Status of this item (Added, Removed, Modified)
    pub status: DiffStatus,
    /// Index in the old database (None if added)
    pub old_index: Option<usize>,
    /// Index in the new database (None if removed)
    pub new_index: Option<usize>,
    /// GUID for records (empty for structs/enums)
    pub guid: Option<String>,
}

/// Result of comparing two DCB databases.
#[derive(Debug, Clone, Default)]
pub struct DcbComparisonResult {
    /// Items that exist only in the new database
    pub added: Vec<DcbComparisonItem>,
    /// Items that exist only in the old database
    pub removed: Vec<DcbComparisonItem>,
    /// Items that exist in both but have different content
    pub modified: Vec<DcbComparisonItem>,
    /// Counts from the old database (records, structs, enums)
    pub old_counts: (usize, usize, usize),
    /// Counts from the new database (records, structs, enums)
    pub new_counts: (usize, usize, usize),
}

impl DcbComparisonResult {
    /// Get total number of changes
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.removed.len() + self.modified.len()
    }

    /// Check if databases are identical (within the compared scope)
    pub fn is_identical(&self) -> bool {
        self.total_changes() == 0
    }

    /// Get items of a specific type
    pub fn items_of_type(&self, item_type: DcbItemType) -> impl Iterator<Item = &DcbComparisonItem> {
        self.added
            .iter()
            .chain(self.removed.iter())
            .chain(self.modified.iter())
            .filter(move |item| item.item_type == item_type)
    }

    /// Count changes by type
    pub fn count_by_type(&self, item_type: DcbItemType) -> (usize, usize, usize) {
        let added = self.added.iter().filter(|i| i.item_type == item_type).count();
        let removed = self.removed.iter().filter(|i| i.item_type == item_type).count();
        let modified = self.modified.iter().filter(|i| i.item_type == item_type).count();
        (added, removed, modified)
    }
}

/// Progress callback for comparison operations.
/// Arguments are (phase_name, current_item, total_items).
pub type ProgressCallback<'a> = &'a mut dyn FnMut(&str, usize, usize);

/// Compare two DCB databases and return the differences.
///
/// # Arguments
///
/// * `old_db` - The original/base database
/// * `new_db` - The new/updated database
/// * `scope` - What to compare (records, structs, enums, or all)
///
/// # Returns
///
/// A `DcbComparisonResult` containing lists of added, removed, and modified items.
pub fn compare_databases(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    scope: DcbCompareScope,
) -> DcbComparisonResult {
    compare_databases_with_progress(old_db, new_db, scope, &mut |_, _, _| {})
}

/// Compare two DCB databases and return the differences, with progress reporting.
///
/// # Arguments
///
/// * `old_db` - The original/base database
/// * `new_db` - The new/updated database
/// * `scope` - What to compare (records, structs, enums, or all)
/// * `progress` - Callback for progress updates (phase, current, total)
///
/// # Returns
///
/// A `DcbComparisonResult` containing lists of added, removed, and modified items.
pub fn compare_databases_with_progress(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    scope: DcbCompareScope,
    progress: ProgressCallback,
) -> DcbComparisonResult {
    let mut result = DcbComparisonResult {
        old_counts: (
            old_db.records().len(),
            old_db.struct_definitions().len(),
            old_db.enum_definitions().len(),
        ),
        new_counts: (
            new_db.records().len(),
            new_db.struct_definitions().len(),
            new_db.enum_definitions().len(),
        ),
        ..Default::default()
    };

    if matches!(scope, DcbCompareScope::Records | DcbCompareScope::All) {
        compare_records_with_progress(old_db, new_db, &mut result, progress);
    }

    if matches!(scope, DcbCompareScope::Structs | DcbCompareScope::All) {
        compare_structs_with_progress(old_db, new_db, &mut result, progress);
    }

    if matches!(scope, DcbCompareScope::Enums | DcbCompareScope::All) {
        compare_enums_with_progress(old_db, new_db, &mut result, progress);
    }

    // Sort all results by name for consistent output
    result.added.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    result.removed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    result.modified.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    result
}

/// Compare records between two databases.
fn compare_records(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
) {
    // Build map of GUID -> (index, record) for main records
    // Main records are the "root" records, not sub-records referenced by pointers
    let old_records: HashMap<String, (usize, &_)> = old_db
        .main_records()
        .enumerate()
        .map(|(i, r)| (r.id.to_string(), (i, r)))
        .collect();

    let new_records: HashMap<String, (usize, &_)> = new_db
        .main_records()
        .enumerate()
        .map(|(i, r)| (r.id.to_string(), (i, r)))
        .collect();

    let old_guids: HashSet<_> = old_records.keys().cloned().collect();
    let new_guids: HashSet<_> = new_records.keys().cloned().collect();

    // Added records (GUID only in new)
    for guid in new_guids.difference(&old_guids) {
        let (idx, record) = new_records[guid];
        let name = new_db.record_name(record).unwrap_or("Unknown").to_string();
        result.added.push(DcbComparisonItem {
            name,
            item_type: DcbItemType::Record,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(idx),
            guid: Some(guid.clone()),
        });
    }

    // Removed records (GUID only in old)
    for guid in old_guids.difference(&new_guids) {
        let (idx, record) = old_records[guid];
        let name = old_db.record_name(record).unwrap_or("Unknown").to_string();
        result.removed.push(DcbComparisonItem {
            name,
            item_type: DcbItemType::Record,
            status: DiffStatus::Removed,
            old_index: Some(idx),
            new_index: None,
            guid: Some(guid.clone()),
        });
    }

    // Modified records - compare XML export
    let old_exporter = XmlExporter::new(old_db);
    let new_exporter = XmlExporter::new(new_db);

    for guid in old_guids.intersection(&new_guids) {
        let (old_idx, old_record) = old_records[guid];
        let (new_idx, new_record) = new_records[guid];

        // Compare by XML export
        let old_xml = old_exporter.export_record(old_record).unwrap_or_default();
        let new_xml = new_exporter.export_record(new_record).unwrap_or_default();

        if old_xml != new_xml {
            let name = new_db.record_name(new_record).unwrap_or("Unknown").to_string();
            result.modified.push(DcbComparisonItem {
                name,
                item_type: DcbItemType::Record,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: Some(guid.clone()),
            });
        }
    }
}

/// Compare struct definitions between two databases.
fn compare_structs(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
) {
    // Build map of name -> index for structs
    let old_structs: HashMap<String, usize> = old_db
        .struct_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| old_db.struct_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let new_structs: HashMap<String, usize> = new_db
        .struct_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| new_db.struct_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let old_names: HashSet<_> = old_structs.keys().cloned().collect();
    let new_names: HashSet<_> = new_structs.keys().cloned().collect();

    // Added structs
    for name in new_names.difference(&old_names) {
        result.added.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Struct,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(new_structs[name]),
            guid: None,
        });
    }

    // Removed structs
    for name in old_names.difference(&new_names) {
        result.removed.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Struct,
            status: DiffStatus::Removed,
            old_index: Some(old_structs[name]),
            new_index: None,
            guid: None,
        });
    }

    // Modified structs - compare C header export (ignoring comments)
    let old_exporter = CHeaderExporter::new(old_db);
    let new_exporter = CHeaderExporter::new(new_db);

    for name in old_names.intersection(&new_names) {
        let old_idx = old_structs[name];
        let new_idx = new_structs[name];

        let old_header = strip_comments(&old_exporter.generate_struct_preview(old_idx));
        let new_header = strip_comments(&new_exporter.generate_struct_preview(new_idx));

        if old_header != new_header {
            result.modified.push(DcbComparisonItem {
                name: name.clone(),
                item_type: DcbItemType::Struct,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: None,
            });
        }
    }
}

/// Compare enum definitions between two databases.
fn compare_enums(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
) {
    // Build map of name -> index for enums
    let old_enums: HashMap<String, usize> = old_db
        .enum_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| old_db.enum_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let new_enums: HashMap<String, usize> = new_db
        .enum_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| new_db.enum_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let old_names: HashSet<_> = old_enums.keys().cloned().collect();
    let new_names: HashSet<_> = new_enums.keys().cloned().collect();

    // Added enums
    for name in new_names.difference(&old_names) {
        result.added.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Enum,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(new_enums[name]),
            guid: None,
        });
    }

    // Removed enums
    for name in old_names.difference(&new_names) {
        result.removed.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Enum,
            status: DiffStatus::Removed,
            old_index: Some(old_enums[name]),
            new_index: None,
            guid: None,
        });
    }

    // Modified enums - compare C header export (ignoring comments)
    let old_exporter = CHeaderExporter::new(old_db);
    let new_exporter = CHeaderExporter::new(new_db);

    for name in old_names.intersection(&new_names) {
        let old_idx = old_enums[name];
        let new_idx = new_enums[name];

        let old_header = strip_comments(&old_exporter.generate_enum_preview(old_idx));
        let new_header = strip_comments(&new_exporter.generate_enum_preview(new_idx));

        if old_header != new_header {
            result.modified.push(DcbComparisonItem {
                name: name.clone(),
                item_type: DcbItemType::Enum,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: None,
            });
        }
    }
}

/// Compare records between two databases with progress reporting.
fn compare_records_with_progress(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
    progress: ProgressCallback,
) {
    // Build map of GUID -> (index, record) for main records
    let old_records: HashMap<String, (usize, &_)> = old_db
        .main_records()
        .enumerate()
        .map(|(i, r)| (r.id.to_string(), (i, r)))
        .collect();

    let new_records: HashMap<String, (usize, &_)> = new_db
        .main_records()
        .enumerate()
        .map(|(i, r)| (r.id.to_string(), (i, r)))
        .collect();

    let old_guids: HashSet<_> = old_records.keys().cloned().collect();
    let new_guids: HashSet<_> = new_records.keys().cloned().collect();

    // Added records
    for guid in new_guids.difference(&old_guids) {
        let (idx, record) = new_records[guid];
        let name = new_db.record_name(record).unwrap_or("Unknown").to_string();
        result.added.push(DcbComparisonItem {
            name,
            item_type: DcbItemType::Record,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(idx),
            guid: Some(guid.clone()),
        });
    }

    // Removed records
    for guid in old_guids.difference(&new_guids) {
        let (idx, record) = old_records[guid];
        let name = old_db.record_name(record).unwrap_or("Unknown").to_string();
        result.removed.push(DcbComparisonItem {
            name,
            item_type: DcbItemType::Record,
            status: DiffStatus::Removed,
            old_index: Some(idx),
            new_index: None,
            guid: Some(guid.clone()),
        });
    }

    // Modified records - compare XML export
    let old_exporter = XmlExporter::new(old_db);
    let new_exporter = XmlExporter::new(new_db);
    let common_guids: Vec<_> = old_guids.intersection(&new_guids).collect();
    let total = common_guids.len();

    for (i, guid) in common_guids.iter().enumerate() {
        if i % 100 == 0 {
            progress("Records", i, total);
        }

        let (old_idx, old_record) = old_records[*guid];
        let (new_idx, new_record) = new_records[*guid];

        let old_xml = old_exporter.export_record(old_record).unwrap_or_default();
        let new_xml = new_exporter.export_record(new_record).unwrap_or_default();

        if old_xml != new_xml {
            let name = new_db.record_name(new_record).unwrap_or("Unknown").to_string();
            result.modified.push(DcbComparisonItem {
                name,
                item_type: DcbItemType::Record,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: Some((*guid).clone()),
            });
        }
    }
    progress("Records", total, total);
}

/// Compare struct definitions with progress reporting.
fn compare_structs_with_progress(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
    progress: ProgressCallback,
) {
    // Build map of name -> index for structs
    let old_structs: HashMap<String, usize> = old_db
        .struct_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| old_db.struct_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let new_structs: HashMap<String, usize> = new_db
        .struct_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| new_db.struct_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let old_names: HashSet<_> = old_structs.keys().cloned().collect();
    let new_names: HashSet<_> = new_structs.keys().cloned().collect();

    // Added structs
    for name in new_names.difference(&old_names) {
        result.added.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Struct,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(new_structs[name]),
            guid: None,
        });
    }

    // Removed structs
    for name in old_names.difference(&new_names) {
        result.removed.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Struct,
            status: DiffStatus::Removed,
            old_index: Some(old_structs[name]),
            new_index: None,
            guid: None,
        });
    }

    // Modified structs - compare C header export (ignoring comments)
    let old_exporter = CHeaderExporter::new(old_db);
    let new_exporter = CHeaderExporter::new(new_db);
    let common_names: Vec<_> = old_names.intersection(&new_names).collect();
    let total = common_names.len();

    for (i, name) in common_names.iter().enumerate() {
        if i % 100 == 0 {
            progress("Structs", i, total);
        }

        let old_idx = old_structs[*name];
        let new_idx = new_structs[*name];

        let old_header = strip_comments(&old_exporter.generate_struct_preview(old_idx));
        let new_header = strip_comments(&new_exporter.generate_struct_preview(new_idx));

        if old_header != new_header {
            result.modified.push(DcbComparisonItem {
                name: (*name).clone(),
                item_type: DcbItemType::Struct,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: None,
            });
        }
    }
    progress("Structs", total, total);
}

/// Compare enum definitions with progress reporting.
fn compare_enums_with_progress(
    old_db: &DataCoreDatabase,
    new_db: &DataCoreDatabase,
    result: &mut DcbComparisonResult,
    progress: ProgressCallback,
) {
    // Build map of name -> index for enums
    let old_enums: HashMap<String, usize> = old_db
        .enum_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| old_db.enum_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let new_enums: HashMap<String, usize> = new_db
        .enum_definitions()
        .iter()
        .enumerate()
        .filter_map(|(i, _)| new_db.enum_name(i).map(|n| (n.to_string(), i)))
        .collect();

    let old_names: HashSet<_> = old_enums.keys().cloned().collect();
    let new_names: HashSet<_> = new_enums.keys().cloned().collect();

    // Added enums
    for name in new_names.difference(&old_names) {
        result.added.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Enum,
            status: DiffStatus::Added,
            old_index: None,
            new_index: Some(new_enums[name]),
            guid: None,
        });
    }

    // Removed enums
    for name in old_names.difference(&new_names) {
        result.removed.push(DcbComparisonItem {
            name: name.clone(),
            item_type: DcbItemType::Enum,
            status: DiffStatus::Removed,
            old_index: Some(old_enums[name]),
            new_index: None,
            guid: None,
        });
    }

    // Modified enums - compare C header export (ignoring comments)
    let old_exporter = CHeaderExporter::new(old_db);
    let new_exporter = CHeaderExporter::new(new_db);
    let common_names: Vec<_> = old_names.intersection(&new_names).collect();
    let total = common_names.len();

    for (i, name) in common_names.iter().enumerate() {
        if i % 50 == 0 {
            progress("Enums", i, total);
        }

        let old_idx = old_enums[*name];
        let new_idx = new_enums[*name];

        let old_header = strip_comments(&old_exporter.generate_enum_preview(old_idx));
        let new_header = strip_comments(&new_exporter.generate_enum_preview(new_idx));

        if old_header != new_header {
            result.modified.push(DcbComparisonItem {
                name: (*name).clone(),
                item_type: DcbItemType::Enum,
                status: DiffStatus::Modified,
                old_index: Some(old_idx),
                new_index: Some(new_idx),
                guid: None,
            });
        }
    }
    progress("Enums", total, total);
}

/// Generate content for diffing a record from a database.
///
/// Returns the XML export of the record, suitable for text diffing.
pub fn get_record_content(db: &DataCoreDatabase, record_index: usize) -> Option<String> {
    let records: Vec<_> = db.main_records().collect();
    records.get(record_index).and_then(|record| {
        XmlExporter::new(db).export_record(record).ok()
    })
}

/// Generate content for diffing a struct from a database.
///
/// Returns the C header preview of the struct, suitable for text diffing.
pub fn get_struct_content(db: &DataCoreDatabase, struct_index: usize) -> Option<String> {
    if struct_index < db.struct_definitions().len() {
        Some(CHeaderExporter::new(db).generate_struct_preview(struct_index))
    } else {
        None
    }
}

/// Generate content for diffing an enum from a database.
///
/// Returns the C header preview of the enum, suitable for text diffing.
pub fn get_enum_content(db: &DataCoreDatabase, enum_index: usize) -> Option<String> {
    if enum_index < db.enum_definitions().len() {
        Some(CHeaderExporter::new(db).generate_enum_preview(enum_index))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_from_str() {
        assert_eq!(DcbCompareScope::from_str("records"), DcbCompareScope::Records);
        assert_eq!(DcbCompareScope::from_str("STRUCTS"), DcbCompareScope::Structs);
        assert_eq!(DcbCompareScope::from_str("enums"), DcbCompareScope::Enums);
        assert_eq!(DcbCompareScope::from_str("all"), DcbCompareScope::All);
        assert_eq!(DcbCompareScope::from_str("unknown"), DcbCompareScope::All);
    }

    #[test]
    fn test_item_type_display() {
        assert_eq!(format!("{}", DcbItemType::Record), "Record");
        assert_eq!(format!("{}", DcbItemType::Struct), "Struct");
        assert_eq!(format!("{}", DcbItemType::Enum), "Enum");
    }

    #[test]
    fn test_comparison_result_methods() {
        let result = DcbComparisonResult {
            added: vec![DcbComparisonItem {
                name: "NewStruct".to_string(),
                item_type: DcbItemType::Struct,
                status: DiffStatus::Added,
                old_index: None,
                new_index: Some(0),
                guid: None,
            }],
            removed: vec![],
            modified: vec![DcbComparisonItem {
                name: "ModRecord".to_string(),
                item_type: DcbItemType::Record,
                status: DiffStatus::Modified,
                old_index: Some(0),
                new_index: Some(0),
                guid: Some("xxx".to_string()),
            }],
            old_counts: (10, 5, 3),
            new_counts: (10, 6, 3),
        };

        assert_eq!(result.total_changes(), 2);
        assert!(!result.is_identical());
        assert_eq!(result.count_by_type(DcbItemType::Struct), (1, 0, 0));
        assert_eq!(result.count_by_type(DcbItemType::Record), (0, 0, 1));
    }
}

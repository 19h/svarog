//! Parallel XML export using rayon.
//!
//! This module provides high-performance parallel export of DataCore records
//! using rayon's work-stealing thread pool.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use rayon::prelude::*;

use super::xml::ExportError;
use super::XmlExporter;
use crate::structs::DataCoreRecord;
use crate::DataCoreDatabase;

/// High-performance parallel XML exporter.
///
/// Uses rayon for parallel processing and thread-local buffers
/// to minimize allocations and lock contention.
pub struct ParallelXmlExporter<'a> {
    database: &'a DataCoreDatabase,
}

impl<'a> ParallelXmlExporter<'a> {
    /// Create a new parallel exporter.
    pub fn new(database: &'a DataCoreDatabase) -> Self {
        Self { database }
    }

    /// Export all main records to a directory in parallel.
    ///
    /// Returns the number of successfully exported records.
    /// The progress callback receives (completed, total) counts.
    pub fn export_all<P: AsRef<Path>, F>(
        &self,
        output_dir: P,
        mut progress: F,
    ) -> Result<ExportStats, ExportError>
    where
        F: FnMut(usize, usize) + Send,
    {
        let output_dir = output_dir.as_ref();
        std::fs::create_dir_all(output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

        let main_records: Vec<_> = self.database.main_records().collect();
        let total = main_records.len();

        let exported = AtomicUsize::new(0);
        let errors = AtomicUsize::new(0);
        let progress = Mutex::new(&mut progress);

        // Parallel export with progress tracking
        main_records.par_iter().for_each(|record| {
            let result = self.export_single_record(record, output_dir);

            match result {
                Ok(()) => {
                    exported.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }

            // Update progress (with lock to avoid races)
            let done = exported.load(Ordering::Relaxed) + errors.load(Ordering::Relaxed);
            if done % 100 == 0 || done == total {
                if let Some(mut p) = progress.try_lock() {
                    (*p)(done, total);
                }
            }
        });

        let exported_count = exported.load(Ordering::Relaxed);
        let error_count = errors.load(Ordering::Relaxed);

        // Final progress update
        progress.lock()(total, total);

        Ok(ExportStats {
            exported: exported_count,
            errors: error_count,
            total,
        })
    }

    /// Export a batch of records by indices.
    pub fn export_batch<P: AsRef<Path>>(
        &self,
        indices: &[usize],
        output_dir: P,
    ) -> Result<ExportStats, ExportError> {
        let output_dir = output_dir.as_ref();
        std::fs::create_dir_all(output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

        let records = self.database.records();
        let total = indices.len();

        let exported = AtomicUsize::new(0);
        let errors = AtomicUsize::new(0);

        indices.par_iter().for_each(|&idx| {
            if let Some(record) = records.get(idx) {
                let result = self.export_single_record(record, output_dir);

                match result {
                    Ok(()) => {
                        exported.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        Ok(ExportStats {
            exported: exported.load(Ordering::Relaxed),
            errors: errors.load(Ordering::Relaxed),
            total,
        })
    }

    /// Export records in parallel, returning XML strings.
    ///
    /// This is useful when you want to process the XML in memory
    /// rather than writing to disk.
    pub fn export_to_strings(&self, records: &[&DataCoreRecord]) -> Vec<Result<String, ExportError>> {
        let exporter = XmlExporter::new(self.database);

        records
            .par_iter()
            .map(|record| exporter.export_record(record))
            .collect()
    }

    fn export_single_record(
        &self,
        record: &DataCoreRecord,
        output_dir: &Path,
    ) -> Result<(), ExportError> {
        let exporter = XmlExporter::new(self.database);

        let file_name = self
            .database
            .record_file_name(record)
            .unwrap_or("unknown.xml");

        // Convert path separators and add .xml extension
        let output_path = output_dir.join(file_name.replace('/', std::path::MAIN_SEPARATOR_STR));
        let output_path = output_path.with_extension("xml");

        // Create parent directories
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ExportError::Io(e.to_string()))?;
        }

        // Export record
        let xml = exporter.export_record(record)?;
        std::fs::write(&output_path, xml).map_err(|e| ExportError::Io(e.to_string()))?;

        Ok(())
    }
}

/// Statistics from a parallel export operation.
#[derive(Debug, Clone, Copy)]
pub struct ExportStats {
    /// Number of records successfully exported.
    pub exported: usize,
    /// Number of records that failed to export.
    pub errors: usize,
    /// Total number of records attempted.
    pub total: usize,
}

impl ExportStats {
    /// Check if all records were exported successfully.
    pub fn is_complete(&self) -> bool {
        self.errors == 0 && self.exported == self.total
    }
}

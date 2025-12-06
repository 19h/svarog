//! P4K archive entry.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::zip::CompressionMethod;

/// An entry (file) within a P4K archive.
///
/// This contains metadata about the file, not the file data itself.
/// Use [`P4kArchive::read_entry`] to get the actual file contents.
#[derive(Debug, Clone)]
pub struct P4kEntry {
    /// File name/path within the archive.
    name: String,
    /// Compressed size in bytes.
    compressed_size: u64,
    /// Uncompressed size in bytes.
    uncompressed_size: u64,
    /// Compression method used.
    compression_method: CompressionMethod,
    /// Whether the entry is encrypted.
    is_encrypted: bool,
    /// Offset to the local file header in the archive.
    local_header_offset: u64,
    /// DOS date/time of last modification.
    dos_datetime: u32,
    /// CRC32 checksum of uncompressed data.
    crc32: u32,
}

impl P4kEntry {
    /// Create a new P4K entry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        name: String,
        compressed_size: u64,
        uncompressed_size: u64,
        compression_method: CompressionMethod,
        is_encrypted: bool,
        local_header_offset: u64,
        dos_datetime: u32,
        crc32: u32,
    ) -> Self {
        Self {
            name,
            compressed_size,
            uncompressed_size,
            compression_method,
            is_encrypted,
            local_header_offset,
            dos_datetime,
            crc32,
        }
    }

    /// Get the file name/path.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the compressed size in bytes.
    #[inline]
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// Get the uncompressed size in bytes.
    #[inline]
    pub fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Get the compression method.
    #[inline]
    pub fn compression_method(&self) -> CompressionMethod {
        self.compression_method
    }

    /// Check if the entry is encrypted.
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        self.is_encrypted
    }

    /// Get the offset to the local file header.
    #[inline]
    pub(crate) fn local_header_offset(&self) -> u64 {
        self.local_header_offset
    }

    /// Get the CRC32 checksum.
    #[inline]
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Get the last modification time as a SystemTime.
    ///
    /// Returns None if the DOS datetime is invalid.
    pub fn last_modified(&self) -> Option<SystemTime> {
        dos_datetime_to_system_time(self.dos_datetime)
    }

    /// Get the relative output path for extraction.
    ///
    /// Converts Windows path separators to the platform's native separator.
    pub fn output_path(&self) -> PathBuf {
        // Replace backslashes with forward slashes for cross-platform compatibility
        let normalized = self.name.replace('\\', "/");
        PathBuf::from(normalized)
    }

    /// Check if this entry represents a directory.
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/') || self.name.ends_with('\\')
    }

    /// Get the file extension, if any.
    pub fn extension(&self) -> Option<&str> {
        Path::new(&self.name)
            .extension()
            .and_then(|ext| ext.to_str())
    }
}

/// Convert DOS date/time format to SystemTime.
///
/// DOS date/time format:
/// - Time: bits 0-4 = seconds/2, bits 5-10 = minutes, bits 11-15 = hours
/// - Date: bits 16-20 = day, bits 21-24 = month, bits 25-31 = year-1980
fn dos_datetime_to_system_time(datetime: u32) -> Option<SystemTime> {
    let year = 1980 + ((datetime >> 25) & 0x7F) as i32;
    let month = ((datetime >> 21) & 0x0F) as u32;
    let day = ((datetime >> 16) & 0x1F) as u32;
    let hour = ((datetime >> 11) & 0x1F) as u32;
    let minute = ((datetime >> 5) & 0x3F) as u32;
    let second = ((datetime & 0x1F) * 2) as u32;

    // Basic validation
    if month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    // Calculate days since Unix epoch (1970-01-01)
    // This is a simplified calculation that doesn't account for all edge cases
    let mut days = 0i64;

    // Years
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Months
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += DAYS_IN_MONTH[(m - 1) as usize] as i64;
        if m == 2 && is_leap_year(year) {
            days += 1;
        }
    }

    // Days
    days += (day - 1) as i64;

    let secs = days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;

    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs as u64))
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_path_normalization() {
        let entry = P4kEntry::new(
            r"Data\Objects\test.cgf".to_string(),
            0,
            0,
            CompressionMethod::Store,
            false,
            0,
            0,
            0,
        );

        let path = entry.output_path();
        assert_eq!(path, PathBuf::from("Data/Objects/test.cgf"));
    }

    #[test]
    fn test_is_dir() {
        let dir_entry = P4kEntry::new(
            "Data/Objects/".to_string(),
            0,
            0,
            CompressionMethod::Store,
            false,
            0,
            0,
            0,
        );
        assert!(dir_entry.is_dir());

        let file_entry = P4kEntry::new(
            "Data/Objects/test.cgf".to_string(),
            0,
            0,
            CompressionMethod::Store,
            false,
            0,
            0,
            0,
        );
        assert!(!file_entry.is_dir());
    }

    #[test]
    fn test_extension() {
        let entry = P4kEntry::new(
            "test.dds".to_string(),
            0,
            0,
            CompressionMethod::Store,
            false,
            0,
            0,
            0,
        );
        assert_eq!(entry.extension(), Some("dds"));
    }
}

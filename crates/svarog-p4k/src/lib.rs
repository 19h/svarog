//! P4K archive reader for Star Citizen game files.
//!
//! The P4K format is a customized ZIP64 archive format used by Star Citizen
//! to package game assets. It supports:
//!
//! - ZIP64 extended format for large archives (>4GB)
//! - AES-128-CBC encryption for protected entries
//! - Zstandard compression (method 100)
//! - DEFLATE compression (method 8)
//! - Custom extra fields (0x5000, 0x5002, 0x5003)
//!
//! See `crates/svarog-p4k/P4K_FORMAT.md` for the dump-backed v1/v2
//! layout notes used by this crate.
//!
//! # Performance Optimizations
//!
//! This crate is heavily optimized for maximum throughput:
//! - SIMD-accelerated null padding detection (AVX2/SSE2)
//! - Arena-allocated entry names (zero per-entry heap allocations)
//! - Parallel extraction with rayon (with `parallel` feature)
//! - Zero-copy memory-mapped file access
//! - Optimized AES decryption/encryption with AES-NI when available
//!
//! # Example
//!
//! ```no_run
//! use svarog_p4k::P4kArchive;
//!
//! let archive = P4kArchive::open("Game.p4k")?;
//!
//! // Preferred: zero-copy iteration
//! for entry in archive.iter() {
//!     println!("{}: {} bytes", entry.name, entry.uncompressed_size);
//! }
//!
//! // Read a specific file
//! if let Some(entry) = archive.get(0) {
//!     let data = archive.read(&entry)?;
//! }
//! # Ok::<(), svarog_p4k::Error>(())
//! ```

mod archive;
pub mod compare;
mod crypto;
mod decompress;
mod entry;
mod error;
mod simd;
mod subarchive;
mod writer;
pub mod zip;

pub use archive::{P4kArchive, P4kEntryRef, P4kVersion};
pub use compare::{compare_archives, is_cryxml_extension, is_text_file};
pub use compare::{FileComparisonResult, P4kComparisonResult};
pub use crypto::signature_metadata_sha256;
pub use entry::P4kEntry;
pub use error::{Error, Result};
pub use subarchive::{
    parse_subarchive, parse_subarchive_cdr_entries, parse_subarchive_cdr_info,
    parse_subarchive_file, parse_subarchive_from_reader, subarchive_eocdr_begin_offset,
    subarchive_eocdr_size, validate_subarchive_eocdr, SubArchive, SubArchiveCdrEntry,
    SubArchiveCdrInfo, SubArchiveStatus, SUBARCHIVE_EOCDR_SIZE,
};
pub use writer::{
    convert_v1_to_v2, convert_v1_to_v2_with_progress, dump_archive_raw_payloads_to_dir,
    dump_archive_to_dir, rewrite_v2_tail_in_place, P4kBuilder, P4kConvertCopyMethod,
    P4kConvertProgress, P4kEntryMetadata, P4kStagedEntry, P4kWriteStats, P4kWriterOptions,
};

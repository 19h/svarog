//! P4K archive reader for Star Citizen game files.
//!
//! The P4K format is a customized ZIP64 archive format used by Star Citizen
//! to package game assets. It supports:
//!
//! - ZIP64 extended format for large archives (>4GB)
//! - AES-256-CBC encryption for protected entries
//! - Zstandard compression (method 100)
//! - DEFLATE compression (method 8)
//! - Custom extra fields (0x5000, 0x5002, 0x5003)
//!
//! # Performance Optimizations
//!
//! This crate is heavily optimized for maximum throughput:
//! - SIMD-accelerated null padding detection (AVX2/SSE2)
//! - Arena-allocated entry names (zero per-entry heap allocations)
//! - Parallel extraction with rayon (with `parallel` feature)
//! - Zero-copy memory-mapped file access
//! - Optimized AES decryption with AES-NI when available
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
mod crypto;
mod decompress;
mod entry;
mod error;
mod simd;
pub mod zip;

pub use archive::{P4kArchive, P4kEntryRef};
pub use entry::P4kEntry;
pub use error::{Error, Result};

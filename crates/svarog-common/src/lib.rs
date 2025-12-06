//! Common utilities for Svarog.
//!
//! This crate provides foundational types and utilities used across all Svarog crates:
//!
//! - [`BinaryReader`] - Zero-copy binary reading from byte slices
//! - [`CigGuid`] - Star Citizen's custom GUID format
//! - [`crc`] - CRC32C hashing utilities
//! - [`simd`] - SIMD-accelerated operations (AVX2, SSE2, NEON)
//! - Color types and other common structures

mod error;
mod guid;
mod reader;

pub mod crc;
pub mod simd;

pub use error::{Error, Result};
pub use guid::CigGuid;
pub use reader::BinaryReader;

/// Re-export zerocopy traits for convenience
pub use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Re-export memchr for SIMD-accelerated byte searching
pub use memchr;

//! Common utilities for Svarog.
//!
//! This crate provides foundational types and utilities used across all Svarog crates:
//!
//! - [`BinaryReader`] - Zero-copy binary reading from byte slices
//! - [`CigGuid`] - Star Citizen's custom GUID format
//! - [`crc32c`] - CRC32C hashing utilities
//! - Color types and other common structures

mod error;
mod guid;
mod reader;

pub mod crc;

pub use error::{Error, Result};
pub use guid::CigGuid;
pub use reader::BinaryReader;

/// Re-export zerocopy traits for convenience
pub use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

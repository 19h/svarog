//! DDS texture file handling for Star Citizen.
//!
//! Star Citizen splits large DDS textures into multiple files for streaming:
//! - `texture.dds` - The base file with header and small mipmaps
//! - `texture.dds.8` - Largest mipmap(s)
//! - `texture.dds.7` - Second largest mipmap(s)
//! - ...down to...
//! - `texture.dds.0` - Smallest split mipmap
//!
//! This crate provides utilities to merge these split files back into
//! a complete DDS file.
//!
//! # Example
//!
//! ```no_run
//! use svarog_dds::merge_dds;
//!
//! // Merge a split DDS file
//! let merged = merge_dds("path/to/texture.dds")?;
//! std::fs::write("merged.dds", &merged)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod header;
mod merge;

pub use error::{Error, Result};
pub use header::{DdsHeader, DdsHeaderDxt10, DdsPixelFormat};
pub use merge::merge_dds;

/// DDS file magic bytes ("DDS ").
pub const DDS_MAGIC: &[u8; 4] = b"DDS ";

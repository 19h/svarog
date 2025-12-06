//! CHF (Character Head File) parser for Star Citizen.
//!
//! CHF files store character customization data including DNA (facial features),
//! materials, and equipped items. This crate can read, modify, and write CHF files.
//!
//! # File Format
//!
//! CHF files are exactly 4096 bytes with the following structure:
//! - 2 bytes: Magic (0x4242)
//! - 2 bytes: Unknown (possibly version)
//! - 4 bytes: CRC32C checksum
//! - 4 bytes: Compressed size
//! - 4 bytes: Uncompressed size
//! - N bytes: Zstd-compressed data
//! - 8 bytes: "diogotr7" magic for modded files, or zeros
//!
//! # Data Structure
//!
//! The decompressed data contains:
//! - Gender GUID (16 bytes)
//! - DNA data (216 bytes of facial morphs)
//! - Item port tree (hierarchical equipment attachments)
//! - Materials (appearance customization)
//!
//! # Example
//!
//! ```no_run
//! use svarog_chf::{ChfFile, parts::ChfData};
//!
//! // Read a character file
//! let chf = ChfFile::from_chf("character.chf")?;
//! println!("Modded: {}", chf.is_modded());
//!
//! // Parse the internal data
//! let data = ChfData::parse(chf.data())?;
//! println!("Gender: {}", data.gender_id());
//!
//! // Access DNA (facial features)
//! for (face_part, blends) in data.dna().iter_face_parts() {
//!     println!("{}: {:?}", face_part, blends);
//! }
//!
//! // Write back (possibly modified)
//! chf.write_to_chf("output.chf")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod file;
pub mod parts;

pub use error::{Error, Result};
pub use file::ChfFile;

// Re-export commonly used types at crate root
pub use parts::{ChfData, Dna, FacePart, ItemPort, Material, NameHash};

//! CHF data parts.
//!
//! This module contains the structures for parsing the internal data
//! of CHF files, including DNA, materials, and item ports.
//!
//! # Structure Overview
//!
//! A CHF file's decompressed data contains:
//! - [`ChfData`]: The main container
//!   - Gender GUID (16 bytes identifying male/female base)
//!   - [`Dna`]: Facial feature morphs (48 blend targets across 12 face parts)
//!   - [`ItemPort`]: Equipment attachment tree (hierarchical ports with GUIDs)
//!   - [`Material`]: Appearance customization (textures and shader parameters)
//!
//! # Name Hashing
//!
//! CHF files use CRC32C hashes instead of strings to identify field names
//! and item types. The [`NameHash`] type provides a lookup dictionary to
//! reverse common hashes back to human-readable names.

mod data;
mod dna;
mod itemport;
mod material;
mod name_hash;

pub use data::ChfData;
pub use dna::{Dna, DnaPart, FacePart, BLENDS_PER_FACE_PART, DNA_PART_COUNT, DNA_SIZE};
pub use itemport::{ItemPort, ItemPortIter};
pub use material::{ColorRgba, Material, MaterialParam, SubMaterial, Texture};
pub use name_hash::{is_known_hash, known_hashes, NameHash};

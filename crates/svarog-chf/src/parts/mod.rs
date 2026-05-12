//! CHF data parts.
//!
//! This module contains the structures for parsing the internal data
//! of CHF files, including DNA, materials, and item ports.
//!
//! # Structure Overview
//!
//! A CHF file's decompressed data contains:
//! - [`ChfData`]: The main container
//!   - Versioned `CharacterCustomization` serializer data
//!   - `modelTag` and `voiceTag`
//!   - [`Dna`]: Facial feature morphs (48 blend targets across 12 face parts)
//!   - [`ItemPort`]: `Loadout` entries with port IDs, item GUIDs, and child counts
//!   - [`Material`]: Appearance customization (textures and shader parameters)
//!   - Decal entries for newer CHF versions
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

pub use data::{
    is_supported_version, ChfData, Decal, CHF_CURRENT_VERSION, CHF_MAX_VERSION, CHF_MIN_VERSION,
};
pub use dna::{Dna, DnaPart, FacePart, BLENDS_PER_FACE_PART, DNA_PART_COUNT, DNA_SIZE};
pub use itemport::{ItemPort, ItemPortIter};
pub use material::{ColorRgba, Material, MaterialParam, SubMaterial, Texture};
pub use name_hash::{is_known_hash, known_hashes, NameHash};

//! CryXmlB header structure.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// CryXmlB file header.
///
/// This structure follows the 8-byte magic "CryXmlB\0" at the start of the file.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CryXmlHeader {
    /// Total XML size (purpose unclear, may be legacy).
    pub xml_size: u32,
    /// Position of node table in file.
    pub node_table_position: u32,
    /// Number of nodes.
    pub node_count: u32,
    /// Position of attribute table in file.
    pub attribute_table_position: u32,
    /// Number of attributes.
    pub attribute_count: u32,
    /// Position of child index table in file.
    pub child_table_position: u32,
    /// Number of child indices.
    pub child_count: u32,
    /// Position of string data in file.
    pub string_data_position: u32,
    /// Size of string data in bytes.
    pub string_data_size: u32,
}

impl CryXmlHeader {
    /// The magic bytes at the start of a CryXmlB file.
    pub const MAGIC: &'static [u8; 8] = b"CryXmlB\0";

    /// Size of the magic bytes.
    pub const MAGIC_LEN: usize = 8;
}

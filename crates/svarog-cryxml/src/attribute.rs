//! CryXmlB attribute structure.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// An attribute in a CryXmlB node.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CryXmlAttribute {
    /// Offset into the string table for the attribute key.
    pub key_string_offset: u32,
    /// Offset into the string table for the attribute value.
    pub value_string_offset: u32,
}

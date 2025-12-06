//! CryXmlB node structure.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// A node in the CryXmlB tree.
///
/// Nodes are stored in a flat array and reference each other by index.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CryXmlNode {
    /// Offset into the string table for the tag name.
    pub tag_string_offset: u32,
    /// Offset into the string table for the content (usually empty).
    pub content_string_offset: u32,
    /// Number of attributes on this node.
    pub attribute_count: u16,
    /// Number of child nodes.
    pub child_count: u16,
    /// Parent node index (-1 for root).
    pub parent_index: i32,
    /// Index of the first attribute in the attribute array.
    pub first_attribute_index: i32,
    /// Index of the first child in the child index array.
    pub first_child_index: i32,
}

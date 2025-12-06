//! String ID types for referencing strings in string tables.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Reference to a string in string table 1.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreStringId {
    /// Offset into the string table.
    id: i32,
}

impl DataCoreStringId {
    /// Check if this is a null/empty string reference.
    pub fn is_null(&self) -> bool {
        self.id() < 0
    }

    /// Get the ID value.
    pub fn id(&self) -> i32 {
        self.id
    }
}

/// Reference to a string in string table 2.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreStringId2 {
    /// Offset into the string table.
    id: i32,
}

impl DataCoreStringId2 {
    /// Check if this is a null/empty string reference.
    pub fn is_null(&self) -> bool {
        self.id() < 0
    }

    /// Get the ID value.
    pub fn id(&self) -> i32 {
        self.id
    }
}

//! String ID types for referencing strings in string tables.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Reference to a string in string table 1.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, PartialEq, Eq, Hash)]
#[repr(C, packed)]
pub struct DataCoreStringId {
    /// Offset into the string table.
    id: i32,
}

impl DataCoreStringId {
    /// Create a new string ID from an offset.
    #[inline]
    pub fn new(offset: i32) -> Self {
        Self { id: offset }
    }

    /// Create a null string ID.
    #[inline]
    pub fn null() -> Self {
        Self { id: -1 }
    }

    /// Check if this is a null/empty string reference.
    pub fn is_null(&self) -> bool {
        self.id() < 0
    }

    /// Get the ID value.
    pub fn id(&self) -> i32 {
        self.id
    }
}

impl Default for DataCoreStringId {
    fn default() -> Self {
        Self::null()
    }
}

/// Reference to a string in string table 2.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, PartialEq, Eq, Hash)]
#[repr(C, packed)]
pub struct DataCoreStringId2 {
    /// Offset into the string table.
    id: i32,
}

impl DataCoreStringId2 {
    /// Create a new string ID from an offset.
    #[inline]
    pub fn new(offset: i32) -> Self {
        Self { id: offset }
    }

    /// Create a null string ID.
    #[inline]
    pub fn null() -> Self {
        Self { id: -1 }
    }

    /// Check if this is a null/empty string reference.
    pub fn is_null(&self) -> bool {
        self.id() < 0
    }

    /// Get the ID value.
    pub fn id(&self) -> i32 {
        self.id
    }
}

impl Default for DataCoreStringId2 {
    fn default() -> Self {
        Self::null()
    }
}

//! Reference and pointer types.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use svarog_common::CigGuid;

/// A pointer to an instance in a value pool.
///
/// Used for strong and weak pointers to reference other structs.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCorePointer {
    /// Index of the struct type.
    pub struct_index: i32,
    /// Instance index within the struct type's data.
    pub instance_index: i32,
}

impl DataCorePointer {
    /// Check if this is a null pointer.
    pub fn is_null(&self) -> bool {
        self.struct_index == -1 || self.instance_index == -1
    }
}

/// A reference to another record by GUID.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreReference {
    /// GUID of the referenced record.
    pub record_id: CigGuid,
    /// Instance index (purpose unclear, possibly legacy).
    pub instance_index: i32,
}

impl DataCoreReference {
    /// Check if this is a null reference.
    pub fn is_null(&self) -> bool {
        self.record_id.is_empty()
    }
}

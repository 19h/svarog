//! Record and data mapping types.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use svarog_common::CigGuid;

use super::{DataCoreStringId, DataCoreStringId2};

/// A record in the DataCore database.
///
/// Records are instances of struct types with actual data.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreRecord {
    /// Offset into string table 2 for the record name.
    pub name_offset: DataCoreStringId2,
    /// Offset into string table 1 for the file name.
    pub file_name_offset: DataCoreStringId,
    /// Index of the struct type this record is an instance of.
    pub struct_index: i32,
    /// Unique identifier for this record.
    pub id: CigGuid,
    /// Instance index within the struct type's data block.
    pub instance_index: u16,
    /// Size of the struct (redundant with struct definition).
    pub struct_size: u16,
}

/// Mapping of struct type to data location.
///
/// This tells us where the data for instances of a struct type is stored.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreDataMapping {
    /// Number of instances of this struct type.
    pub struct_count: u32,
    /// Index of the struct type.
    pub struct_index: i32,
}

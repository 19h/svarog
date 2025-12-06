//! Struct and enum definition types.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::DataCoreStringId2;

/// Definition of a struct type in DataCore.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreStructDefinition {
    /// Offset into string table 2 for the struct name.
    pub name_offset: DataCoreStringId2,
    /// Index of the parent struct type (-1 if none).
    pub parent_type_index: i32,
    /// Number of attributes/properties defined by this struct (not including inherited).
    pub attribute_count: u16,
    /// Index of the first attribute in the property definitions array.
    pub first_attribute_index: u16,
    /// Size of this struct in bytes.
    pub struct_size: u32,
}

/// Definition of an enum type in DataCore.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCoreEnumDefinition {
    /// Offset into string table 2 for the enum name.
    pub name_offset: DataCoreStringId2,
    /// Number of values in this enum.
    pub value_count: u16,
    /// Index of the first value in the enum options array.
    pub first_value_index: u16,
}

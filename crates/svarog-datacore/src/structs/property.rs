//! Property definition types.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::DataCoreStringId2;
use crate::DataType;

/// Definition of a property/attribute in a struct.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DataCorePropertyDefinition {
    /// Offset into string table 2 for the property name.
    pub name_offset: DataCoreStringId2,
    /// Index of the struct type for Class/StrongPointer/WeakPointer types.
    pub struct_index: u16,
    /// Data type of this property.
    pub data_type: u16,
    /// Conversion type (affects how the value is interpreted).
    pub conversion_type: u16,
    /// Padding.
    pub _padding: u16,
}

impl DataCorePropertyDefinition {
    /// Get the data type as an enum.
    pub fn get_data_type(&self) -> Option<DataType> {
        DataType::from_u16(self.data_type)
    }

    /// Check if this property is an array.
    pub fn is_array(&self) -> bool {
        self.conversion_type == 1
    }
}

/// Conversion types for property values.
/// Used for interpreting property data during export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
#[allow(dead_code)]
pub enum ConversionType {
    /// Single value.
    Attribute = 0,
    /// Array of values.
    ComplexArray = 1,
    /// Simple array.
    SimpleArray = 2,
}

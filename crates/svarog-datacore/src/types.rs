//! DataCore data types.

/// Data types used in DataCore property definitions.
///
/// The values are the actual binary values from the DCB file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DataType {
    /// Boolean value.
    Boolean = 0x0001,
    /// Signed 8-bit integer.
    SByte = 0x0002,
    /// Signed 16-bit integer.
    Int16 = 0x0003,
    /// Signed 32-bit integer.
    Int32 = 0x0004,
    /// Signed 64-bit integer.
    Int64 = 0x0005,
    /// Unsigned 8-bit integer.
    Byte = 0x0006,
    /// Unsigned 16-bit integer.
    UInt16 = 0x0007,
    /// Unsigned 32-bit integer.
    UInt32 = 0x0008,
    /// Unsigned 64-bit integer.
    UInt64 = 0x0009,
    /// String value.
    String = 0x000A,
    /// 32-bit floating point.
    Single = 0x000B,
    /// 64-bit floating point.
    Double = 0x000C,
    /// Localized string reference.
    Locale = 0x000D,
    /// GUID value.
    Guid = 0x000E,
    /// Enum choice value.
    EnumChoice = 0x000F,
    /// Nested struct (class).
    Class = 0x0010,
    /// Strong pointer (index-based, owns the target).
    StrongPointer = 0x0110,
    /// Weak pointer (index-based, does not own target).
    WeakPointer = 0x0210,
    /// Reference to another record.
    Reference = 0x0310,
}

impl DataType {
    /// Parse from a u16 value.
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x0001 => Some(Self::Boolean),
            0x0002 => Some(Self::SByte),
            0x0003 => Some(Self::Int16),
            0x0004 => Some(Self::Int32),
            0x0005 => Some(Self::Int64),
            0x0006 => Some(Self::Byte),
            0x0007 => Some(Self::UInt16),
            0x0008 => Some(Self::UInt32),
            0x0009 => Some(Self::UInt64),
            0x000A => Some(Self::String),
            0x000B => Some(Self::Single),
            0x000C => Some(Self::Double),
            0x000D => Some(Self::Locale),
            0x000E => Some(Self::Guid),
            0x000F => Some(Self::EnumChoice),
            0x0010 => Some(Self::Class),
            0x0110 => Some(Self::StrongPointer),
            0x0210 => Some(Self::WeakPointer),
            0x0310 => Some(Self::Reference),
            _ => None,
        }
    }

    /// Get the size in bytes of this data type when stored inline.
    pub fn inline_size(&self) -> usize {
        match self {
            Self::Boolean => 1,
            Self::SByte | Self::Byte => 1,
            Self::Int16 | Self::UInt16 => 2,
            Self::Int32 | Self::UInt32 => 4,
            Self::Int64 | Self::UInt64 => 8,
            Self::Single => 4,
            Self::Double => 8,
            Self::Guid => 16,
            Self::String | Self::Locale | Self::EnumChoice => 4, // DataCoreStringId
            Self::StrongPointer | Self::WeakPointer => 8,        // DataCorePointer
            Self::Reference => 20,                                // DataCoreReference
            Self::Class => 0, // Size depends on the struct definition
        }
    }

    /// Get the string name for this data type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Boolean => "Boolean",
            Self::SByte => "SByte",
            Self::Int16 => "Int16",
            Self::Int32 => "Int32",
            Self::Int64 => "Int64",
            Self::Byte => "Byte",
            Self::UInt16 => "UInt16",
            Self::UInt32 => "UInt32",
            Self::UInt64 => "UInt64",
            Self::String => "String",
            Self::Single => "Single",
            Self::Double => "Double",
            Self::Locale => "Locale",
            Self::Guid => "Guid",
            Self::EnumChoice => "EnumChoice",
            Self::Class => "Class",
            Self::StrongPointer => "StrongPointer",
            Self::WeakPointer => "WeakPointer",
            Self::Reference => "Reference",
        }
    }

    /// Check if this type is a primitive (stored directly in value pools when in arrays).
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            Self::Boolean
                | Self::SByte
                | Self::Byte
                | Self::Int16
                | Self::UInt16
                | Self::Int32
                | Self::UInt32
                | Self::Int64
                | Self::UInt64
                | Self::Single
                | Self::Double
                | Self::Guid
                | Self::String
                | Self::Locale
                | Self::EnumChoice
        )
    }

    /// Check if this type is a pointer/reference type.
    pub fn is_reference(&self) -> bool {
        matches!(
            self,
            Self::Reference | Self::WeakPointer | Self::StrongPointer
        )
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

//! Type-safe value representation for DataCore properties.
//!
//! The `Value` enum represents any value that can be stored in a DataCore property,
//! providing a Rust-native interface to access game data.

use svarog_common::CigGuid;

/// A type-safe value from the DataCore database.
///
/// This enum represents all possible values that can be stored in DataCore properties,
/// providing safe access to the underlying data without exposing raw binary readers.
#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    /// Boolean value.
    Bool(bool),
    /// Signed 8-bit integer.
    Int8(i8),
    /// Signed 16-bit integer.
    Int16(i16),
    /// Signed 32-bit integer.
    Int32(i32),
    /// Signed 64-bit integer.
    Int64(i64),
    /// Unsigned 8-bit integer.
    UInt8(u8),
    /// Unsigned 16-bit integer.
    UInt16(u16),
    /// Unsigned 32-bit integer.
    UInt32(u32),
    /// Unsigned 64-bit integer.
    UInt64(u64),
    /// 32-bit floating point.
    Float(f32),
    /// 64-bit floating point.
    Double(f64),
    /// String value (borrowed from the database).
    String(&'a str),
    /// Localized string value (borrowed from the database).
    Locale(&'a str),
    /// Enum choice value.
    Enum(&'a str),
    /// GUID value.
    Guid(CigGuid),
    /// Nested struct instance.
    Class(InstanceRef),
    /// Strong pointer to another instance.
    StrongPointer(Option<InstanceRef>),
    /// Weak pointer to another instance.
    WeakPointer(Option<InstanceRef>),
    /// Reference to another record (by GUID).
    Reference(Option<RecordRef>),
    /// Array of values.
    Array(ArrayRef),
    /// Null/empty value.
    Null,
}

/// Reference to an instance within the database.
///
/// This is a lightweight handle that can be used to access the actual instance data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceRef {
    /// Index of the struct type.
    pub struct_index: u32,
    /// Index of the instance within the struct's data block.
    pub instance_index: u32,
}

impl InstanceRef {
    /// Create a new instance reference.
    #[inline]
    pub fn new(struct_index: u32, instance_index: u32) -> Self {
        Self { struct_index, instance_index }
    }
}

/// Reference to a record by GUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordRef {
    /// The GUID of the referenced record.
    pub guid: CigGuid,
}

impl RecordRef {
    /// Create a new record reference.
    #[inline]
    pub fn new(guid: CigGuid) -> Self {
        Self { guid }
    }
}

/// Reference to an array of values.
///
/// Arrays in DataCore are stored in value pools with a count and first index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayRef {
    /// The data type of array elements.
    pub element_type: ArrayElementType,
    /// Struct index for Class/Pointer types.
    pub struct_index: u32,
    /// Number of elements in the array.
    pub count: u32,
    /// First index in the value pool.
    pub first_index: u32,
}

/// Element type for arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayElementType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float,
    Double,
    String,
    Locale,
    Enum,
    Guid,
    Class,
    StrongPointer,
    WeakPointer,
    Reference,
}

impl<'a> Value<'a> {
    /// Check if this value is null.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Try to get this value as a boolean.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as an i32.
    #[inline]
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int8(v) => Some(*v as i32),
            Value::Int16(v) => Some(*v as i32),
            Value::Int32(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as an i64.
    #[inline]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int8(v) => Some(*v as i64),
            Value::Int16(v) => Some(*v as i64),
            Value::Int32(v) => Some(*v as i64),
            Value::Int64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as a u32.
    #[inline]
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::UInt8(v) => Some(*v as u32),
            Value::UInt16(v) => Some(*v as u32),
            Value::UInt32(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as a u64.
    #[inline]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt8(v) => Some(*v as u64),
            Value::UInt16(v) => Some(*v as u64),
            Value::UInt32(v) => Some(*v as u64),
            Value::UInt64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as an f32.
    #[inline]
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as an f64.
    #[inline]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v as f64),
            Value::Double(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get this value as a string.
    #[inline]
    pub fn as_str(&self) -> Option<&'a str> {
        match self {
            Value::String(s) | Value::Locale(s) | Value::Enum(s) => Some(s),
            _ => None,
        }
    }

    /// Try to get this value as a GUID.
    #[inline]
    pub fn as_guid(&self) -> Option<CigGuid> {
        match self {
            Value::Guid(g) => Some(*g),
            _ => None,
        }
    }

    /// Try to get this value as an instance reference (for Class types).
    #[inline]
    pub fn as_instance(&self) -> Option<InstanceRef> {
        match self {
            Value::Class(r) => Some(*r),
            Value::StrongPointer(Some(r)) | Value::WeakPointer(Some(r)) => Some(*r),
            _ => None,
        }
    }

    /// Try to get this value as a record reference.
    #[inline]
    pub fn as_record_ref(&self) -> Option<RecordRef> {
        match self {
            Value::Reference(Some(r)) => Some(*r),
            _ => None,
        }
    }

    /// Try to get this value as an array reference.
    #[inline]
    pub fn as_array(&self) -> Option<ArrayRef> {
        match self {
            Value::Array(a) => Some(*a),
            _ => None,
        }
    }
}

impl std::fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(v) => write!(f, "{}", v),
            Value::Int8(v) => write!(f, "{}", v),
            Value::Int16(v) => write!(f, "{}", v),
            Value::Int32(v) => write!(f, "{}", v),
            Value::Int64(v) => write!(f, "{}", v),
            Value::UInt8(v) => write!(f, "{}", v),
            Value::UInt16(v) => write!(f, "{}", v),
            Value::UInt32(v) => write!(f, "{}", v),
            Value::UInt64(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Double(v) => write!(f, "{}", v),
            Value::String(s) | Value::Locale(s) | Value::Enum(s) => write!(f, "{}", s),
            Value::Guid(g) => write!(f, "{}", g),
            Value::Class(r) => write!(f, "Instance({}, {})", r.struct_index, r.instance_index),
            Value::StrongPointer(Some(r)) => write!(f, "StrongPtr({}, {})", r.struct_index, r.instance_index),
            Value::StrongPointer(None) => write!(f, "StrongPtr(null)"),
            Value::WeakPointer(Some(r)) => write!(f, "WeakPtr({}, {})", r.struct_index, r.instance_index),
            Value::WeakPointer(None) => write!(f, "WeakPtr(null)"),
            Value::Reference(Some(r)) => write!(f, "Ref({})", r.guid),
            Value::Reference(None) => write!(f, "Ref(null)"),
            Value::Array(a) => write!(f, "Array[{}]", a.count),
            Value::Null => write!(f, "null"),
        }
    }
}

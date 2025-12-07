//! Instance traversal for DataCore records and structs.
//!
//! This module provides the `Instance` type for accessing struct instances
//! and their properties in a type-safe manner.

use svarog_common::{BinaryReader, CigGuid};

use crate::structs::{
    DataCorePointer, DataCorePropertyDefinition, DataCoreRecord, DataCoreReference,
    DataCoreStringId,
};
use crate::value::{ArrayElementType, ArrayRef, InstanceRef, RecordRef, Value};
use crate::{DataCoreDatabase, DataType};

/// A view into a struct instance within the DataCore database.
///
/// This provides access to the properties of a struct instance with type-safe
/// value extraction. Instances are lightweight and borrow from the database.
#[derive(Clone, Copy)]
pub struct Instance<'a> {
    database: &'a DataCoreDatabase,
    struct_index: u32,
    instance_index: u32,
}

impl<'a> Instance<'a> {
    /// Create a new instance view.
    #[inline]
    pub(crate) fn new(database: &'a DataCoreDatabase, struct_index: u32, instance_index: u32) -> Self {
        Self {
            database,
            struct_index,
            instance_index,
        }
    }

    /// Get the struct type index.
    #[inline]
    pub fn struct_index(&self) -> u32 {
        self.struct_index
    }

    /// Get the instance index within the struct type's data block.
    #[inline]
    pub fn instance_index(&self) -> u32 {
        self.instance_index
    }

    /// Get the name of the struct type.
    #[inline]
    pub fn type_name(&self) -> Option<&'a str> {
        self.database.struct_name(self.struct_index as usize)
    }

    /// Get a reference to this instance.
    #[inline]
    pub fn as_ref(&self) -> InstanceRef {
        InstanceRef::new(self.struct_index, self.instance_index)
    }

    /// Get a property value by name.
    ///
    /// Returns `None` if the property doesn't exist.
    pub fn get(&self, name: &str) -> Option<Value<'a>> {
        let properties = self.database.get_struct_properties(self.struct_index as usize);
        let mut reader = self
            .database
            .get_instance_reader(self.struct_index as usize, self.instance_index as usize);

        for prop in properties {
            let prop_name = self.database.property_name(prop)?;

            if prop_name == name {
                return self.read_property_value(prop, &mut reader);
            } else {
                // Skip this property
                self.skip_property(prop, &mut reader);
            }
        }

        None
    }

    /// Iterate over all properties of this instance.
    pub fn properties(&self) -> PropertyIterator<'a> {
        PropertyIterator::new(self.database, self.struct_index, self.instance_index)
    }

    /// Check if this instance has a property with the given name.
    pub fn has_property(&self, name: &str) -> bool {
        let properties = self.database.get_struct_properties(self.struct_index as usize);
        properties.iter().any(|prop| {
            self.database
                .property_name(prop)
                .map(|n| n == name)
                .unwrap_or(false)
        })
    }

    /// Get a nested instance by property name.
    ///
    /// This is a convenience method for accessing nested Class, StrongPointer,
    /// or WeakPointer properties.
    pub fn get_instance(&self, name: &str) -> Option<Instance<'a>> {
        match self.get(name)? {
            Value::Class(r) | Value::StrongPointer(Some(r)) | Value::WeakPointer(Some(r)) => {
                Some(Instance::new(self.database, r.struct_index, r.instance_index))
            }
            _ => None,
        }
    }

    /// Get a string property value.
    #[inline]
    pub fn get_str(&self, name: &str) -> Option<&'a str> {
        self.get(name).and_then(|v| v.as_str())
    }

    /// Get an integer property value.
    #[inline]
    pub fn get_i32(&self, name: &str) -> Option<i32> {
        self.get(name).and_then(|v| v.as_i32())
    }

    /// Get an integer property value.
    #[inline]
    pub fn get_i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.as_i64())
    }

    /// Get an unsigned integer property value.
    #[inline]
    pub fn get_u32(&self, name: &str) -> Option<u32> {
        self.get(name).and_then(|v| v.as_u32())
    }

    /// Get a float property value.
    #[inline]
    pub fn get_f32(&self, name: &str) -> Option<f32> {
        self.get(name).and_then(|v| v.as_f32())
    }

    /// Get a double property value.
    #[inline]
    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|v| v.as_f64())
    }

    /// Get a boolean property value.
    #[inline]
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(|v| v.as_bool())
    }

    /// Get a GUID property value.
    #[inline]
    pub fn get_guid(&self, name: &str) -> Option<CigGuid> {
        self.get(name).and_then(|v| v.as_guid())
    }

    /// Get an array property and iterate over its elements.
    pub fn get_array(&self, name: &str) -> Option<ArrayIterator<'a>> {
        match self.get(name)? {
            Value::Array(arr) => Some(ArrayIterator::new(self.database, arr)),
            _ => None,
        }
    }

    fn read_property_value(
        &self,
        prop: &DataCorePropertyDefinition,
        reader: &mut BinaryReader<'_>,
    ) -> Option<Value<'a>> {
        let data_type = DataType::from_u16(prop.data_type)?;

        if prop.conversion_type == 0 {
            // Single value
            self.read_single_value(data_type, prop.struct_index as u32, reader)
        } else {
            // Array
            let count = reader.read_i32().ok()? as u32;
            let first_index = reader.read_i32().ok()? as u32;

            Some(Value::Array(ArrayRef {
                element_type: data_type_to_array_element(data_type),
                struct_index: prop.struct_index as u32,
                count,
                first_index,
            }))
        }
    }

    fn read_single_value(
        &self,
        data_type: DataType,
        struct_index: u32,
        reader: &mut BinaryReader<'_>,
    ) -> Option<Value<'a>> {
        Some(match data_type {
            DataType::Boolean => Value::Bool(reader.read_bool().ok()?),
            DataType::SByte => Value::Int8(reader.read_i8().ok()?),
            DataType::Int16 => Value::Int16(reader.read_i16().ok()?),
            DataType::Int32 => Value::Int32(reader.read_i32().ok()?),
            DataType::Int64 => Value::Int64(reader.read_i64().ok()?),
            DataType::Byte => Value::UInt8(reader.read_u8().ok()?),
            DataType::UInt16 => Value::UInt16(reader.read_u16().ok()?),
            DataType::UInt32 => Value::UInt32(reader.read_u32().ok()?),
            DataType::UInt64 => Value::UInt64(reader.read_u64().ok()?),
            DataType::Single => Value::Float(reader.read_f32().ok()?),
            DataType::Double => Value::Double(reader.read_f64().ok()?),
            DataType::Guid => Value::Guid(reader.read_struct().ok()?),
            DataType::String => {
                let string_id: DataCoreStringId = reader.read_struct().ok()?;
                Value::String(self.database.get_string(&string_id).unwrap_or(""))
            }
            DataType::Locale => {
                let string_id: DataCoreStringId = reader.read_struct().ok()?;
                Value::Locale(self.database.get_string(&string_id).unwrap_or(""))
            }
            DataType::EnumChoice => {
                let string_id: DataCoreStringId = reader.read_struct().ok()?;
                Value::Enum(self.database.get_string(&string_id).unwrap_or(""))
            }
            DataType::Class => {
                // For Class, we create an instance reference pointing to a nested read position
                // The instance data is inline, so we need to track the current reader state
                // and create an instance that reads from the same struct data
                Value::Class(InstanceRef::new(struct_index, self.instance_index))
            }
            DataType::StrongPointer => {
                let pointer: DataCorePointer = reader.read_struct().ok()?;
                if pointer.is_null() {
                    Value::StrongPointer(None)
                } else {
                    Value::StrongPointer(Some(InstanceRef::new(
                        pointer.struct_index as u32,
                        pointer.instance_index as u32,
                    )))
                }
            }
            DataType::WeakPointer => {
                let pointer: DataCorePointer = reader.read_struct().ok()?;
                if pointer.is_null() {
                    Value::WeakPointer(None)
                } else {
                    Value::WeakPointer(Some(InstanceRef::new(
                        pointer.struct_index as u32,
                        pointer.instance_index as u32,
                    )))
                }
            }
            DataType::Reference => {
                let reference: DataCoreReference = reader.read_struct().ok()?;
                if reference.is_null() {
                    Value::Reference(None)
                } else {
                    Value::Reference(Some(RecordRef::new(reference.record_id)))
                }
            }
        })
    }

    fn skip_property(&self, prop: &DataCorePropertyDefinition, reader: &mut BinaryReader<'_>) {
        let data_type = match DataType::from_u16(prop.data_type) {
            Some(dt) => dt,
            None => return,
        };

        if prop.conversion_type == 0 {
            // Single value
            if data_type == DataType::Class {
                // For nested classes, we need to skip all their properties recursively
                self.skip_class(prop.struct_index as u32, reader);
            } else {
                reader.advance(data_type.inline_size());
            }
        } else {
            // Array - just skip the count and first_index (8 bytes)
            reader.advance(8);
        }
    }

    fn skip_class(&self, struct_index: u32, reader: &mut BinaryReader<'_>) {
        let properties = self.database.get_struct_properties(struct_index as usize);
        for prop in properties {
            self.skip_property(prop, reader);
        }
    }
}

impl std::fmt::Debug for Instance<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("type", &self.type_name().unwrap_or("Unknown"))
            .field("struct_index", &self.struct_index)
            .field("instance_index", &self.instance_index)
            .finish()
    }
}

/// A record in the DataCore database.
///
/// Records are named instances with a GUID and file association.
#[derive(Clone, Copy)]
pub struct Record<'a> {
    database: &'a DataCoreDatabase,
    record: &'a DataCoreRecord,
}

impl<'a> Record<'a> {
    /// Create a new record view.
    #[inline]
    pub(crate) fn new(database: &'a DataCoreDatabase, record: &'a DataCoreRecord) -> Self {
        Self { database, record }
    }

    /// Get the record's unique identifier.
    #[inline]
    pub fn id(&self) -> CigGuid {
        self.record.id
    }

    /// Get the record's name.
    #[inline]
    pub fn name(&self) -> Option<&'a str> {
        self.database.record_name(self.record)
    }

    /// Get the record's file name/path.
    #[inline]
    pub fn file_name(&self) -> Option<&'a str> {
        self.database.record_file_name(self.record)
    }

    /// Get the struct type index.
    #[inline]
    pub fn struct_index(&self) -> u32 {
        self.record.struct_index as u32
    }

    /// Get the instance index.
    #[inline]
    pub fn instance_index(&self) -> u32 {
        self.record.instance_index as u32
    }

    /// Get the type name.
    #[inline]
    pub fn type_name(&self) -> Option<&'a str> {
        self.database.struct_name(self.record.struct_index as usize)
    }

    /// Check if this is a main record (one per file).
    #[inline]
    pub fn is_main(&self) -> bool {
        self.database.is_main_record(&self.record.id)
    }

    /// Get this record as an instance for property access.
    #[inline]
    pub fn as_instance(&self) -> Instance<'a> {
        Instance::new(
            self.database,
            self.record.struct_index as u32,
            self.record.instance_index as u32,
        )
    }

    /// Get a property value by name (convenience method).
    #[inline]
    pub fn get(&self, name: &str) -> Option<Value<'a>> {
        self.as_instance().get(name)
    }

    /// Iterate over all properties (convenience method).
    #[inline]
    pub fn properties(&self) -> PropertyIterator<'a> {
        self.as_instance().properties()
    }

    /// Get the underlying raw record.
    #[inline]
    pub fn raw(&self) -> &'a DataCoreRecord {
        self.record
    }

    /// Get a string property value.
    #[inline]
    pub fn get_str(&self, name: &str) -> Option<&'a str> {
        self.as_instance().get_str(name)
    }

    /// Get an integer property value.
    #[inline]
    pub fn get_i32(&self, name: &str) -> Option<i32> {
        self.as_instance().get_i32(name)
    }

    /// Get an integer property value.
    #[inline]
    pub fn get_i64(&self, name: &str) -> Option<i64> {
        self.as_instance().get_i64(name)
    }

    /// Get an unsigned integer property value.
    #[inline]
    pub fn get_u32(&self, name: &str) -> Option<u32> {
        self.as_instance().get_u32(name)
    }

    /// Get a float property value.
    #[inline]
    pub fn get_f32(&self, name: &str) -> Option<f32> {
        self.as_instance().get_f32(name)
    }

    /// Get a double property value.
    #[inline]
    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.as_instance().get_f64(name)
    }

    /// Get a boolean property value.
    #[inline]
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.as_instance().get_bool(name)
    }

    /// Get a GUID property value.
    #[inline]
    pub fn get_guid(&self, name: &str) -> Option<CigGuid> {
        self.as_instance().get_guid(name)
    }

    /// Get a nested instance by property name.
    #[inline]
    pub fn get_instance(&self, name: &str) -> Option<Instance<'a>> {
        self.as_instance().get_instance(name)
    }

    /// Get an array property and iterate over its elements.
    #[inline]
    pub fn get_array(&self, name: &str) -> Option<ArrayIterator<'a>> {
        self.as_instance().get_array(name)
    }
}

impl std::fmt::Debug for Record<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("id", &self.id())
            .field("name", &self.name())
            .field("type", &self.type_name())
            .field("file", &self.file_name())
            .finish()
    }
}

/// Iterator over properties of an instance.
pub struct PropertyIterator<'a> {
    database: &'a DataCoreDatabase,
    properties: Vec<&'a DataCorePropertyDefinition>,
    reader: BinaryReader<'a>,
    index: usize,
}

impl<'a> PropertyIterator<'a> {
    fn new(database: &'a DataCoreDatabase, struct_index: u32, instance_index: u32) -> Self {
        let properties = database.get_struct_properties(struct_index as usize);
        let reader = database.get_instance_reader(struct_index as usize, instance_index as usize);

        Self {
            database,
            properties,
            reader,
            index: 0,
        }
    }
}

impl<'a> Iterator for PropertyIterator<'a> {
    type Item = Property<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.properties.len() {
            return None;
        }

        let prop = self.properties[self.index];
        self.index += 1;

        let name = self.database.property_name(prop).unwrap_or("Unknown");
        let data_type = DataType::from_u16(prop.data_type)?;

        let value = if prop.conversion_type == 0 {
            // Single value
            read_single_value(self.database, data_type, prop.struct_index as u32, &mut self.reader)?
        } else {
            // Array
            let count = self.reader.read_i32().ok()? as u32;
            let first_index = self.reader.read_i32().ok()? as u32;

            Value::Array(ArrayRef {
                element_type: data_type_to_array_element(data_type),
                struct_index: prop.struct_index as u32,
                count,
                first_index,
            })
        };

        Some(Property { name, value })
    }
}

/// A property name-value pair.
#[derive(Debug, Clone)]
pub struct Property<'a> {
    /// The property name.
    pub name: &'a str,
    /// The property value.
    pub value: Value<'a>,
}

/// Iterator over array elements.
pub struct ArrayIterator<'a> {
    database: &'a DataCoreDatabase,
    array: ArrayRef,
    current_index: u32,
}

impl<'a> ArrayIterator<'a> {
    fn new(database: &'a DataCoreDatabase, array: ArrayRef) -> Self {
        Self {
            database,
            array,
            current_index: 0,
        }
    }

    /// Get the number of elements in the array.
    #[inline]
    pub fn len(&self) -> usize {
        self.array.count as usize
    }

    /// Check if the array is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.array.count == 0
    }
}

impl<'a> Iterator for ArrayIterator<'a> {
    type Item = Value<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_index >= self.array.count {
            return None;
        }

        let index = (self.array.first_index + self.current_index) as usize;
        self.current_index += 1;

        Some(match self.array.element_type {
            ArrayElementType::Bool => {
                Value::Bool(self.database.bool_value(index).unwrap_or(false))
            }
            ArrayElementType::Int8 => Value::Int8(self.database.int8_value(index).unwrap_or(0)),
            ArrayElementType::Int16 => Value::Int16(self.database.int16_value(index).unwrap_or(0)),
            ArrayElementType::Int32 => Value::Int32(self.database.int32_value(index).unwrap_or(0)),
            ArrayElementType::Int64 => Value::Int64(self.database.int64_value(index).unwrap_or(0)),
            ArrayElementType::UInt8 => Value::UInt8(self.database.uint8_value(index).unwrap_or(0)),
            ArrayElementType::UInt16 => {
                Value::UInt16(self.database.uint16_value(index).unwrap_or(0))
            }
            ArrayElementType::UInt32 => {
                Value::UInt32(self.database.uint32_value(index).unwrap_or(0))
            }
            ArrayElementType::UInt64 => {
                Value::UInt64(self.database.uint64_value(index).unwrap_or(0))
            }
            ArrayElementType::Float => {
                Value::Float(self.database.float_value(index).unwrap_or(0.0))
            }
            ArrayElementType::Double => {
                Value::Double(self.database.double_value(index).unwrap_or(0.0))
            }
            ArrayElementType::String => {
                let s = self
                    .database
                    .string_id_value(index)
                    .and_then(|id| self.database.get_string(&id))
                    .unwrap_or("");
                Value::String(s)
            }
            ArrayElementType::Locale => {
                let s = self
                    .database
                    .locale_value(index)
                    .and_then(|id| self.database.get_string(&id))
                    .unwrap_or("");
                Value::Locale(s)
            }
            ArrayElementType::Enum => {
                let s = self
                    .database
                    .enum_value(index)
                    .and_then(|id| self.database.get_string(&id))
                    .unwrap_or("");
                Value::Enum(s)
            }
            ArrayElementType::Guid => {
                Value::Guid(self.database.guid_value(index).unwrap_or_default())
            }
            ArrayElementType::Class => Value::Class(InstanceRef::new(
                self.array.struct_index,
                index as u32,
            )),
            ArrayElementType::StrongPointer => {
                let ptr = self.database.strong_value(index);
                match ptr {
                    Some(p) if !p.is_null() => Value::StrongPointer(Some(InstanceRef::new(
                        p.struct_index as u32,
                        p.instance_index as u32,
                    ))),
                    _ => Value::StrongPointer(None),
                }
            }
            ArrayElementType::WeakPointer => {
                let ptr = self.database.weak_value(index);
                match ptr {
                    Some(p) if !p.is_null() => Value::WeakPointer(Some(InstanceRef::new(
                        p.struct_index as u32,
                        p.instance_index as u32,
                    ))),
                    _ => Value::WeakPointer(None),
                }
            }
            ArrayElementType::Reference => {
                let reference = self.database.reference_value(index);
                match reference {
                    Some(r) if !r.is_null() => {
                        Value::Reference(Some(RecordRef::new(r.record_id)))
                    }
                    _ => Value::Reference(None),
                }
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.array.count - self.current_index) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ArrayIterator<'_> {}

// Helper functions

fn read_single_value<'a>(
    database: &'a DataCoreDatabase,
    data_type: DataType,
    struct_index: u32,
    reader: &mut BinaryReader<'_>,
) -> Option<Value<'a>> {
    Some(match data_type {
        DataType::Boolean => Value::Bool(reader.read_bool().ok()?),
        DataType::SByte => Value::Int8(reader.read_i8().ok()?),
        DataType::Int16 => Value::Int16(reader.read_i16().ok()?),
        DataType::Int32 => Value::Int32(reader.read_i32().ok()?),
        DataType::Int64 => Value::Int64(reader.read_i64().ok()?),
        DataType::Byte => Value::UInt8(reader.read_u8().ok()?),
        DataType::UInt16 => Value::UInt16(reader.read_u16().ok()?),
        DataType::UInt32 => Value::UInt32(reader.read_u32().ok()?),
        DataType::UInt64 => Value::UInt64(reader.read_u64().ok()?),
        DataType::Single => Value::Float(reader.read_f32().ok()?),
        DataType::Double => Value::Double(reader.read_f64().ok()?),
        DataType::Guid => Value::Guid(reader.read_struct().ok()?),
        DataType::String => {
            let string_id: DataCoreStringId = reader.read_struct().ok()?;
            Value::String(database.get_string(&string_id).unwrap_or(""))
        }
        DataType::Locale => {
            let string_id: DataCoreStringId = reader.read_struct().ok()?;
            Value::Locale(database.get_string(&string_id).unwrap_or(""))
        }
        DataType::EnumChoice => {
            let string_id: DataCoreStringId = reader.read_struct().ok()?;
            Value::Enum(database.get_string(&string_id).unwrap_or(""))
        }
        DataType::Class => {
            // For inline classes, we need to track position differently
            // This is complex - for now return a reference
            Value::Class(InstanceRef::new(struct_index, 0))
        }
        DataType::StrongPointer => {
            let pointer: DataCorePointer = reader.read_struct().ok()?;
            if pointer.is_null() {
                Value::StrongPointer(None)
            } else {
                Value::StrongPointer(Some(InstanceRef::new(
                    pointer.struct_index as u32,
                    pointer.instance_index as u32,
                )))
            }
        }
        DataType::WeakPointer => {
            let pointer: DataCorePointer = reader.read_struct().ok()?;
            if pointer.is_null() {
                Value::WeakPointer(None)
            } else {
                Value::WeakPointer(Some(InstanceRef::new(
                    pointer.struct_index as u32,
                    pointer.instance_index as u32,
                )))
            }
        }
        DataType::Reference => {
            let reference: DataCoreReference = reader.read_struct().ok()?;
            if reference.is_null() {
                Value::Reference(None)
            } else {
                Value::Reference(Some(RecordRef::new(reference.record_id)))
            }
        }
    })
}

fn data_type_to_array_element(data_type: DataType) -> ArrayElementType {
    match data_type {
        DataType::Boolean => ArrayElementType::Bool,
        DataType::SByte => ArrayElementType::Int8,
        DataType::Int16 => ArrayElementType::Int16,
        DataType::Int32 => ArrayElementType::Int32,
        DataType::Int64 => ArrayElementType::Int64,
        DataType::Byte => ArrayElementType::UInt8,
        DataType::UInt16 => ArrayElementType::UInt16,
        DataType::UInt32 => ArrayElementType::UInt32,
        DataType::UInt64 => ArrayElementType::UInt64,
        DataType::Single => ArrayElementType::Float,
        DataType::Double => ArrayElementType::Double,
        DataType::String => ArrayElementType::String,
        DataType::Locale => ArrayElementType::Locale,
        DataType::EnumChoice => ArrayElementType::Enum,
        DataType::Guid => ArrayElementType::Guid,
        DataType::Class => ArrayElementType::Class,
        DataType::StrongPointer => ArrayElementType::StrongPointer,
        DataType::WeakPointer => ArrayElementType::WeakPointer,
        DataType::Reference => ArrayElementType::Reference,
    }
}

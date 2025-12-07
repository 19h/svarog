//! DataCore database builder for creating and modifying DCB files.
//!
//! This module provides a builder API for constructing DataCore databases
//! from scratch or modifying existing ones.
//!
//! # Creating a New Database
//!
//! ```no_run
//! use svarog_datacore::{DataCoreBuilder, DataType};
//!
//! let mut builder = DataCoreBuilder::new();
//!
//! // Define a struct type
//! let weapon_struct = builder.add_struct("Weapon", None);
//! builder.add_property(weapon_struct, "name", DataType::String);
//! builder.add_property(weapon_struct, "damage", DataType::Single);
//! builder.add_property(weapon_struct, "ammoCount", DataType::Int32);
//!
//! // Add a record
//! let record = builder.add_record("LaserRifle", weapon_struct, "weapons/laser_rifle.xml");
//! builder.set_string(record, "name", "Laser Rifle");
//! builder.set_float(record, "damage", 150.0);
//! builder.set_i32(record, "ammoCount", 50);
//!
//! // Write to file
//! builder.write_to_file("output.dcb")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::io::{self, Write};
use std::path::Path;

use hashbrown::HashMap as FastHashMap;
use rustc_hash::FxHasher;
use std::hash::BuildHasherDefault;

use svarog_common::CigGuid;

use crate::structs::{
    DataCoreDataMapping, DataCoreEnumDefinition, DataCorePointer, DataCorePropertyDefinition,
    DataCoreRecord, DataCoreReference, DataCoreStringId, DataCoreStringId2,
    DataCoreStructDefinition,
};
use crate::DataType;

type FxHashMap<K, V> = FastHashMap<K, V, BuildHasherDefault<FxHasher>>;

/// DCB file version.
const DCB_VERSION: u32 = 6;

/// Builder for creating DataCore databases.
///
/// This builder allows you to construct a DCB file from scratch by defining
/// struct types, properties, enums, and records with their values.
#[derive(Debug)]
pub struct DataCoreBuilder {
    // Schema definitions
    structs: Vec<StructDef>,
    properties: Vec<PropertyDef>,
    enums: Vec<EnumDef>,
    enum_options: Vec<String>,

    // Records and their data
    records: Vec<RecordDef>,

    // Value pools
    bool_pool: Vec<bool>,
    int8_pool: Vec<i8>,
    int16_pool: Vec<i16>,
    int32_pool: Vec<i32>,
    int64_pool: Vec<i64>,
    uint8_pool: Vec<u8>,
    uint16_pool: Vec<u16>,
    uint32_pool: Vec<u32>,
    uint64_pool: Vec<u64>,
    float_pool: Vec<f32>,
    double_pool: Vec<f64>,
    guid_pool: Vec<CigGuid>,
    string_id_pool: Vec<DataCoreStringId>,
    locale_pool: Vec<DataCoreStringId>,
    enum_value_pool: Vec<DataCoreStringId>,
    strong_pool: Vec<DataCorePointer>,
    weak_pool: Vec<DataCorePointer>,
    reference_pool: Vec<DataCoreReference>,

    // String tables
    string_table_1: StringTable, // File names, content strings
    string_table_2: StringTable, // Type names, property names, record names

    // Instance data (per-struct)
    instance_data: Vec<Vec<u8>>,

    // Mapping from struct index to data mapping info
    struct_instance_counts: Vec<u32>,

    // Original data mapping order (for preserving order when loading from existing DB)
    // If Some, use this order when writing; otherwise generate fresh order
    original_data_mapping_order: Option<Vec<usize>>,
}

/// A struct type definition being built.
#[derive(Debug, Clone)]
struct StructDef {
    name: String,
    parent_index: i32,
    first_property_index: u16,
    property_count: u16,
    size: u32,
}

/// A property definition being built.
#[derive(Debug, Clone)]
struct PropertyDef {
    name: String,
    struct_index: u16,
    data_type: DataType,
    conversion_type: u16, // Preserves exact original value (0=attribute, 1=complex array, 2=simple array, etc.)
}

/// An enum definition being built.
#[derive(Debug, Clone)]
struct EnumDef {
    name: String,
    first_value_index: u16,
    value_count: u16,
}

/// A record being built.
#[derive(Debug, Clone)]
struct RecordDef {
    name: String,
    file_name: String,
    struct_index: u32,
    guid: CigGuid,
    instance_index: u16,
}

/// String table for interning strings.
#[derive(Debug, Default)]
struct StringTable {
    data: Vec<u8>,
    offsets: FxHashMap<String, i32>,
}

impl StringTable {
    fn new() -> Self {
        Self::default()
    }

    /// Create from raw data (for copying existing databases).
    fn from_raw(data: &[u8]) -> Self {
        let mut offsets = FxHashMap::default();
        let data = data.to_vec();

        // Build offset map by scanning for null-terminated strings
        let mut offset = 0;
        while offset < data.len() {
            let start = offset;
            // Find null terminator
            while offset < data.len() && data[offset] != 0 {
                offset += 1;
            }
            // Parse as UTF-8 and add to map
            if let Ok(s) = std::str::from_utf8(&data[start..offset]) {
                offsets.insert(s.to_string(), start as i32);
            }
            offset += 1; // Skip null terminator
        }

        Self { data, offsets }
    }

    /// Add a string and return its offset.
    fn add(&mut self, s: &str) -> i32 {
        if let Some(&offset) = self.offsets.get(s) {
            return offset;
        }

        let offset = self.data.len() as i32;
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0); // null terminator
        self.offsets.insert(s.to_string(), offset);
        offset
    }

    /// Get the total size of the string table.
    fn len(&self) -> usize {
        self.data.len()
    }
}

/// Handle to a struct type in the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructHandle(pub u32);

/// Handle to an enum type in the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumHandle(pub u32);

/// Handle to a record in the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordHandle(pub u32);

impl DataCoreBuilder {
    /// Create a new empty database builder.
    pub fn new() -> Self {
        Self {
            structs: Vec::new(),
            properties: Vec::new(),
            enums: Vec::new(),
            enum_options: Vec::new(),
            records: Vec::new(),
            bool_pool: Vec::new(),
            int8_pool: Vec::new(),
            int16_pool: Vec::new(),
            int32_pool: Vec::new(),
            int64_pool: Vec::new(),
            uint8_pool: Vec::new(),
            uint16_pool: Vec::new(),
            uint32_pool: Vec::new(),
            uint64_pool: Vec::new(),
            float_pool: Vec::new(),
            double_pool: Vec::new(),
            guid_pool: Vec::new(),
            string_id_pool: Vec::new(),
            locale_pool: Vec::new(),
            enum_value_pool: Vec::new(),
            strong_pool: Vec::new(),
            weak_pool: Vec::new(),
            reference_pool: Vec::new(),
            string_table_1: StringTable::new(),
            string_table_2: StringTable::new(),
            instance_data: Vec::new(),
            struct_instance_counts: Vec::new(),
            original_data_mapping_order: None,
        }
    }

    /// Create a builder from an existing database.
    ///
    /// This allows loading an existing DCB file and modifying it.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_datacore::{DataCoreDatabase, DataCoreBuilder};
    ///
    /// let db = DataCoreDatabase::open("Game.dcb")?;
    /// let mut builder = DataCoreBuilder::from_database(&db)?;
    ///
    /// // Modify the builder...
    ///
    /// builder.write_to_file("Modified.dcb")?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_database(db: &crate::DataCoreDatabase) -> io::Result<Self> {
        let mut builder = Self::new();

        // Copy struct definitions
        for (i, struct_def) in db.struct_definitions().iter().enumerate() {
            let name = db.struct_name(i).unwrap_or("Unknown");
            builder.structs.push(StructDef {
                name: name.to_string(),
                parent_index: struct_def.parent_type_index,
                first_property_index: struct_def.first_attribute_index,
                property_count: struct_def.attribute_count,
                size: struct_def.struct_size,
            });
            builder.instance_data.push(Vec::new());
            builder.struct_instance_counts.push(0);
        }

        // Copy property definitions
        for prop in db.property_definitions() {
            let name = db.property_name(prop).unwrap_or("Unknown");
            let data_type = DataType::from_u16(prop.data_type).unwrap_or(DataType::Int32);
            builder.properties.push(PropertyDef {
                name: name.to_string(),
                struct_index: prop.struct_index,
                data_type,
                conversion_type: prop.conversion_type,
            });
        }

        // Copy enum definitions - preserve original indices
        for (i, enum_def) in db.enum_definitions().iter().enumerate() {
            let name = db.enum_name(i).unwrap_or("Unknown");

            builder.enums.push(EnumDef {
                name: name.to_string(),
                first_value_index: enum_def.first_value_index,
                value_count: enum_def.value_count,
            });
        }

        // Copy ALL enum options in order (by walking through the pool)
        for i in 0..db.pool_counts().enum_option_count {
            if let Some(opt_id) = db.enum_option_value(i) {
                let opt_str = db.get_string2(&opt_id).unwrap_or("");
                builder.enum_options.push(opt_str.to_string());
            }
        }

        // Get instance counts from the original data mappings (NOT from records!)
        // Records only cover top-level entities with names, but there are many
        // instances that are embedded data (e.g., animationParams in LadderConfig)
        for mapping in db.data_mappings() {
            let struct_index = mapping.struct_index as usize;
            builder.struct_instance_counts[struct_index] = mapping.struct_count;
        }

        // Copy records
        for raw_record in db.records() {
            let name = db.record_name(raw_record).unwrap_or("Unknown");
            let file_name = db.record_file_name(raw_record).unwrap_or("unknown");
            let struct_index = raw_record.struct_index as u32;
            let instance_index = raw_record.instance_index;

            builder.records.push(RecordDef {
                name: name.to_string(),
                file_name: file_name.to_string(),
                struct_index,
                guid: raw_record.id,
                instance_index,
            });
        }

        // Copy instance data for each struct type
        for (struct_idx, struct_def) in db.struct_definitions().iter().enumerate() {
            let instance_count = builder.struct_instance_counts[struct_idx];
            let struct_size = struct_def.struct_size as usize;

            let mut data = Vec::with_capacity(instance_count as usize * struct_size);

            for instance_idx in 0..instance_count {
                let reader = db.get_instance_reader(struct_idx, instance_idx as usize);
                data.extend_from_slice(reader.remaining_bytes());
            }

            builder.instance_data[struct_idx] = data;
        }

        // Copy string tables - the instance data contains string IDs that are offsets
        // into these tables, so we must preserve them exactly
        builder.string_table_1 = StringTable::from_raw(db.raw_string_table_1());
        builder.string_table_2 = StringTable::from_raw(db.raw_string_table_2());

        // Copy value pools - the instance data may contain array headers with indices
        // into these pools, so we must preserve them exactly
        use crate::PoolType;

        // Copy all value pools as raw bytes and reinterpret

        // Bool pool
        let bool_data = db.raw_pool_data(PoolType::Bool);
        builder.bool_pool = bool_data.iter().map(|&b| b != 0).collect();

        // Int8 pool
        let int8_data = db.raw_pool_data(PoolType::Int8);
        builder.int8_pool = int8_data.iter().map(|&b| b as i8).collect();

        // Int16 pool
        let int16_data = db.raw_pool_data(PoolType::Int16);
        builder.int16_pool = int16_data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        // Int32 pool
        let int32_data = db.raw_pool_data(PoolType::Int32);
        builder.int32_pool = int32_data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        // Int64 pool
        let int64_data = db.raw_pool_data(PoolType::Int64);
        builder.int64_pool = int64_data
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();

        // UInt8 pool
        builder.uint8_pool = db.raw_pool_data(PoolType::UInt8).to_vec();

        // UInt16 pool
        let uint16_data = db.raw_pool_data(PoolType::UInt16);
        builder.uint16_pool = uint16_data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        // UInt32 pool
        let uint32_data = db.raw_pool_data(PoolType::UInt32);
        builder.uint32_pool = uint32_data
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        // UInt64 pool
        let uint64_data = db.raw_pool_data(PoolType::UInt64);
        builder.uint64_pool = uint64_data
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();

        // Float pool
        let float_data = db.raw_pool_data(PoolType::Float);
        builder.float_pool = float_data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        // Double pool
        let double_data = db.raw_pool_data(PoolType::Double);
        builder.double_pool = double_data
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();

        // GUID pool
        let guid_data = db.raw_pool_data(PoolType::Guid);
        builder.guid_pool = guid_data
            .chunks_exact(16)
            .map(|c| {
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(c);
                CigGuid::from_bytes(bytes)
            })
            .collect();

        // StringId pool
        let string_id_data = db.raw_pool_data(PoolType::StringId);
        builder.string_id_pool = string_id_data
            .chunks_exact(4)
            .map(|c| DataCoreStringId::new(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();

        // Locale pool
        let locale_data = db.raw_pool_data(PoolType::Locale);
        builder.locale_pool = locale_data
            .chunks_exact(4)
            .map(|c| DataCoreStringId::new(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();

        // EnumValue pool
        let enum_value_data = db.raw_pool_data(PoolType::EnumValue);
        builder.enum_value_pool = enum_value_data
            .chunks_exact(4)
            .map(|c| DataCoreStringId::new(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();

        // Strong pointer pool
        let strong_data = db.raw_pool_data(PoolType::Strong);
        builder.strong_pool = strong_data
            .chunks_exact(8)
            .map(|c| DataCorePointer {
                struct_index: i32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                instance_index: i32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            })
            .collect();

        // Weak pointer pool
        let weak_data = db.raw_pool_data(PoolType::Weak);
        builder.weak_pool = weak_data
            .chunks_exact(8)
            .map(|c| DataCorePointer {
                struct_index: i32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                instance_index: i32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            })
            .collect();

        // Reference pool
        let ref_data = db.raw_pool_data(PoolType::Reference);
        builder.reference_pool = ref_data
            .chunks_exact(20)
            .map(|c| {
                let mut guid_bytes = [0u8; 16];
                guid_bytes.copy_from_slice(&c[0..16]);
                DataCoreReference {
                    record_id: CigGuid::from_bytes(guid_bytes),
                    instance_index: i32::from_le_bytes([c[16], c[17], c[18], c[19]]),
                }
            })
            .collect();

        // Preserve the original data mapping order
        builder.original_data_mapping_order = Some(
            db.data_mappings()
                .iter()
                .map(|m| m.struct_index as usize)
                .collect()
        );


        Ok(builder)
    }

    /// Add a struct type definition.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the struct type
    /// * `parent` - Optional parent struct for inheritance
    ///
    /// # Returns
    ///
    /// A handle to the new struct type.
    pub fn add_struct(&mut self, name: &str, parent: Option<StructHandle>) -> StructHandle {
        let index = self.structs.len() as u32;
        let parent_index = parent.map(|h| h.0 as i32).unwrap_or(-1);

        // Inherit the parent's size if there is a parent
        let initial_size = parent
            .map(|h| self.structs[h.0 as usize].size)
            .unwrap_or(0);

        self.structs.push(StructDef {
            name: name.to_string(),
            parent_index,
            first_property_index: self.properties.len() as u16,
            property_count: 0,
            size: initial_size,
        });

        self.instance_data.push(Vec::new());
        self.struct_instance_counts.push(0);

        StructHandle(index)
    }

    /// Add a property to a struct type.
    ///
    /// # Arguments
    ///
    /// * `struct_handle` - The struct to add the property to
    /// * `name` - The property name
    /// * `data_type` - The data type of the property
    ///
    /// Properties must be added in order, immediately after creating the struct.
    pub fn add_property(&mut self, struct_handle: StructHandle, name: &str, data_type: DataType) {
        self.add_property_internal(struct_handle, name, data_type, None, false);
    }

    /// Add an array property to a struct type.
    pub fn add_array_property(
        &mut self,
        struct_handle: StructHandle,
        name: &str,
        element_type: DataType,
    ) {
        self.add_property_internal(struct_handle, name, element_type, None, true);
    }

    /// Add a property that references another struct type (for Class, StrongPointer, WeakPointer).
    pub fn add_typed_property(
        &mut self,
        struct_handle: StructHandle,
        name: &str,
        data_type: DataType,
        target_struct: StructHandle,
    ) {
        self.add_property_internal(struct_handle, name, data_type, Some(target_struct), false);
    }

    fn add_property_internal(
        &mut self,
        struct_handle: StructHandle,
        name: &str,
        data_type: DataType,
        target_struct: Option<StructHandle>,
        is_array: bool,
    ) {
        let struct_index = target_struct.map(|h| h.0 as u16).unwrap_or(0);
        // For new properties: 0 = not array, 1 = complex array
        let conversion_type = if is_array { 1 } else { 0 };

        self.properties.push(PropertyDef {
            name: name.to_string(),
            struct_index,
            data_type,
            conversion_type,
        });

        // Update the struct's property count and size
        let s = &mut self.structs[struct_handle.0 as usize];
        s.property_count += 1;

        // Calculate size contribution
        let size = if is_array {
            8 // count + first_index
        } else {
            data_type.inline_size()
        };
        s.size += size as u32;
    }

    /// Add an enum type definition.
    pub fn add_enum(&mut self, name: &str, values: &[&str]) -> EnumHandle {
        let index = self.enums.len() as u32;
        let first_value_index = self.enum_options.len() as u16;

        for value in values {
            self.enum_options.push(value.to_string());
        }

        self.enums.push(EnumDef {
            name: name.to_string(),
            first_value_index,
            value_count: values.len() as u16,
        });

        EnumHandle(index)
    }

    /// Add a record (instance of a struct type).
    ///
    /// # Arguments
    ///
    /// * `name` - The record name
    /// * `struct_handle` - The struct type this record is an instance of
    /// * `file_name` - The file path for this record
    ///
    /// # Returns
    ///
    /// A handle to the new record.
    pub fn add_record(
        &mut self,
        name: &str,
        struct_handle: StructHandle,
        file_name: &str,
    ) -> RecordHandle {
        self.add_record_with_guid(name, struct_handle, file_name, CigGuid::random())
    }

    /// Add a record with a specific GUID.
    pub fn add_record_with_guid(
        &mut self,
        name: &str,
        struct_handle: StructHandle,
        file_name: &str,
        guid: CigGuid,
    ) -> RecordHandle {
        let record_index = self.records.len() as u32;
        let struct_index = struct_handle.0;

        // Allocate instance data for this record
        let instance_index = self.struct_instance_counts[struct_index as usize];
        self.struct_instance_counts[struct_index as usize] += 1;

        // Initialize instance data with zeros
        let struct_size = self.structs[struct_index as usize].size as usize;
        let instance_data = &mut self.instance_data[struct_index as usize];
        instance_data.resize(instance_data.len() + struct_size, 0);

        self.records.push(RecordDef {
            name: name.to_string(),
            file_name: file_name.to_string(),
            struct_index,
            guid,
            instance_index: instance_index as u16,
        });

        RecordHandle(record_index)
    }

    /// Set a boolean property value.
    pub fn set_bool(&mut self, record: RecordHandle, property: &str, value: bool) {
        self.set_value(record, property, |offset, data| {
            data[offset] = value as u8;
        });
    }

    /// Set an i8 property value.
    pub fn set_i8(&mut self, record: RecordHandle, property: &str, value: i8) {
        self.set_value(record, property, |offset, data| {
            data[offset] = value as u8;
        });
    }

    /// Set an i16 property value.
    pub fn set_i16(&mut self, record: RecordHandle, property: &str, value: i16) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set an i32 property value.
    pub fn set_i32(&mut self, record: RecordHandle, property: &str, value: i32) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set an i64 property value.
    pub fn set_i64(&mut self, record: RecordHandle, property: &str, value: i64) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a u8 property value.
    pub fn set_u8(&mut self, record: RecordHandle, property: &str, value: u8) {
        self.set_value(record, property, |offset, data| {
            data[offset] = value;
        });
    }

    /// Set a u16 property value.
    pub fn set_u16(&mut self, record: RecordHandle, property: &str, value: u16) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a u32 property value.
    pub fn set_u32(&mut self, record: RecordHandle, property: &str, value: u32) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a u64 property value.
    pub fn set_u64(&mut self, record: RecordHandle, property: &str, value: u64) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a float property value.
    pub fn set_float(&mut self, record: RecordHandle, property: &str, value: f32) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a double property value.
    pub fn set_double(&mut self, record: RecordHandle, property: &str, value: f64) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        });
    }

    /// Set a string property value.
    pub fn set_string(&mut self, record: RecordHandle, property: &str, value: &str) {
        let string_id = DataCoreStringId::new(self.string_table_1.add(value));
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&string_id.id().to_le_bytes());
        });
    }

    /// Set a GUID property value.
    pub fn set_guid(&mut self, record: RecordHandle, property: &str, value: CigGuid) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 16].copy_from_slice(value.as_bytes());
        });
    }

    /// Set a strong pointer property.
    pub fn set_strong_pointer(
        &mut self,
        record: RecordHandle,
        property: &str,
        target: Option<RecordHandle>,
    ) {
        let pointer = match target {
            Some(target) => {
                let target_record = &self.records[target.0 as usize];
                DataCorePointer {
                    struct_index: target_record.struct_index as i32,
                    instance_index: target_record.instance_index as i32,
                }
            }
            None => DataCorePointer {
                struct_index: -1,
                instance_index: -1,
            },
        };

        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&pointer.struct_index.to_le_bytes());
            data[offset + 4..offset + 8].copy_from_slice(&pointer.instance_index.to_le_bytes());
        });
    }

    /// Set a weak pointer property.
    pub fn set_weak_pointer(
        &mut self,
        record: RecordHandle,
        property: &str,
        target: Option<RecordHandle>,
    ) {
        // Same as strong pointer for now
        self.set_strong_pointer(record, property, target);
    }

    /// Set a reference property (by GUID).
    pub fn set_reference(&mut self, record: RecordHandle, property: &str, target_guid: CigGuid) {
        let reference = DataCoreReference {
            record_id: target_guid,
            instance_index: 0,
        };

        self.set_value(record, property, |offset, data| {
            data[offset..offset + 16].copy_from_slice(reference.record_id.as_bytes());
            data[offset + 16..offset + 20].copy_from_slice(&reference.instance_index.to_le_bytes());
        });
    }

    /// Set an array property with boolean values.
    pub fn set_bool_array(&mut self, record: RecordHandle, property: &str, values: &[bool]) {
        let first_index = self.bool_pool.len() as i32;
        self.bool_pool.extend_from_slice(values);
        self.set_array_header(record, property, values.len() as i32, first_index);
    }

    /// Set an array property with i32 values.
    pub fn set_i32_array(&mut self, record: RecordHandle, property: &str, values: &[i32]) {
        let first_index = self.int32_pool.len() as i32;
        self.int32_pool.extend_from_slice(values);
        self.set_array_header(record, property, values.len() as i32, first_index);
    }

    /// Set an array property with f32 values.
    pub fn set_float_array(&mut self, record: RecordHandle, property: &str, values: &[f32]) {
        let first_index = self.float_pool.len() as i32;
        self.float_pool.extend_from_slice(values);
        self.set_array_header(record, property, values.len() as i32, first_index);
    }

    /// Set an array property with string values.
    pub fn set_string_array(&mut self, record: RecordHandle, property: &str, values: &[&str]) {
        let first_index = self.string_id_pool.len() as i32;
        for value in values {
            let string_id = DataCoreStringId::new(self.string_table_1.add(value));
            self.string_id_pool.push(string_id);
        }
        self.set_array_header(record, property, values.len() as i32, first_index);
    }

    /// Set an array property with GUID values.
    pub fn set_guid_array(&mut self, record: RecordHandle, property: &str, values: &[CigGuid]) {
        let first_index = self.guid_pool.len() as i32;
        self.guid_pool.extend_from_slice(values);
        self.set_array_header(record, property, values.len() as i32, first_index);
    }

    fn set_array_header(
        &mut self,
        record: RecordHandle,
        property: &str,
        count: i32,
        first_index: i32,
    ) {
        self.set_value(record, property, |offset, data| {
            data[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
            data[offset + 4..offset + 8].copy_from_slice(&first_index.to_le_bytes());
        });
    }

    fn set_value<F>(&mut self, record: RecordHandle, property: &str, setter: F)
    where
        F: FnOnce(usize, &mut [u8]),
    {
        let record_def = &self.records[record.0 as usize];
        let struct_index = record_def.struct_index as usize;
        let instance_index = record_def.instance_index as usize;
        let struct_def = &self.structs[struct_index];
        let struct_size = struct_def.size as usize;

        // Find property offset by traversing the inheritance chain
        if let Some(offset) = self.find_property_offset(struct_index, property) {
            let instance_start = instance_index * struct_size;
            let data = &mut self.instance_data[struct_index];
            setter(instance_start + offset, data);
        }
    }

    /// Find the offset of a property within a struct, traversing the inheritance chain.
    fn find_property_offset(&self, struct_index: usize, property: &str) -> Option<usize> {
        let struct_def = &self.structs[struct_index];

        // First check parent's properties (they come first in memory layout)
        let mut offset = 0;
        if struct_def.parent_index >= 0 {
            let parent_index = struct_def.parent_index as usize;
            if let Some(parent_offset) = self.find_property_offset(parent_index, property) {
                return Some(parent_offset);
            }
            // Parent's properties weren't a match, but we need to skip past parent's size
            offset = self.structs[parent_index].size as usize;
        }

        // Check this struct's own properties
        let first_prop = struct_def.first_property_index as usize;
        let prop_count = struct_def.property_count as usize;

        for i in first_prop..first_prop + prop_count {
            let prop = &self.properties[i];
            if prop.name == property {
                return Some(offset);
            }

            // Advance offset - array if conversion_type != 0
            offset += if prop.conversion_type != 0 {
                8
            } else {
                prop.data_type.inline_size()
            };
        }

        None
    }

    /// Build the database and write to a file.
    pub fn write_to_file<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let data = self.build()?;
        std::fs::write(path, data)
    }

    /// Build the database and return the raw bytes.
    pub fn build(&mut self) -> io::Result<Vec<u8>> {
        // Finalize all strings before building
        self.finalize_strings();

        let mut output = Vec::new();
        self.write_to(&mut output)?;
        Ok(output)
    }

    /// Write the database to a writer.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        // Build all the binary structures

        // Header (17 u32 values = 68 bytes)
        let unknown1: u32 = 0;
        let version: u32 = DCB_VERSION;
        let unknown2: u32 = 0;
        let unknown3: u32 = 0;

        // Convert definitions to binary structs
        let struct_defs: Vec<DataCoreStructDefinition> = self
            .structs
            .iter()
            .map(|s| {
                let name_offset = DataCoreStringId2::new(self.string_table_2_offset(&s.name));
                DataCoreStructDefinition {
                    name_offset,
                    parent_type_index: s.parent_index,
                    attribute_count: s.property_count,
                    first_attribute_index: s.first_property_index,
                    struct_size: s.size,
                }
            })
            .collect();

        let property_defs: Vec<DataCorePropertyDefinition> = self
            .properties
            .iter()
            .map(|p| DataCorePropertyDefinition {
                name_offset: DataCoreStringId2::new(self.string_table_2_offset(&p.name)),
                struct_index: p.struct_index,
                data_type: p.data_type as u16,
                conversion_type: p.conversion_type,
                _padding: 0,
            })
            .collect();

        let enum_defs: Vec<DataCoreEnumDefinition> = self
            .enums
            .iter()
            .map(|e| DataCoreEnumDefinition {
                name_offset: DataCoreStringId2::new(self.string_table_2_offset(&e.name)),
                value_count: e.value_count,
                first_value_index: e.first_value_index,
            })
            .collect();

        // Build data mappings - use original order if available, otherwise generate fresh
        // IMPORTANT: When loading from an existing database, we must preserve ALL mappings
        // including those with struct_count=0, as the mapping order defines the data layout.
        let data_mappings: Vec<DataCoreDataMapping> = if let Some(ref original_order) = self.original_data_mapping_order {
            // Use the original order from the source database - include ALL mappings
            original_order
                .iter()
                .map(|&i| DataCoreDataMapping {
                    struct_count: self.struct_instance_counts[i],
                    struct_index: i as i32,
                })
                .collect()
        } else {
            // Generate fresh mappings - for new databases, only include non-empty structs
            self.structs
                .iter()
                .enumerate()
                .filter(|(i, _)| self.struct_instance_counts[*i] > 0)
                .map(|(i, _)| DataCoreDataMapping {
                    struct_count: self.struct_instance_counts[i],
                    struct_index: i as i32,
                })
                .collect()
        };

        // Build records
        let records: Vec<DataCoreRecord> = self
            .records
            .iter()
            .map(|r| DataCoreRecord {
                name_offset: DataCoreStringId2::new(self.string_table_2_offset(&r.name)),
                file_name_offset: DataCoreStringId::new(self.string_table_1.offsets.get(&r.file_name).copied().unwrap_or(-1)),
                struct_index: r.struct_index as i32,
                id: r.guid,
                instance_index: r.instance_index,
                struct_size: self.structs[r.struct_index as usize].size as u16,
            })
            .collect();

        // Build enum options as string IDs
        let enum_option_ids: Vec<DataCoreStringId2> = self
            .enum_options
            .iter()
            .map(|s| DataCoreStringId2::new(self.string_table_2_offset(s)))
            .collect();

        // Write header
        writer.write_all(&unknown1.to_le_bytes())?;
        writer.write_all(&version.to_le_bytes())?;
        writer.write_all(&unknown2.to_le_bytes())?;
        writer.write_all(&unknown3.to_le_bytes())?;

        // Counts
        writer.write_all(&(struct_defs.len() as i32).to_le_bytes())?;
        writer.write_all(&(property_defs.len() as i32).to_le_bytes())?;
        writer.write_all(&(enum_defs.len() as i32).to_le_bytes())?;
        writer.write_all(&(data_mappings.len() as i32).to_le_bytes())?;
        writer.write_all(&(records.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.bool_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.int8_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.int16_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.int32_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.int64_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.uint8_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.uint16_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.uint32_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.uint64_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.float_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.double_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.guid_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.string_id_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.locale_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.enum_value_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.strong_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.weak_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.reference_pool.len() as i32).to_le_bytes())?;
        writer.write_all(&(enum_option_ids.len() as i32).to_le_bytes())?;
        writer.write_all(&(self.string_table_1.len() as u32).to_le_bytes())?;
        writer.write_all(&(self.string_table_2.len() as u32).to_le_bytes())?;

        // Write definitions
        for s in &struct_defs {
            writer.write_all(zerocopy::IntoBytes::as_bytes(s))?;
        }
        for p in &property_defs {
            writer.write_all(zerocopy::IntoBytes::as_bytes(p))?;
        }
        for e in &enum_defs {
            writer.write_all(zerocopy::IntoBytes::as_bytes(e))?;
        }
        for m in &data_mappings {
            writer.write_all(zerocopy::IntoBytes::as_bytes(m))?;
        }
        for r in &records {
            writer.write_all(zerocopy::IntoBytes::as_bytes(r))?;
        }

        // Write value pools
        // int8
        for v in &self.int8_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // int16
        for v in &self.int16_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // int32
        for v in &self.int32_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // int64
        for v in &self.int64_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // uint8
        for v in &self.uint8_pool {
            writer.write_all(&[*v])?;
        }
        // uint16
        for v in &self.uint16_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // uint32
        for v in &self.uint32_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // uint64
        for v in &self.uint64_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // bool
        for v in &self.bool_pool {
            writer.write_all(&[*v as u8])?;
        }
        // float
        for v in &self.float_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // double
        for v in &self.double_pool {
            writer.write_all(&v.to_le_bytes())?;
        }
        // guid
        for v in &self.guid_pool {
            writer.write_all(v.as_bytes())?;
        }

        // Reference pools
        // string_id
        for v in &self.string_id_pool {
            writer.write_all(&v.id().to_le_bytes())?;
        }
        // locale
        for v in &self.locale_pool {
            writer.write_all(&v.id().to_le_bytes())?;
        }
        // enum_value
        for v in &self.enum_value_pool {
            writer.write_all(&v.id().to_le_bytes())?;
        }
        // strong
        for v in &self.strong_pool {
            writer.write_all(zerocopy::IntoBytes::as_bytes(v))?;
        }
        // weak
        for v in &self.weak_pool {
            writer.write_all(zerocopy::IntoBytes::as_bytes(v))?;
        }
        // reference
        for v in &self.reference_pool {
            writer.write_all(zerocopy::IntoBytes::as_bytes(v))?;
        }
        // enum options
        for v in &enum_option_ids {
            writer.write_all(&v.id().to_le_bytes())?;
        }

        // Write string tables
        writer.write_all(&self.string_table_1.data)?;
        writer.write_all(&self.string_table_2.data)?;

        // Write instance data (data section) in data_mappings order
        // This is critical - instance data must be written in the order
        // specified by data_mappings, not in struct index order
        for mapping in &data_mappings {
            let struct_index = mapping.struct_index as usize;
            writer.write_all(&self.instance_data[struct_index])?;
        }

        Ok(())
    }

    fn string_table_2_offset(&self, s: &str) -> i32 {
        self.string_table_2.offsets.get(s).copied().unwrap_or(-1)
    }

    /// Pre-populate string table 2 with all names.
    /// This should be called before build() to ensure all strings have offsets.
    fn finalize_strings(&mut self) {
        // Add all struct names
        for s in &self.structs {
            self.string_table_2.add(&s.name);
        }

        // Add all property names
        for p in &self.properties {
            self.string_table_2.add(&p.name);
        }

        // Add all enum names
        for e in &self.enums {
            self.string_table_2.add(&e.name);
        }

        // Add all enum option values
        for opt in &self.enum_options {
            self.string_table_2.add(opt);
        }

        // Add all record names
        for r in &self.records {
            self.string_table_2.add(&r.name);
            self.string_table_1.add(&r.file_name);
        }
    }
}

impl Default for DataCoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let mut builder = DataCoreBuilder::new();

        // Add struct
        let weapon = builder.add_struct("Weapon", None);
        builder.add_property(weapon, "name", DataType::String);
        builder.add_property(weapon, "damage", DataType::Single);

        // Add record
        let record = builder.add_record("TestWeapon", weapon, "weapons/test.xml");
        builder.set_string(record, "name", "Test Weapon");
        builder.set_float(record, "damage", 100.0);

        // Build should succeed (finalize_strings is called automatically)
        let data = builder.build().unwrap();
        assert!(!data.is_empty());
    }

    #[test]
    fn test_string_table() {
        let mut table = StringTable::new();
        let offset1 = table.add("hello");
        let offset2 = table.add("world");
        let offset3 = table.add("hello"); // duplicate

        assert_eq!(offset1, 0);
        assert_eq!(offset2, 6); // "hello\0" = 6 bytes
        assert_eq!(offset3, offset1); // duplicate returns same offset
    }
}

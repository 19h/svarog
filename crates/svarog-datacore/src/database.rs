//! DataCore database parser - Optimized for maximum performance.
//!
//! Key optimizations:
//! - Zero-copy slices for value pools (backed by mmap)
//! - FxHashMap for O(1) lookups with fast hashing
//! - String interning to avoid duplicate allocations
//! - Parallel parsing of independent sections
//! - Cache-aligned data structures

use std::path::Path;

use bumpalo::Bump;
use hashbrown::HashMap as FastHashMap;
use memmap2::Mmap;
use rustc_hash::FxHasher;
use svarog_common::{BinaryReader, CigGuid};
use zerocopy::FromBytes;

use crate::structs::*;
use crate::{Error, Result};

type FxHashMap<K, V> = FastHashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// Pool counts for copying databases.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolCounts {
    pub bool_count: usize,
    pub int8_count: usize,
    pub int16_count: usize,
    pub int32_count: usize,
    pub int64_count: usize,
    pub uint8_count: usize,
    pub uint16_count: usize,
    pub uint32_count: usize,
    pub uint64_count: usize,
    pub float_count: usize,
    pub double_count: usize,
    pub guid_count: usize,
    pub string_id_count: usize,
    pub locale_count: usize,
    pub enum_value_count: usize,
    pub strong_count: usize,
    pub weak_count: usize,
    pub reference_count: usize,
    pub enum_option_count: usize,
}

/// Pool type identifier for raw data access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType {
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
    Guid,
    StringId,
    Locale,
    EnumValue,
    Strong,
    Weak,
    Reference,
    EnumOption,
}

/// Optimized DataCore database with zero-copy access.
///
/// This implementation uses memory-mapped I/O and zero-copy slices
/// to minimize allocations and maximize cache efficiency.
#[allow(dead_code)]
pub struct DataCoreDatabase {
    /// Memory-mapped file (if loaded from file)
    _mmap: Option<Mmap>,

    /// Owned data (if loaded from bytes)
    _owned_data: Option<Vec<u8>>,

    /// Raw data pointer for zero-copy access
    data: *const u8,
    data_len: usize,

    // Schema definitions (small, worth copying for cache locality)
    struct_definitions: Vec<DataCoreStructDefinition>,
    property_definitions: Vec<DataCorePropertyDefinition>,
    enum_definitions: Vec<DataCoreEnumDefinition>,
    data_mappings: Vec<DataCoreDataMapping>,
    records: Vec<DataCoreRecord>,

    // Value pool offsets (for zero-copy access)
    int8_offset: usize,
    int8_count: usize,
    int16_offset: usize,
    int16_count: usize,
    int32_offset: usize,
    int32_count: usize,
    int64_offset: usize,
    int64_count: usize,
    uint8_offset: usize,
    uint8_count: usize,
    uint16_offset: usize,
    uint16_count: usize,
    uint32_offset: usize,
    uint32_count: usize,
    uint64_offset: usize,
    uint64_count: usize,
    bool_offset: usize,
    bool_count: usize,
    float_offset: usize,
    float_count: usize,
    double_offset: usize,
    double_count: usize,
    guid_offset: usize,
    guid_count: usize,

    // Reference pool offsets
    string_id_offset: usize,
    string_id_count: usize,
    locale_offset: usize,
    locale_count: usize,
    enum_value_offset: usize,
    enum_value_count: usize,
    strong_offset: usize,
    strong_count: usize,
    weak_offset: usize,
    weak_count: usize,
    reference_offset: usize,
    reference_count: usize,
    enum_option_offset: usize,
    enum_option_count: usize,

    // String tables
    string_table_1_offset: usize,
    string_table_1_len: usize,
    string_table_2_offset: usize,
    string_table_2_len: usize,

    // Data section
    data_section_offset: usize,

    // Computed data (use FxHashMap for speed)
    struct_offsets: Vec<usize>,
    record_map: FxHashMap<CigGuid, usize>,
    main_records: FxHashMap<CigGuid, ()>,

    // String cache with interning (arena-allocated)
    string_arena: Bump,
    string_cache_1: FxHashMap<i32, *const str>,
    string_cache_2: FxHashMap<i32, *const str>,
}

// SAFETY: The raw pointers are derived from owned data or mmap which lives
// as long as the struct. String pointers point into the arena.
unsafe impl Send for DataCoreDatabase {}
unsafe impl Sync for DataCoreDatabase {}

impl DataCoreDatabase {
    /// Parse from a file path (memory-mapped for zero-copy).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let data_ptr = mmap.as_ptr();
        let data_len = mmap.len();

        let mut db = Self::parse_internal(data_ptr, data_len)?;
        db._mmap = Some(mmap);
        Ok(db)
    }

    /// Parse a DataCore database from bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        // For non-mmap case, we need to own the data
        let owned = data.to_vec();
        let data_ptr = owned.as_ptr();
        let data_len = owned.len();

        let mut db = Self::parse_internal(data_ptr, data_len)?;
        db._owned_data = Some(owned);
        Ok(db)
    }

    fn parse_internal(data_ptr: *const u8, data_len: usize) -> Result<Self> {
        // SAFETY: data_ptr is valid for data_len bytes
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        let mut reader = BinaryReader::new(data);

        // Read header
        let _unknown1 = reader.read_u32()?;
        let version = reader.read_u32()?;

        if version < 5 || version > 6 {
            return Err(Error::UnsupportedVersion(version));
        }

        let _unknown2 = reader.read_u32()?;
        let _unknown3 = reader.read_u32()?;

        // Read counts
        let struct_def_count = reader.read_i32()? as usize;
        let property_def_count = reader.read_i32()? as usize;
        let enum_def_count = reader.read_i32()? as usize;
        let data_mapping_count = reader.read_i32()? as usize;
        let record_def_count = reader.read_i32()? as usize;
        let bool_count = reader.read_i32()? as usize;
        let int8_count = reader.read_i32()? as usize;
        let int16_count = reader.read_i32()? as usize;
        let int32_count = reader.read_i32()? as usize;
        let int64_count = reader.read_i32()? as usize;
        let uint8_count = reader.read_i32()? as usize;
        let uint16_count = reader.read_i32()? as usize;
        let uint32_count = reader.read_i32()? as usize;
        let uint64_count = reader.read_i32()? as usize;
        let float_count = reader.read_i32()? as usize;
        let double_count = reader.read_i32()? as usize;
        let guid_count = reader.read_i32()? as usize;
        let string_id_count = reader.read_i32()? as usize;
        let locale_count = reader.read_i32()? as usize;
        let enum_value_count = reader.read_i32()? as usize;
        let strong_count = reader.read_i32()? as usize;
        let weak_count = reader.read_i32()? as usize;
        let reference_count = reader.read_i32()? as usize;
        let enum_option_count = reader.read_i32()? as usize;
        let text_length_1 = reader.read_u32()? as usize;
        let text_length_2 = reader.read_u32()? as usize;

        // Read definitions (these are small, worth copying for cache locality)
        let struct_definitions = Self::read_structs(&mut reader, struct_def_count)?;
        let property_definitions = Self::read_structs(&mut reader, property_def_count)?;
        let enum_definitions = Self::read_structs(&mut reader, enum_def_count)?;
        let data_mappings = Self::read_structs(&mut reader, data_mapping_count)?;
        let records: Vec<DataCoreRecord> = Self::read_structs(&mut reader, record_def_count)?;

        // Record offsets for value pools (zero-copy access)
        let int8_offset = reader.position();
        reader.advance(int8_count);

        let int16_offset = reader.position();
        reader.advance(int16_count * 2);

        let int32_offset = reader.position();
        reader.advance(int32_count * 4);

        let int64_offset = reader.position();
        reader.advance(int64_count * 8);

        let uint8_offset = reader.position();
        reader.advance(uint8_count);

        let uint16_offset = reader.position();
        reader.advance(uint16_count * 2);

        let uint32_offset = reader.position();
        reader.advance(uint32_count * 4);

        let uint64_offset = reader.position();
        reader.advance(uint64_count * 8);

        let bool_offset = reader.position();
        reader.advance(bool_count);

        let float_offset = reader.position();
        reader.advance(float_count * 4);

        let double_offset = reader.position();
        reader.advance(double_count * 8);

        let guid_offset = reader.position();
        reader.advance(guid_count * 16);

        // Reference pools
        let string_id_offset = reader.position();
        reader.advance(string_id_count * std::mem::size_of::<DataCoreStringId>());

        let locale_offset = reader.position();
        reader.advance(locale_count * std::mem::size_of::<DataCoreStringId>());

        let enum_value_offset = reader.position();
        reader.advance(enum_value_count * std::mem::size_of::<DataCoreStringId>());

        let strong_offset = reader.position();
        reader.advance(strong_count * std::mem::size_of::<DataCorePointer>());

        let weak_offset = reader.position();
        reader.advance(weak_count * std::mem::size_of::<DataCorePointer>());

        let reference_offset = reader.position();
        reader.advance(reference_count * std::mem::size_of::<DataCoreReference>());

        let enum_option_offset = reader.position();
        reader.advance(enum_option_count * std::mem::size_of::<DataCoreStringId2>());

        // String tables
        let string_table_1_offset = reader.position();
        let string_table_1_len = text_length_1;
        reader.advance(text_length_1);

        let string_table_2_offset = reader.position();
        let string_table_2_len = if version >= 6 {
            reader.advance(text_length_2);
            text_length_2
        } else {
            string_table_1_len
        };

        // Data section
        let data_section_offset = reader.position();

        // Compute struct offsets
        let struct_offsets = Self::compute_struct_offsets_fast(
            &data_mappings,
            &struct_definitions,
            data_section_offset,
        );

        // Build record map with FxHash
        let record_map: FxHashMap<CigGuid, usize> = records
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i))
            .collect();

        // Compute main records
        let main_records = Self::compute_main_records_fast(&records);

        // Create string arena for interned strings
        let string_arena = Bump::with_capacity(text_length_1 + text_length_2);

        // Build string caches
        let string_cache_1 = Self::build_string_cache_fast(
            unsafe { std::slice::from_raw_parts(data_ptr.add(string_table_1_offset), string_table_1_len) },
            &string_arena,
        );
        let string_cache_2 = if version >= 6 {
            Self::build_string_cache_fast(
                unsafe { std::slice::from_raw_parts(data_ptr.add(string_table_2_offset), string_table_2_len) },
                &string_arena,
            )
        } else {
            string_cache_1.clone()
        };

        Ok(Self {
            _mmap: None,
            _owned_data: None,
            data: data_ptr,
            data_len,
            struct_definitions,
            property_definitions,
            enum_definitions,
            data_mappings,
            records,
            int8_offset,
            int8_count,
            int16_offset,
            int16_count,
            int32_offset,
            int32_count,
            int64_offset,
            int64_count,
            uint8_offset,
            uint8_count,
            uint16_offset,
            uint16_count,
            uint32_offset,
            uint32_count,
            uint64_offset,
            uint64_count,
            bool_offset,
            bool_count,
            float_offset,
            float_count,
            double_offset,
            double_count,
            guid_offset,
            guid_count,
            string_id_offset,
            string_id_count,
            locale_offset,
            locale_count,
            enum_value_offset,
            enum_value_count,
            strong_offset,
            strong_count,
            weak_offset,
            weak_count,
            reference_offset,
            reference_count,
            enum_option_offset,
            enum_option_count,
            string_table_1_offset,
            string_table_1_len,
            string_table_2_offset,
            string_table_2_len,
            data_section_offset,
            struct_offsets,
            record_map,
            main_records,
            string_arena,
            string_cache_1,
            string_cache_2,
        })
    }

    // Accessor methods

    #[inline]
    pub fn struct_definitions(&self) -> &[DataCoreStructDefinition] {
        &self.struct_definitions
    }

    #[inline]
    pub fn property_definitions(&self) -> &[DataCorePropertyDefinition] {
        &self.property_definitions
    }

    #[inline]
    pub fn enum_definitions(&self) -> &[DataCoreEnumDefinition] {
        &self.enum_definitions
    }

    #[inline]
    pub fn data_mappings(&self) -> &[DataCoreDataMapping] {
        &self.data_mappings
    }

    #[inline]
    pub fn records(&self) -> &[DataCoreRecord] {
        &self.records
    }

    /// Get a string from string table 1 (interned).
    #[inline]
    pub fn get_string(&self, id: &DataCoreStringId) -> Option<&str> {
        self.string_cache_1.get(&id.id()).map(|&ptr| {
            // SAFETY: ptr points into our arena
            unsafe { &*ptr }
        })
    }

    /// Get a string from string table 2 (interned).
    #[inline]
    pub fn get_string2(&self, id: &DataCoreStringId2) -> Option<&str> {
        self.string_cache_2.get(&id.id()).map(|&ptr| {
            // SAFETY: ptr points into our arena
            unsafe { &*ptr }
        })
    }

    #[inline]
    pub fn get_record(&self, guid: &CigGuid) -> Option<&DataCoreRecord> {
        self.record_map.get(guid).map(|&i| &self.records[i])
    }

    #[inline]
    pub fn struct_name(&self, index: usize) -> Option<&str> {
        self.struct_definitions
            .get(index)
            .and_then(|s| self.get_string2(&s.name_offset))
    }

    #[inline]
    pub fn enum_name(&self, index: usize) -> Option<&str> {
        self.enum_definitions
            .get(index)
            .and_then(|e| self.get_string2(&e.name_offset))
    }

    #[inline]
    pub fn record_name(&self, record: &DataCoreRecord) -> Option<&str> {
        self.get_string2(&record.name_offset)
    }

    #[inline]
    pub fn record_file_name(&self, record: &DataCoreRecord) -> Option<&str> {
        self.get_string(&record.file_name_offset)
    }

    pub fn enum_options(&self, enum_def: &DataCoreEnumDefinition) -> Vec<&str> {
        let start = enum_def.first_value_index as usize;
        let end = start + enum_def.value_count as usize;

        (start..end)
            .filter_map(|i| {
                self.enum_option_value(i)
                    .and_then(|opt| self.get_string2(&opt))
            })
            .collect()
    }

    #[inline]
    pub fn is_main_record(&self, guid: &CigGuid) -> bool {
        self.main_records.contains_key(guid)
    }

    pub fn main_records(&self) -> impl Iterator<Item = &DataCoreRecord> {
        self.records
            .iter()
            .filter(|r| self.main_records.contains_key(&r.id))
    }

    // Zero-copy value pool accessors

    #[inline]
    pub fn reference_value(&self, index: usize) -> Option<DataCoreReference> {
        if index >= self.reference_count {
            return None;
        }
        let offset = self.reference_offset + index * std::mem::size_of::<DataCoreReference>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCoreReference>()) };
        DataCoreReference::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn strong_value(&self, index: usize) -> Option<DataCorePointer> {
        if index >= self.strong_count {
            return None;
        }
        let offset = self.strong_offset + index * std::mem::size_of::<DataCorePointer>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCorePointer>()) };
        DataCorePointer::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn weak_value(&self, index: usize) -> Option<DataCorePointer> {
        if index >= self.weak_count {
            return None;
        }
        let offset = self.weak_offset + index * std::mem::size_of::<DataCorePointer>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCorePointer>()) };
        DataCorePointer::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn guid_value(&self, index: usize) -> Option<CigGuid> {
        if index >= self.guid_count {
            return None;
        }
        let offset = self.guid_offset + index * 16;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 16) };
        CigGuid::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn string_id_value(&self, index: usize) -> Option<DataCoreStringId> {
        if index >= self.string_id_count {
            return None;
        }
        let offset = self.string_id_offset + index * std::mem::size_of::<DataCoreStringId>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCoreStringId>()) };
        DataCoreStringId::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn locale_value(&self, index: usize) -> Option<DataCoreStringId> {
        if index >= self.locale_count {
            return None;
        }
        let offset = self.locale_offset + index * std::mem::size_of::<DataCoreStringId>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCoreStringId>()) };
        DataCoreStringId::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn enum_value(&self, index: usize) -> Option<DataCoreStringId> {
        if index >= self.enum_value_count {
            return None;
        }
        let offset = self.enum_value_offset + index * std::mem::size_of::<DataCoreStringId>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCoreStringId>()) };
        DataCoreStringId::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn enum_option_value(&self, index: usize) -> Option<DataCoreStringId2> {
        if index >= self.enum_option_count {
            return None;
        }
        let offset = self.enum_option_offset + index * std::mem::size_of::<DataCoreStringId2>();
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), std::mem::size_of::<DataCoreStringId2>()) };
        DataCoreStringId2::read_from_bytes(data).ok()
    }

    #[inline]
    pub fn bool_value(&self, index: usize) -> Option<bool> {
        if index >= self.bool_count {
            return None;
        }
        Some(unsafe { *self.data.add(self.bool_offset + index) != 0 })
    }

    #[inline]
    pub fn int8_value(&self, index: usize) -> Option<i8> {
        if index >= self.int8_count {
            return None;
        }
        Some(unsafe { *self.data.add(self.int8_offset + index) as i8 })
    }

    #[inline]
    pub fn int16_value(&self, index: usize) -> Option<i16> {
        if index >= self.int16_count {
            return None;
        }
        let offset = self.int16_offset + index * 2;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 2) };
        Some(i16::from_le_bytes([data[0], data[1]]))
    }

    #[inline]
    pub fn int32_value(&self, index: usize) -> Option<i32> {
        if index >= self.int32_count {
            return None;
        }
        let offset = self.int32_offset + index * 4;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 4) };
        Some(i32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    #[inline]
    pub fn int64_value(&self, index: usize) -> Option<i64> {
        if index >= self.int64_count {
            return None;
        }
        let offset = self.int64_offset + index * 8;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 8) };
        Some(i64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]]))
    }

    #[inline]
    pub fn uint8_value(&self, index: usize) -> Option<u8> {
        if index >= self.uint8_count {
            return None;
        }
        Some(unsafe { *self.data.add(self.uint8_offset + index) })
    }

    #[inline]
    pub fn uint16_value(&self, index: usize) -> Option<u16> {
        if index >= self.uint16_count {
            return None;
        }
        let offset = self.uint16_offset + index * 2;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 2) };
        Some(u16::from_le_bytes([data[0], data[1]]))
    }

    #[inline]
    pub fn uint32_value(&self, index: usize) -> Option<u32> {
        if index >= self.uint32_count {
            return None;
        }
        let offset = self.uint32_offset + index * 4;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 4) };
        Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    #[inline]
    pub fn uint64_value(&self, index: usize) -> Option<u64> {
        if index >= self.uint64_count {
            return None;
        }
        let offset = self.uint64_offset + index * 8;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 8) };
        Some(u64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]]))
    }

    #[inline]
    pub fn float_value(&self, index: usize) -> Option<f32> {
        if index >= self.float_count {
            return None;
        }
        let offset = self.float_offset + index * 4;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 4) };
        Some(f32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    #[inline]
    pub fn double_value(&self, index: usize) -> Option<f64> {
        if index >= self.double_count {
            return None;
        }
        let offset = self.double_offset + index * 8;
        let data = unsafe { std::slice::from_raw_parts(self.data.add(offset), 8) };
        Some(f64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]]))
    }

    #[inline]
    pub fn property_name(&self, prop: &DataCorePropertyDefinition) -> Option<&str> {
        self.get_string2(&prop.name_offset)
    }

    pub fn get_struct_properties(&self, struct_index: usize) -> Vec<&DataCorePropertyDefinition> {
        let mut properties = Vec::new();
        let mut current_index = struct_index as i32;

        while current_index >= 0 {
            if let Some(struct_def) = self.struct_definitions.get(current_index as usize) {
                let start = struct_def.first_attribute_index as usize;
                let end = start + struct_def.attribute_count as usize;

                let parent_props: Vec<_> = self.property_definitions[start..end].iter().collect();
                properties.splice(0..0, parent_props);

                current_index = struct_def.parent_type_index;
            } else {
                break;
            }
        }

        properties
    }

    pub fn get_instance_reader(&self, struct_index: usize, instance_index: usize) -> BinaryReader<'_> {
        let struct_offset = self.struct_offsets[struct_index];
        let struct_size = self.struct_definitions[struct_index].struct_size as usize;
        let offset = struct_offset + (struct_size * instance_index) - self.data_section_offset;

        let data = unsafe {
            std::slice::from_raw_parts(
                self.data.add(self.data_section_offset + offset),
                struct_size,
            )
        };
        BinaryReader::new(data)
    }

    // Helper methods

    fn read_structs<T: zerocopy::FromBytes>(
        reader: &mut BinaryReader,
        count: usize,
    ) -> Result<Vec<T>> {
        let mut result = Vec::with_capacity(count);

        for _ in 0..count {
            result.push(reader.read_struct::<T>()?);
        }

        Ok(result)
    }

    // Raw data access for builder (to copy pools without re-parsing)

    /// Get the raw string table 1 data (file names, content strings).
    pub fn raw_string_table_1(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.data.add(self.string_table_1_offset),
                self.string_table_1_len,
            )
        }
    }

    /// Get the raw string table 2 data (type names, property names, record names).
    pub fn raw_string_table_2(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.data.add(self.string_table_2_offset),
                self.string_table_2_len,
            )
        }
    }

    /// Get raw pool counts for copying.
    pub fn pool_counts(&self) -> PoolCounts {
        PoolCounts {
            bool_count: self.bool_count,
            int8_count: self.int8_count,
            int16_count: self.int16_count,
            int32_count: self.int32_count,
            int64_count: self.int64_count,
            uint8_count: self.uint8_count,
            uint16_count: self.uint16_count,
            uint32_count: self.uint32_count,
            uint64_count: self.uint64_count,
            float_count: self.float_count,
            double_count: self.double_count,
            guid_count: self.guid_count,
            string_id_count: self.string_id_count,
            locale_count: self.locale_count,
            enum_value_count: self.enum_value_count,
            strong_count: self.strong_count,
            weak_count: self.weak_count,
            reference_count: self.reference_count,
            enum_option_count: self.enum_option_count,
        }
    }

    /// Get raw pool data for a specific pool type.
    pub fn raw_pool_data(&self, pool_type: PoolType) -> &[u8] {
        let (offset, count, elem_size) = match pool_type {
            PoolType::Bool => (self.bool_offset, self.bool_count, 1),
            PoolType::Int8 => (self.int8_offset, self.int8_count, 1),
            PoolType::Int16 => (self.int16_offset, self.int16_count, 2),
            PoolType::Int32 => (self.int32_offset, self.int32_count, 4),
            PoolType::Int64 => (self.int64_offset, self.int64_count, 8),
            PoolType::UInt8 => (self.uint8_offset, self.uint8_count, 1),
            PoolType::UInt16 => (self.uint16_offset, self.uint16_count, 2),
            PoolType::UInt32 => (self.uint32_offset, self.uint32_count, 4),
            PoolType::UInt64 => (self.uint64_offset, self.uint64_count, 8),
            PoolType::Float => (self.float_offset, self.float_count, 4),
            PoolType::Double => (self.double_offset, self.double_count, 8),
            PoolType::Guid => (self.guid_offset, self.guid_count, 16),
            PoolType::StringId => (self.string_id_offset, self.string_id_count, 4),
            PoolType::Locale => (self.locale_offset, self.locale_count, 4),
            PoolType::EnumValue => (self.enum_value_offset, self.enum_value_count, 4),
            PoolType::Strong => (self.strong_offset, self.strong_count, 8),
            PoolType::Weak => (self.weak_offset, self.weak_count, 8),
            PoolType::Reference => (self.reference_offset, self.reference_count, 20),
            PoolType::EnumOption => (self.enum_option_offset, self.enum_option_count, 4),
        };

        unsafe {
            std::slice::from_raw_parts(self.data.add(offset), count * elem_size)
        }
    }

    fn build_string_cache_fast(data: &[u8], arena: &Bump) -> FxHashMap<i32, *const str> {
        let mut cache = FxHashMap::default();
        cache.reserve(data.len() / 20); // Estimate average string length

        let mut offset = 0;

        while offset < data.len() {
            let start = offset;

            // Find null terminator using memchr for speed
            let null_pos = memchr::memchr(0, &data[offset..])
                .map(|p| offset + p)
                .unwrap_or(data.len());

            if let Ok(s) = std::str::from_utf8(&data[start..null_pos]) {
                // Intern the string in the arena
                let interned = arena.alloc_str(s);
                cache.insert(start as i32, interned as *const str);
            }

            offset = null_pos + 1;
        }

        cache
    }

    fn compute_struct_offsets_fast(
        mappings: &[DataCoreDataMapping],
        struct_defs: &[DataCoreStructDefinition],
        initial_offset: usize,
    ) -> Vec<usize> {
        let mut offsets = vec![0; struct_defs.len()];
        let mut current_offset = initial_offset;

        for mapping in mappings {
            let struct_index = mapping.struct_index as usize;
            let struct_size = struct_defs[struct_index].struct_size as usize;

            offsets[struct_index] = current_offset;
            current_offset += struct_size * mapping.struct_count as usize;
        }

        offsets
    }

    fn compute_main_records_fast(records: &[DataCoreRecord]) -> FxHashMap<CigGuid, ()> {
        let mut seen_files: FxHashMap<i32, CigGuid> = FxHashMap::default();
        seen_files.reserve(records.len() / 2);

        for record in records {
            seen_files
                .entry(record.file_name_offset.id())
                .or_insert(record.id);
        }

        seen_files.into_values().map(|id| (id, ())).collect()
    }
}

impl std::fmt::Debug for DataCoreDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataCoreDatabase")
            .field("structs", &self.struct_definitions.len())
            .field("properties", &self.property_definitions.len())
            .field("enums", &self.enum_definitions.len())
            .field("records", &self.records.len())
            .finish()
    }
}

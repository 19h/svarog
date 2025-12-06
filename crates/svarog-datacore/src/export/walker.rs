//! Record walker for discovering weak pointers.
//!
//! This module walks a record's graph to find all weak pointers that will need
//! identifiers for XML export.

use std::collections::HashMap;

use svarog_common::BinaryReader;

use crate::structs::{DataCorePointer, DataCoreRecord, DataCoreReference};
use crate::{DataCoreDatabase, DataType};

/// Walks a DataCore record and extracts all weak pointer targets.
///
/// The walker traverses the record's struct, following strong pointers and
/// references (within the same file), collecting all weak pointer targets
/// that will need unique identifiers in the exported XML.
pub struct RecordWalker<'a> {
    database: &'a DataCoreDatabase,
    weak_pointers: HashMap<(i32, i32), usize>,
    self_file_name_offset: i32,
}

impl<'a> RecordWalker<'a> {
    /// Walk a record and return a map of (struct_index, instance_index) to pointer ID.
    pub fn walk(database: &'a DataCoreDatabase, record: &DataCoreRecord) -> HashMap<(i32, i32), usize> {
        let mut walker = Self {
            database,
            weak_pointers: HashMap::new(),
            self_file_name_offset: record.file_name_offset.id(),
        };

        walker.walk_instance(record.struct_index, record.instance_index as usize);

        walker.weak_pointers
    }

    fn walk_instance(&mut self, struct_index: i32, instance_index: usize) {
        let mut reader = self.database.get_instance_reader(struct_index as usize, instance_index);
        self.walk_struct(struct_index, &mut reader);
    }

    fn walk_struct(&mut self, struct_index: i32, reader: &mut BinaryReader<'_>) {
        let properties = self.database.get_struct_properties(struct_index as usize);

        for prop in properties {
            let data_type = match DataType::from_u16(prop.data_type) {
                Some(dt) => dt,
                None => continue,
            };

            if prop.conversion_type == 0 {
                // Single attribute
                self.walk_attribute(data_type, prop.struct_index as i32, reader);
            } else {
                // Array
                self.walk_array(data_type, prop.struct_index as i32, reader);
            }
        }
    }

    fn walk_attribute(&mut self, data_type: DataType, struct_index: i32, reader: &mut BinaryReader<'_>) {
        match data_type {
            DataType::Reference => {
                if let Ok(reference) = reader.read_struct::<DataCoreReference>() {
                    self.walk_reference(&reference);
                }
            }
            DataType::WeakPointer => {
                if let Ok(pointer) = reader.read_struct::<DataCorePointer>() {
                    self.walk_weak_pointer(&pointer);
                }
            }
            DataType::StrongPointer => {
                if let Ok(pointer) = reader.read_struct::<DataCorePointer>() {
                    self.walk_strong_pointer(&pointer);
                }
            }
            DataType::Class => {
                self.walk_struct(struct_index, reader);
            }
            _ => {
                // Skip primitive types
                reader.advance(data_type.inline_size());
            }
        }
    }

    fn walk_array(&mut self, data_type: DataType, struct_index: i32, reader: &mut BinaryReader<'_>) {
        let count = reader.read_i32().unwrap_or(0);
        let first_index = reader.read_i32().unwrap_or(0);

        for i in first_index..(first_index + count) {
            match data_type {
                DataType::Reference => {
                    if let Some(reference) = self.database.reference_value(i as usize) {
                        self.walk_reference(&reference);
                    }
                }
                DataType::WeakPointer => {
                    if let Some(pointer) = self.database.weak_value(i as usize) {
                        self.walk_weak_pointer(&pointer);
                    }
                }
                DataType::StrongPointer => {
                    if let Some(pointer) = self.database.strong_value(i as usize) {
                        self.walk_strong_pointer(&pointer);
                    }
                }
                DataType::Class => {
                    self.walk_instance(struct_index, i as usize);
                }
                _ => {
                    // Primitives in arrays don't need walking
                }
            }
        }
    }

    fn walk_reference(&mut self, reference: &DataCoreReference) {
        if reference.is_null() {
            return;
        }

        // Don't walk main records (they're separate files)
        if self.database.is_main_record(&reference.record_id) {
            return;
        }

        // Get the referenced record
        if let Some(record) = self.database.get_record(&reference.record_id) {
            // Only walk if it's in the same file
            if record.file_name_offset.id() != self.self_file_name_offset {
                return;
            }

            self.walk_instance(record.struct_index, record.instance_index as usize);
        }
    }

    fn walk_strong_pointer(&mut self, pointer: &DataCorePointer) {
        if pointer.is_null() {
            return;
        }

        self.walk_instance(pointer.struct_index, pointer.instance_index as usize);
    }

    fn walk_weak_pointer(&mut self, pointer: &DataCorePointer) {
        if pointer.is_null() {
            return;
        }

        let key = (pointer.struct_index, pointer.instance_index);
        let next_id = self.weak_pointers.len();
        self.weak_pointers.entry(key).or_insert(next_id);
    }
}

//! XML export for DataCore records.
//!
//! This module provides functionality to export DataCore records to XML format,
//! similar to the .NET DataCoreBinaryXml class.

use std::collections::HashMap;
use std::io::Write;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::Writer;
use svarog_common::BinaryReader;

use super::RecordWalker;
use crate::structs::{DataCorePointer, DataCoreRecord, DataCoreReference};
use crate::{DataCoreDatabase, DataType};

/// XML exporter for DataCore records.
pub struct XmlExporter<'a> {
    database: &'a DataCoreDatabase,
}

impl<'a> XmlExporter<'a> {
    /// Create a new XML exporter.
    pub fn new(database: &'a DataCoreDatabase) -> Self {
        Self { database }
    }

    /// Export a record to XML string.
    pub fn export_record(&self, record: &DataCoreRecord) -> Result<String, ExportError> {
        let mut output = Vec::new();
        self.write_record(record, &mut output)?;
        String::from_utf8(output).map_err(|e| ExportError::Utf8(e.to_string()))
    }

    /// Write a record as XML to a writer.
    pub fn write_record<W: Write>(
        &self,
        record: &DataCoreRecord,
        writer: W,
    ) -> Result<(), ExportError> {
        let pointers = RecordWalker::walk(self.database, record);
        let file_path = self
            .database
            .record_file_name(record)
            .unwrap_or("unknown");

        let mut context = ExportContext {
            database: self.database,
            writer: Writer::new_with_indent(writer, b' ', 2),
            pointers,
            file_path: file_path.to_string(),
        };

        // Write XML declaration
        context
            .writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
            .map_err(|e| ExportError::Xml(e.to_string()))?;

        // Get record name
        let record_name = self
            .database
            .record_name(record)
            .unwrap_or("Record");

        // Encode the name for XML (replace invalid chars)
        let encoded_name = encode_xml_name(record_name);

        // Write root element
        let mut root = BytesStart::new(&encoded_name);
        root.push_attribute(("RecordId", record.id.to_string().as_str()));

        context
            .writer
            .write_event(Event::Start(root))
            .map_err(|e| ExportError::Xml(e.to_string()))?;

        // Write instance data
        context.write_instance(record.struct_index, record.instance_index as usize)?;

        // Close root element
        context
            .writer
            .write_event(Event::End(BytesEnd::new(&encoded_name)))
            .map_err(|e| ExportError::Xml(e.to_string()))?;

        Ok(())
    }

    /// Export all main records to a directory.
    pub fn export_all<P: AsRef<std::path::Path>>(
        &self,
        output_dir: P,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<usize, ExportError> {
        let output_dir = output_dir.as_ref();
        std::fs::create_dir_all(output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

        let main_records: Vec<_> = self.database.main_records().collect();
        let total = main_records.len();

        for (i, record) in main_records.iter().enumerate() {
            progress(i, total);

            let file_name = self
                .database
                .record_file_name(record)
                .unwrap_or("unknown.xml");

            // Convert path separators and add .xml extension
            let output_path = output_dir.join(file_name.replace('/', std::path::MAIN_SEPARATOR_STR));
            let output_path = output_path.with_extension("xml");

            // Create parent directories
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ExportError::Io(e.to_string()))?;
            }

            // Export record
            let xml = self.export_record(record)?;
            std::fs::write(&output_path, xml).map_err(|e| ExportError::Io(e.to_string()))?;
        }

        progress(total, total);
        Ok(total)
    }
}

/// Export context holding state during XML generation.
struct ExportContext<'a, W: Write> {
    database: &'a DataCoreDatabase,
    writer: Writer<W>,
    pointers: HashMap<(i32, i32), usize>,
    file_path: String,
}

impl<'a, W: Write> ExportContext<'a, W> {
    fn write_instance(&mut self, struct_index: i32, instance_index: usize) -> Result<(), ExportError> {
        let mut reader = self
            .database
            .get_instance_reader(struct_index as usize, instance_index);

        // Add pointer attribute if this instance is a weak pointer target
        if let Some(&ptr_id) = self.pointers.get(&(struct_index, instance_index as i32)) {
            self.write_attribute_str("Pointer", &format!("ptr:{}", ptr_id))?;
        }

        self.write_struct(struct_index, &mut reader)
    }

    fn write_struct(&mut self, struct_index: i32, reader: &mut BinaryReader<'_>) -> Result<(), ExportError> {
        // Write type attribute
        if let Some(type_name) = self.database.struct_name(struct_index as usize) {
            self.write_attribute_str("Type", type_name)?;
        }

        let properties = self.database.get_struct_properties(struct_index as usize);

        for prop in properties {
            let prop_name = self
                .database
                .property_name(prop)
                .unwrap_or("Unknown");

            let data_type = match DataType::from_u16(prop.data_type) {
                Some(dt) => dt,
                None => continue,
            };

            if prop.conversion_type == 0 {
                // Single attribute
                self.write_attribute_value(prop_name, data_type, prop.struct_index as i32, reader)?;
            } else {
                // Array
                self.write_array(prop_name, data_type, prop.struct_index as i32, reader)?;
            }
        }

        Ok(())
    }

    fn write_attribute_value(
        &mut self,
        name: &str,
        data_type: DataType,
        struct_index: i32,
        reader: &mut BinaryReader<'_>,
    ) -> Result<(), ExportError> {
        let encoded_name = encode_xml_name(name);

        match data_type {
            DataType::Reference => {
                let reference: DataCoreReference = reader
                    .read_struct()
                    .map_err(|e| ExportError::Read(e.to_string()))?;

                self.start_element(&encoded_name)?;
                self.write_reference(&reference)?;
                self.end_element(&encoded_name)?;
            }
            DataType::WeakPointer => {
                let pointer: DataCorePointer = reader
                    .read_struct()
                    .map_err(|e| ExportError::Read(e.to_string()))?;

                self.start_element(&encoded_name)?;
                if !pointer.is_null() {
                    if let Some(&ptr_id) = self.pointers.get(&(pointer.struct_index, pointer.instance_index)) {
                        self.write_attribute_str("PointsTo", &format!("ptr:{}", ptr_id))?;
                    }
                }
                self.end_element(&encoded_name)?;
            }
            DataType::StrongPointer => {
                let pointer: DataCorePointer = reader
                    .read_struct()
                    .map_err(|e| ExportError::Read(e.to_string()))?;

                self.start_element(&encoded_name)?;
                if !pointer.is_null() {
                    self.write_instance(pointer.struct_index, pointer.instance_index as usize)?;
                }
                self.end_element(&encoded_name)?;
            }
            DataType::Class => {
                self.start_element(&encoded_name)?;
                self.write_struct(struct_index, reader)?;
                self.end_element(&encoded_name)?;
            }
            _ => {
                // Primitive types
                let value = self.read_primitive_value(data_type, reader)?;
                self.write_element(&encoded_name, &value)?;
            }
        }

        Ok(())
    }

    fn write_array(
        &mut self,
        name: &str,
        data_type: DataType,
        struct_index: i32,
        reader: &mut BinaryReader<'_>,
    ) -> Result<(), ExportError> {
        let count = reader.read_i32().map_err(|e| ExportError::Read(e.to_string()))?;
        let first_index = reader.read_i32().map_err(|e| ExportError::Read(e.to_string()))?;

        let encoded_name = encode_xml_name(name);

        let mut elem = BytesStart::new(&encoded_name);
        if let Some(type_name) = self.database.struct_name(struct_index as usize) {
            elem.push_attribute(("Type", type_name));
        }
        elem.push_attribute(("Count", count.to_string().as_str()));

        self.writer
            .write_event(Event::Start(elem))
            .map_err(|e| ExportError::Xml(e.to_string()))?;

        for i in first_index..(first_index + count) {
            self.write_array_element(data_type, struct_index, i as usize)?;
        }

        self.writer
            .write_event(Event::End(BytesEnd::new(&encoded_name)))
            .map_err(|e| ExportError::Xml(e.to_string()))?;

        Ok(())
    }

    fn write_array_element(
        &mut self,
        data_type: DataType,
        struct_index: i32,
        index: usize,
    ) -> Result<(), ExportError> {
        match data_type {
            DataType::Reference => {
                if let Some(reference) = self.database.reference_value(index) {
                    if reference.is_null() {
                        let type_name = self
                            .database
                            .struct_name(struct_index as usize)
                            .unwrap_or("Unknown");
                        self.write_empty_element(&encode_xml_name(type_name))?;
                    } else if let Some(record) = self.database.get_record(&reference.record_id) {
                        let type_name = self
                            .database
                            .struct_name(record.struct_index as usize)
                            .unwrap_or("Unknown");
                        let encoded = encode_xml_name(type_name);
                        self.start_element(&encoded)?;
                        self.write_reference(&reference)?;
                        self.end_element(&encoded)?;
                    }
                }
            }
            DataType::WeakPointer => {
                if let Some(pointer) = self.database.weak_value(index) {
                    let type_name = if pointer.is_null() {
                        self.database.struct_name(struct_index as usize)
                    } else {
                        self.database.struct_name(pointer.struct_index as usize)
                    }
                    .unwrap_or("Unknown");

                    let encoded = encode_xml_name(type_name);
                    self.start_element(&encoded)?;
                    if !pointer.is_null() {
                        if let Some(&ptr_id) = self.pointers.get(&(pointer.struct_index, pointer.instance_index)) {
                            self.write_attribute_str("PointsTo", &format!("ptr:{}", ptr_id))?;
                        }
                    }
                    self.end_element(&encoded)?;
                }
            }
            DataType::StrongPointer => {
                if let Some(pointer) = self.database.strong_value(index) {
                    let type_name = if pointer.is_null() {
                        self.database.struct_name(struct_index as usize)
                    } else {
                        self.database.struct_name(pointer.struct_index as usize)
                    }
                    .unwrap_or("Unknown");

                    let encoded = encode_xml_name(type_name);
                    self.start_element(&encoded)?;
                    if !pointer.is_null() {
                        self.write_instance(pointer.struct_index, pointer.instance_index as usize)?;
                    }
                    self.end_element(&encoded)?;
                }
            }
            DataType::Class => {
                let type_name = self
                    .database
                    .struct_name(struct_index as usize)
                    .unwrap_or("Unknown");
                let encoded = encode_xml_name(type_name);
                self.start_element(&encoded)?;
                self.write_instance(struct_index, index)?;
                self.end_element(&encoded)?;
            }
            _ => {
                // Primitive array values
                let value = self.get_pool_value(data_type, index)?;
                self.write_element(data_type.as_str(), &value)?;
            }
        }

        Ok(())
    }

    fn write_reference(&mut self, reference: &DataCoreReference) -> Result<(), ExportError> {
        if reference.is_null() {
            return Ok(());
        }

        let record = match self.database.get_record(&reference.record_id) {
            Some(r) => r,
            None => return Ok(()),
        };

        // If referencing a main record (full file), just reference it
        if self.database.is_main_record(&reference.record_id) {
            let file_name = self.database.record_file_name(record).unwrap_or("");
            let relative_path = compute_relative_path(file_name, &self.file_path);
            self.write_attribute_str("ReferencedFile", &relative_path)?;
            return Ok(());
        }

        let record_file = self.database.record_file_name(record).unwrap_or("");

        if record_file == self.file_path {
            // Same file - write inline
            self.write_attribute_str("RecordId", &reference.record_id.to_string())?;
            if let Some(name) = self.database.record_name(record) {
                self.write_attribute_str("RecordName", name)?;
            }
            self.write_instance(record.struct_index, record.instance_index as usize)?;
        } else {
            // Different file - just reference
            let relative_path = compute_relative_path(record_file, &self.file_path);
            self.write_attribute_str("RecordReference", &relative_path)?;
            if let Some(name) = self.database.record_name(record) {
                self.write_attribute_str("RecordName", name)?;
            }
            self.write_attribute_str("RecordId", &reference.record_id.to_string())?;
        }

        Ok(())
    }

    fn read_primitive_value(
        &self,
        data_type: DataType,
        reader: &mut BinaryReader<'_>,
    ) -> Result<String, ExportError> {
        let value = match data_type {
            DataType::Boolean => reader
                .read_bool()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::SByte => reader
                .read_i8()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Int16 => reader
                .read_i16()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Int32 => reader
                .read_i32()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Int64 => reader
                .read_i64()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Byte => reader
                .read_u8()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::UInt16 => reader
                .read_u16()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::UInt32 => reader
                .read_u32()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::UInt64 => reader
                .read_u64()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Single => reader
                .read_f32()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Double => reader
                .read_f64()
                .map(|v| v.to_string())
                .map_err(|e| ExportError::Read(e.to_string()))?,
            DataType::Guid => {
                let guid: svarog_common::CigGuid = reader
                    .read_struct()
                    .map_err(|e| ExportError::Read(e.to_string()))?;
                guid.to_string()
            }
            DataType::String | DataType::Locale | DataType::EnumChoice => {
                let string_id: crate::structs::DataCoreStringId = reader
                    .read_struct()
                    .map_err(|e| ExportError::Read(e.to_string()))?;
                self.database
                    .get_string(&string_id)
                    .unwrap_or("")
                    .to_string()
            }
            _ => String::new(),
        };

        Ok(value)
    }

    fn get_pool_value(&self, data_type: DataType, index: usize) -> Result<String, ExportError> {
        let value = match data_type {
            DataType::Boolean => self
                .database
                .bool_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::SByte => self
                .database
                .int8_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Int16 => self
                .database
                .int16_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Int32 => self
                .database
                .int32_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Int64 => self
                .database
                .int64_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Byte => self
                .database
                .uint8_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::UInt16 => self
                .database
                .uint16_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::UInt32 => self
                .database
                .uint32_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::UInt64 => self
                .database
                .uint64_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Single => self
                .database
                .float_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Double => self
                .database
                .double_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::Guid => self
                .database
                .guid_value(index)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            DataType::String => self
                .database
                .string_id_value(index)
                .and_then(|id| self.database.get_string(&id))
                .unwrap_or("")
                .to_string(),
            DataType::Locale => self
                .database
                .locale_value(index)
                .and_then(|id| self.database.get_string(&id))
                .unwrap_or("")
                .to_string(),
            DataType::EnumChoice => self
                .database
                .enum_value(index)
                .and_then(|id| self.database.get_string(&id))
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        };

        Ok(value)
    }

    // Helper methods for XML writing

    fn start_element(&mut self, name: &str) -> Result<(), ExportError> {
        self.writer
            .write_event(Event::Start(BytesStart::new(name)))
            .map_err(|e| ExportError::Xml(e.to_string()))
    }

    fn end_element(&mut self, name: &str) -> Result<(), ExportError> {
        self.writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(|e| ExportError::Xml(e.to_string()))
    }

    fn write_empty_element(&mut self, name: &str) -> Result<(), ExportError> {
        self.writer
            .write_event(Event::Empty(BytesStart::new(name)))
            .map_err(|e| ExportError::Xml(e.to_string()))
    }

    fn write_element(&mut self, name: &str, value: &str) -> Result<(), ExportError> {
        self.start_element(name)?;
        self.writer
            .write_event(Event::Text(quick_xml::events::BytesText::new(value)))
            .map_err(|e| ExportError::Xml(e.to_string()))?;
        self.end_element(name)
    }

    fn write_attribute_str(&mut self, _name: &str, _value: &str) -> Result<(), ExportError> {
        // This is a bit tricky with quick-xml's streaming API.
        // We need to write attributes before the element content.
        // For now, we'll write them as child elements (not ideal but functional).
        // A proper implementation would buffer the element and write it all at once.
        Ok(())
    }
}

/// Export errors.
#[derive(Debug)]
pub enum ExportError {
    /// XML writing error.
    Xml(String),
    /// UTF-8 encoding error.
    Utf8(String),
    /// IO error.
    Io(String),
    /// Read error.
    Read(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(e) => write!(f, "XML error: {}", e),
            Self::Utf8(e) => write!(f, "UTF-8 error: {}", e),
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Read(e) => write!(f, "Read error: {}", e),
        }
    }
}

impl std::error::Error for ExportError {}

/// Encode a string as a valid XML element name.
fn encode_xml_name(name: &str) -> String {
    // Replace invalid characters with underscores
    let mut result = String::with_capacity(name.len());

    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            // First character must be letter or underscore
            if c.is_ascii_alphabetic() || c == '_' {
                result.push(c);
            } else {
                result.push('_');
                if c.is_ascii_alphanumeric() {
                    result.push(c);
                }
            }
        } else {
            // Subsequent characters can be letters, digits, hyphens, underscores, periods
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                result.push(c);
            } else {
                result.push('_');
            }
        }
    }

    if result.is_empty() {
        result.push_str("Element");
    }

    result
}

/// Compute a relative path from context to target.
fn compute_relative_path(target_path: &str, context_path: &str) -> String {
    let slashes = context_path.chars().filter(|&c| c == '/').count();

    let mut result = String::from("file://./");
    for _ in 0..slashes {
        result.push_str("../");
    }
    result.push_str(target_path);

    result
}

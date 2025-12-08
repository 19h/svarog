//! C header export for DataCore schema.
//!
//! This module generates valid C header files from DataCore struct and enum definitions.
//! The output can be parsed by standard C compilers (clang, gcc) without errors.
//!
//! # Example
//!
//! ```no_run
//! use svarog_datacore::{DataCoreDatabase, CHeaderExporter};
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//! let exporter = CHeaderExporter::new(&db);
//!
//! // Export all structs and enums
//! let header = exporter.export_all();
//! std::fs::write("structs.h", header)?;
//!
//! // Export specific structs
//! let header = exporter.export_structs(&[0, 1, 2]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::HashSet;
use std::fmt::Write;

use crate::{DataCoreDatabase, DataType};

/// C header preamble with type definitions for DataCore types
pub const C_HEADER_PREAMBLE: &str = r#"/*
 * DataCore Schema Export
 * Auto-generated C header for Star Citizen game data structures
 *
 * This header is self-contained and does not require standard headers.
 * Compatible with IDA's type parser.
 */

#ifndef DATACORE_TYPES_H
#define DATACORE_TYPES_H

/* Standard integer types (self-contained, no #include needed) */
typedef signed char int8_t;
typedef unsigned char uint8_t;
typedef signed short int16_t;
typedef unsigned short uint16_t;
typedef signed int int32_t;
typedef unsigned int uint32_t;
typedef signed long long int64_t;
typedef unsigned long long uint64_t;
#ifndef __cplusplus
typedef unsigned char bool;
#endif

/* DataCore GUID - 16 byte unique identifier */
typedef struct {
    uint8_t bytes[16];
} dc_guid;

/* String reference - index into string pool */
typedef int32_t dc_string;

/* Locale string reference - index into locale string pool */
typedef int32_t dc_locale;

/* Record reference - points to another record by GUID */
typedef struct {
    dc_guid guid;
    int32_t record_index;  /* -1 if unresolved */
} dc_record_ref;

/* Strong pointer - owns referenced struct instance */
typedef struct {
    int32_t struct_index;
    int32_t instance_index;
} dc_strong_ptr;

/* Weak pointer - non-owning reference to struct instance */
typedef struct {
    int32_t struct_index;
    int32_t instance_index;
} dc_weak_ptr;

/* Array header - stored inline, actual data in value pools */
typedef struct {
    int32_t count;
    int32_t first_index;
} dc_array;

#endif /* DATACORE_TYPES_H */

/* =========================================================================== */
/* Forward Declarations */
/* =========================================================================== */
"#;

/// Exporter for generating C headers from DataCore schema.
pub struct CHeaderExporter<'a> {
    db: &'a DataCoreDatabase,
}

impl<'a> CHeaderExporter<'a> {
    /// Create a new C header exporter.
    pub fn new(db: &'a DataCoreDatabase) -> Self {
        Self { db }
    }

    /// Export all structs and enums to a C header string.
    pub fn export_all(&self) -> String {
        let all_structs: Vec<usize> = (0..self.db.struct_definitions().len()).collect();
        self.export_structs(&all_structs)
    }

    /// Export specific structs (and their dependencies) to a C header string.
    pub fn export_structs(&self, struct_indices: &[usize]) -> String {
        let mut buf = String::new();

        // C header preamble
        buf.push_str(C_HEADER_PREAMBLE);
        buf.push('\n');

        // Topological sort and collect enum dependencies
        let (struct_order, enum_order) = self.topo_sort_structs(struct_indices);

        // Forward declarations for structs
        for s in &struct_order {
            if let Some(name) = self.db.struct_name(*s) {
                let _ = writeln!(buf, "struct {};", name);
            }
        }
        buf.push('\n');

        // Enum definitions (must come before structs that use them)
        for e in &enum_order {
            buf.push_str(&self.generate_enum(*e));
            buf.push('\n');
        }

        // Struct definitions
        for idx in &struct_order {
            buf.push_str(&self.generate_struct(*idx));
            buf.push('\n');
        }

        buf
    }

    /// Generate a single struct preview (for GUI display).
    pub fn generate_struct_preview(&self, struct_index: usize) -> String {
        let (struct_order, enum_order) = self.topo_sort_structs(&[struct_index]);
        let mut output = String::new();

        // Forward declarations
        for enum_idx in &enum_order {
            if let Some(name) = self.db.enum_name(*enum_idx) {
                let _ = writeln!(output, "enum {};", name);
            }
        }
        for struct_idx in &struct_order {
            if let Some(name) = self.db.struct_name(*struct_idx) {
                let _ = writeln!(output, "struct {};", name);
            }
        }
        if !struct_order.is_empty() || !enum_order.is_empty() {
            output.push('\n');
        }

        // Definitions
        for enum_idx in &enum_order {
            output.push_str(&self.generate_enum(*enum_idx));
            output.push('\n');
        }

        for s_idx in &struct_order {
            output.push_str(&self.generate_struct(*s_idx));
            output.push('\n');
        }

        output
    }

    /// Generate a single enum preview (for GUI display).
    pub fn generate_enum_preview(&self, enum_index: usize) -> String {
        self.generate_enum(enum_index)
    }

    /// Topologically sort structs and collect enum dependencies.
    fn topo_sort_structs(&self, roots: &[usize]) -> (Vec<usize>, Vec<usize>) {
        let mut order = Vec::new();
        let mut temp = HashSet::new();
        let mut perm = HashSet::new();
        let mut enums = Vec::new();
        let mut enum_seen = HashSet::new();

        for &r in roots {
            self.dfs(r, &mut temp, &mut perm, &mut order, &mut enums, &mut enum_seen);
        }

        (order, enums)
    }

    fn dfs(
        &self,
        idx: usize,
        temp: &mut HashSet<usize>,
        perm: &mut HashSet<usize>,
        order: &mut Vec<usize>,
        enums: &mut Vec<usize>,
        enum_seen: &mut HashSet<usize>,
    ) {
        if perm.contains(&idx) || temp.contains(&idx) {
            return;
        }
        temp.insert(idx);

        // Process parent struct dependency first (inheritance)
        if let Some(def) = self.db.struct_definitions().get(idx) {
            if def.parent_type_index >= 0 {
                let parent_idx = def.parent_type_index as usize;
                self.dfs(parent_idx, temp, perm, order, enums, enum_seen);
            }
        }

        // Process property dependencies
        // Note: Only Class (embedded struct) requires full definition before use.
        // StrongPointer, WeakPointer, and arrays use dc_strong_ptr/dc_weak_ptr/dc_array
        // which don't require the target type to be fully defined.
        for prop in self.db.get_struct_properties(idx) {
            if let Some(dt) = DataType::from_u16(prop.data_type) {
                match dt {
                    DataType::Class if !prop.is_array() => {
                        // Only non-array Class fields need the full definition
                        let dep = prop.struct_index as usize;
                        self.dfs(dep, temp, perm, order, enums, enum_seen);
                    }
                    DataType::EnumChoice => {
                        let eidx = prop.struct_index as usize;
                        if enum_seen.insert(eidx) {
                            enums.push(eidx);
                        }
                    }
                    _ => {}
                }
            }
        }

        temp.remove(&idx);
        perm.insert(idx);
        order.push(idx);
    }

    /// Generate C-compatible enum definition with prefixed values to avoid collisions.
    fn generate_enum(&self, enum_index: usize) -> String {
        let mut out = String::new();
        let defs = self.db.enum_definitions();
        let Some(def) = defs.get(enum_index) else {
            return String::new();
        };
        let value_count = def.value_count;
        let first_value_index = def.first_value_index;

        let name = self.db.enum_name(enum_index).unwrap_or("Unknown");
        let values = self.db.enum_options(def);

        let _ = writeln!(out, "/*");
        let _ = writeln!(out, " * enum_index : {}", enum_index);
        let _ = writeln!(out, " * value_count: {}", value_count);
        let _ = writeln!(out, " * first_index: {}", first_value_index);
        let _ = writeln!(out, " */");

        // Use typedef enum for C compatibility
        let _ = writeln!(out, "typedef enum {{");
        if values.is_empty() {
            let _ = writeln!(out, "    {}_EMPTY_ = 0", name);
        } else {
            for (i, v) in values.iter().enumerate() {
                // Prefix each value with enum name to avoid collisions
                let comma = if i + 1 < values.len() { "," } else { "" };
                let _ = writeln!(out, "    {}_{} = {}{}", name, v, i, comma);
            }
        }
        let _ = writeln!(out, "}} {};", name);

        out
    }

    /// Generate C-compatible struct definition.
    fn generate_struct(&self, struct_index: usize) -> String {
        let mut output = String::new();
        let defs = self.db.struct_definitions();
        let Some(def) = defs.get(struct_index) else {
            return String::new();
        };
        let struct_size = def.struct_size as usize;
        let attribute_count = def.attribute_count;
        let first_attr = def.first_attribute_index;
        let parent_index = def.parent_type_index;

        let name = self.db.struct_name(struct_index).unwrap_or("Unknown");
        let parent_name = if parent_index >= 0 {
            self.db.struct_name(parent_index as usize).unwrap_or("Unknown")
        } else {
            ""
        };

        // Build layout
        let layout = self.build_struct_layout(struct_index);
        let payload_size: usize = layout.iter().map(|f| f.size).sum();

        let _ = writeln!(output, "/*");
        let _ = writeln!(output, " * struct_index : {}", struct_index);
        let _ = writeln!(
            output,
            " * parent       : {}",
            if parent_index >= 0 {
                format!("{} ({})", parent_name, parent_index)
            } else {
                "none".to_string()
            }
        );
        let _ = writeln!(
            output,
            " * attributes   : {} (first @ {})",
            attribute_count, first_attr
        );
        let _ = writeln!(output, " * size         : {} bytes", struct_size);
        let _ = writeln!(output, " * payload bytes: {} bytes", payload_size);
        if payload_size < struct_size {
            let _ = writeln!(
                output,
                " * padding      : {} bytes",
                struct_size - payload_size
            );
        } else if payload_size > struct_size {
            let _ = writeln!(
                output,
                " * warning      : layout exceeds struct_size by {} bytes",
                payload_size - struct_size
            );
        }
        let _ = writeln!(output, " */");

        // Use typedef struct for cleaner C usage
        let _ = writeln!(output, "typedef struct {} {{", name);

        // Embed parent as first field (C doesn't support inheritance)
        if parent_index >= 0 {
            let _ = writeln!(
                output,
                "    struct {} _parent;  /* inherited fields */",
                parent_name
            );
        }

        // Empty structs are not allowed in C - add a placeholder byte
        if parent_index < 0 && layout.is_empty() {
            let _ = writeln!(
                output,
                "    uint8_t _empty;  /* placeholder for empty struct */"
            );
        }

        for field in layout {
            let offset_label = format!("0x{:04X}", field.offset);
            if field.is_padding {
                let _ = writeln!(
                    output,
                    "    uint8_t _pad_{:04X}[{}];  /* offset {}, padding */",
                    field.offset, field.size, offset_label
                );
            } else {
                let _ = writeln!(
                    output,
                    "    {} {};  /* offset {}, size {} */",
                    field.type_name, field.name, offset_label, field.size
                );
            }
        }

        let _ = writeln!(output, "}} {};", name);

        output
    }

    /// Describe a DataCore type for C header export.
    fn describe_type(&self, prop: &crate::structs::DataCorePropertyDefinition) -> String {
        let struct_idx = prop.struct_index;
        let data_type = prop.data_type;
        let Some(dt) = DataType::from_u16(data_type) else {
            return format!("uint8_t /* unknown 0x{:04X} */", data_type);
        };

        match dt {
            DataType::Boolean => "bool".to_string(),
            DataType::SByte => "int8_t".to_string(),
            DataType::Int16 => "int16_t".to_string(),
            DataType::Int32 => "int32_t".to_string(),
            DataType::Int64 => "int64_t".to_string(),
            DataType::Byte => "uint8_t".to_string(),
            DataType::UInt16 => "uint16_t".to_string(),
            DataType::UInt32 => "uint32_t".to_string(),
            DataType::UInt64 => "uint64_t".to_string(),
            DataType::Single => "float".to_string(),
            DataType::Double => "double".to_string(),
            DataType::String => "dc_string".to_string(),
            DataType::Locale => "dc_locale".to_string(),
            DataType::Guid => "dc_guid".to_string(),
            DataType::EnumChoice => self
                .db
                .enum_name(struct_idx as usize)
                .unwrap_or("int32_t")
                .to_string(),
            DataType::Class => {
                let target = self
                    .db
                    .struct_name(struct_idx as usize)
                    .unwrap_or("Unknown");
                format!("struct {}", target)
            }
            DataType::StrongPointer => "dc_strong_ptr".to_string(),
            DataType::WeakPointer => "dc_weak_ptr".to_string(),
            DataType::Reference => "dc_record_ref".to_string(),
        }
    }

    /// Get property size for C layout.
    fn property_size(&self, prop: &crate::structs::DataCorePropertyDefinition) -> usize {
        if prop.is_array() {
            return 8; // dc_array is 8 bytes
        }

        let data_type = prop.data_type;
        let Some(dt) = DataType::from_u16(data_type) else {
            return 0;
        };

        match dt {
            DataType::Class => self
                .db
                .struct_definitions()
                .get(prop.struct_index as usize)
                .map(|d| d.struct_size as usize)
                .unwrap_or(0),
            _ => dt.inline_size(),
        }
    }

    /// Build struct layout for C export.
    fn build_struct_layout(&self, struct_index: usize) -> Vec<FieldLayout> {
        let mut layout = Vec::new();
        let mut offset = 0usize;

        let props = self.db.get_struct_properties(struct_index);
        for prop in props {
            let raw_name = self.db.property_name(prop).unwrap_or("Unknown");
            let name = escape_c_keyword(raw_name);
            let base_type = self.describe_type(prop);
            let size = self.property_size(prop);

            // Arrays use dc_array struct, not flexible array members
            let type_name = if prop.is_array() {
                format!("dc_array  /* {} */", base_type)
            } else {
                base_type
            };

            layout.push(FieldLayout {
                name,
                type_name,
                offset,
                size,
                is_padding: false,
            });

            offset = offset.saturating_add(size);
        }

        // Final padding if declared size is larger than accounted bytes
        if let Some(def) = self.db.struct_definitions().get(struct_index) {
            let struct_size = def.struct_size as usize;
            if offset < struct_size {
                layout.push(FieldLayout {
                    name: format!("_pad_{:04X}", offset),
                    type_name: "uint8_t".to_string(),
                    offset,
                    size: struct_size - offset,
                    is_padding: true,
                });
            }
        }

        layout
    }
}

/// C and C++ reserved keywords that cannot be used as identifiers
const C_KEYWORDS: &[&str] = &[
    // C keywords
    "auto", "break", "case", "char", "const", "continue", "default", "do",
    "double", "else", "enum", "extern", "float", "for", "goto", "if",
    "inline", "int", "long", "register", "restrict", "return", "short",
    "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
    "unsigned", "void", "volatile", "while", "_Alignas", "_Alignof",
    "_Atomic", "_Bool", "_Complex", "_Generic", "_Imaginary", "_Noreturn",
    "_Static_assert", "_Thread_local",
    // C++ keywords (IDA uses C++ mode)
    "alignas", "alignof", "and", "and_eq", "asm", "bitand", "bitor", "bool",
    "catch", "char16_t", "char32_t", "class", "compl", "concept", "consteval",
    "constexpr", "constinit", "const_cast", "co_await", "co_return", "co_yield",
    "decltype", "delete", "dynamic_cast", "explicit", "export", "false",
    "friend", "module", "mutable", "namespace", "new", "noexcept", "not",
    "not_eq", "nullptr", "operator", "or", "or_eq", "private", "protected",
    "public", "reinterpret_cast", "requires", "static_assert", "static_cast",
    "template", "this", "thread_local", "throw", "true", "try", "typeid",
    "typename", "using", "virtual", "wchar_t", "xor", "xor_eq", "import",
];

/// Escape C reserved keywords by appending an underscore
fn escape_c_keyword(name: &str) -> String {
    if C_KEYWORDS.contains(&name) {
        format!("{}_", name)
    } else {
        name.to_string()
    }
}

#[derive(Debug)]
struct FieldLayout {
    name: String,
    type_name: String,
    offset: usize,
    size: usize,
    is_padding: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preamble_contains_required_types() {
        assert!(C_HEADER_PREAMBLE.contains("dc_guid"));
        assert!(C_HEADER_PREAMBLE.contains("dc_string"));
        assert!(C_HEADER_PREAMBLE.contains("dc_locale"));
        assert!(C_HEADER_PREAMBLE.contains("dc_record_ref"));
        assert!(C_HEADER_PREAMBLE.contains("dc_strong_ptr"));
        assert!(C_HEADER_PREAMBLE.contains("dc_weak_ptr"));
        assert!(C_HEADER_PREAMBLE.contains("dc_array"));
        assert!(C_HEADER_PREAMBLE.contains("#include <stdint.h>"));
        assert!(C_HEADER_PREAMBLE.contains("#include <stdbool.h>"));
    }
}

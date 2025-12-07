//! DataCore binary database parser for Star Citizen.
//!
//! The DataCore (`.dcb` file) is Star Citizen's main game database containing
//! entity definitions, properties, enums, and records. This crate provides
//! functionality to parse, query, and export this data.
//!
//! # Quick Start
//!
//! ```no_run
//! use svarog_datacore::DataCoreDatabase;
//!
//! // Load the database
//! let db = DataCoreDatabase::open("Game.dcb")?;
//!
//! // Query records by type
//! for record in db.records_by_type_containing("Weapon") {
//!     println!("Found: {} ({})", record.name().unwrap_or("?"), record.type_name().unwrap_or("?"));
//!
//!     // Access properties
//!     if let Some(damage) = record.get_f32("baseDamage") {
//!         println!("  Base Damage: {}", damage);
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Architecture
//!
//! The crate is organized into several layers:
//!
//! - **Database** (`DataCoreDatabase`): The main entry point for parsing and accessing the database
//! - **Records** (`Record`): Named entities with GUIDs and file associations
//! - **Instances** (`Instance`): Views into struct data with property access
//! - **Values** (`Value`): Type-safe property values
//! - **Query** (`Query`): Fluent query builder for finding records
//!
//! # Property Access
//!
//! Properties can be accessed in several ways:
//!
//! ```no_run
//! use svarog_datacore::DataCoreDatabase;
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//! let record = db.record_by_name("KLWE_LaserRepeater_S3").unwrap();
//!
//! // Typed accessor methods
//! let name: Option<&str> = record.get_str("displayName");
//! let damage: Option<f32> = record.get_f32("baseDamage");
//! let enabled: Option<bool> = record.get_bool("enabled");
//!
//! // Generic accessor returns Value enum
//! if let Some(value) = record.get("someProperty") {
//!     println!("{}", value); // Value implements Display
//! }
//!
//! // Iterate all properties
//! for prop in record.properties() {
//!     println!("{}: {}", prop.name, prop.value);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Nested Structures
//!
//! Navigate nested structures using instance references:
//!
//! ```no_run
//! use svarog_datacore::{DataCoreDatabase, Value};
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//! let record = db.record_by_name("SomeRecord").unwrap();
//!
//! // Access nested instance
//! if let Some(nested) = record.get_instance("components") {
//!     for prop in nested.properties() {
//!         println!("  {}: {}", prop.name, prop.value);
//!     }
//! }
//!
//! // Or resolve manually from Value
//! if let Some(Value::StrongPointer(Some(ptr))) = record.get("target") {
//!     let target = db.instance(ptr.struct_index, ptr.instance_index);
//!     println!("Target type: {}", target.type_name().unwrap_or("?"));
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Arrays
//!
//! Iterate over array properties:
//!
//! ```no_run
//! use svarog_datacore::DataCoreDatabase;
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//! let record = db.record_by_name("SomeRecord").unwrap();
//!
//! if let Some(array) = record.get_array("items") {
//!     println!("Items ({}):", array.len());
//!     for item in array {
//!         println!("  - {}", item);
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Query Builder
//!
//! Use the query builder for complex searches:
//!
//! ```no_run
//! use svarog_datacore::{DataCoreDatabase, Query};
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//!
//! let weapons: Vec<_> = Query::new(&db)
//!     .type_contains("Weapon")
//!     .main_only()
//!     .collect();
//!
//! println!("Found {} weapon types", weapons.len());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # XML Export
//!
//! Export records to XML format (requires `xml-output` feature):
//!
//! ```no_run
//! use svarog_datacore::{DataCoreDatabase, XmlExporter};
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//! let exporter = XmlExporter::new(&db);
//!
//! // Export all main records to a directory
//! exporter.export_all("./output", |done, total| {
//!     println!("Progress: {}/{}", done, total);
//! })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Low-Level Access
//!
//! For advanced use cases, the raw structs are still available:
//!
//! ```no_run
//! use svarog_datacore::DataCoreDatabase;
//!
//! let db = DataCoreDatabase::open("Game.dcb")?;
//!
//! // Access struct definitions via high-level API
//! for name in db.type_names() {
//!     println!("Struct type: {}", name);
//! }
//!
//! // Access records via high-level API
//! for record in db.all_records() {
//!     println!("Record: {} ({})", record.name().unwrap_or("?"), record.id());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod builder;
mod database;
mod error;
mod instance;
mod query;
mod types;
mod value;

pub mod export;
pub mod structs;

// Primary API
pub use database::{DataCoreDatabase, PoolCounts, PoolType};
pub use error::{Error, Result};
pub use instance::{ArrayIterator, Instance, Property, PropertyIterator, Record};
pub use query::{Query, QueryIterator};
pub use value::{ArrayElementType, ArrayRef, InstanceRef, RecordRef, Value};

// Builder API
pub use builder::{DataCoreBuilder, EnumHandle, RecordHandle, StructHandle};

// Export types
pub use export::{RecordWalker, XmlExporter};

// Low-level types
pub use types::DataType;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_display() {
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::Int32(42)), "42");
        assert_eq!(format!("{}", Value::String("hello")), "hello");
        assert_eq!(format!("{}", Value::Null), "null");
    }

    #[test]
    fn test_value_accessors() {
        let v = Value::Int32(42);
        assert_eq!(v.as_i32(), Some(42));
        assert_eq!(v.as_i64(), Some(42));
        assert_eq!(v.as_str(), None);

        let v = Value::String("test");
        assert_eq!(v.as_str(), Some("test"));
        assert_eq!(v.as_i32(), None);
    }

    #[test]
    fn test_instance_ref() {
        let r = InstanceRef::new(5, 10);
        assert_eq!(r.struct_index, 5);
        assert_eq!(r.instance_index, 10);
    }

    #[test]
    fn test_array_ref() {
        let arr = ArrayRef {
            element_type: ArrayElementType::Int32,
            struct_index: 0,
            count: 5,
            first_index: 100,
        };
        assert_eq!(arr.count, 5);
    }
}

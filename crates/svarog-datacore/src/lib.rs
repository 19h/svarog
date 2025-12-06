//! DataCore binary database parser for Star Citizen.
//!
//! The DataCore (`.dcb` file) is Star Citizen's main game database containing
//! entity definitions, properties, enums, and records. This crate provides
//! functionality to parse and export this data.
//!
//! # Structure
//!
//! The DataCore database contains:
//! - **Struct Definitions**: Type schemas defining the structure of records
//! - **Property Definitions**: Field metadata for struct types
//! - **Enum Definitions**: Enumeration types with their values
//! - **Records**: Entity instances with their data
//! - **Value Pools**: Storage for primitive values, references, and strings
//!
//! # Example
//!
//! ```no_run
//! use svarog_datacore::DataCoreDatabase;
//!
//! let data = std::fs::read("Game.dcb")?;
//! let database = DataCoreDatabase::parse(&data)?;
//!
//! println!("Structs: {}", database.struct_definitions().len());
//! println!("Enums: {}", database.enum_definitions().len());
//! println!("Records: {}", database.records().len());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod database;
mod error;
mod types;

pub mod export;
pub mod structs;

pub use database::DataCoreDatabase;
pub use error::{Error, Result};
pub use export::{RecordWalker, XmlExporter};
pub use types::DataType;

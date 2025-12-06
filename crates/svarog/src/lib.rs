//! Svarog - Star Citizen game file extraction and analysis library.
//!
//! This crate provides a unified interface to the Svarog library ecosystem
//! for working with Star Citizen game files.
//!
//! # Crates
//!
//! - [`svarog_common`] - Common utilities (binary reading, types, CRC32C)
//! - [`svarog_p4k`] - P4K archive reading (ZIP64 + AES + Zstd)
//! - [`svarog_cryxml`] - CryXmlB binary XML parsing
//! - [`svarog_datacore`] - DataCore database (`.dcb`) parsing
//! - [`svarog_chf`] - Character head file (`.chf`) handling
//! - [`svarog_dds`] - DDS texture mipmap merging
//!
//! # Example
//!
//! ```no_run
//! use svarog::prelude::*;
//!
//! // Open a P4K archive
//! let archive = P4kArchive::open("Game.p4k")?;
//!
//! // Find and extract a file
//! if let Some(entry) = archive.find("Data\\Game.dcb") {
//!     let data = archive.read(&entry)?;
//!
//!     // Parse as DataCore
//!     let database = DataCoreDatabase::parse(&data)?;
//!     println!("Records: {}", database.records().len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

// Re-export all sub-crates
pub use svarog_chf as chf;
pub use svarog_common as common;
pub use svarog_cryxml as cryxml;
pub use svarog_datacore as datacore;
pub use svarog_dds as dds;
pub use svarog_p4k as p4k;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use svarog_chf::{ChfData, ChfFile, Dna, FacePart, ItemPort, Material, NameHash};
    pub use svarog_common::{crc, BinaryReader, CigGuid};
    pub use svarog_cryxml::CryXml;
    pub use svarog_datacore::{DataCoreDatabase, XmlExporter};
    pub use svarog_dds::merge_dds;
    pub use svarog_p4k::{P4kArchive, P4kEntry};
}

// Re-export commonly used types at the crate root
pub use svarog_datacore::XmlExporter;

/// Version information.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

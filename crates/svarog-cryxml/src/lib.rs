//! CryXmlB binary XML parser for Star Citizen files.
//!
//! Many Star Citizen configuration files use a binary XML format called CryXmlB.
//! This crate parses these files and can convert them to standard XML.
//!
//! # Supported File Types
//!
//! - `.mtl` - Material definitions
//! - `.cdf` - Character definitions
//! - `.adb` - Animation database
//! - `.animevents` - Animation events
//! - `.bspace` - Blend spaces
//! - `.chrparams` - Character parameters
//! - Some `.xml` files (the binary variant)
//!
//! # Example
//!
//! ```no_run
//! use svarog_cryxml::CryXml;
//!
//! let data = std::fs::read("material.mtl")?;
//!
//! if CryXml::is_cryxml(&data) {
//!     let cryxml = CryXml::parse(&data)?;
//!     let xml_string = cryxml.to_xml_string()?;
//!     println!("{}", xml_string);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod header;
mod node;
mod attribute;
mod parser;

pub use error::{Error, Result};
pub use header::CryXmlHeader;
pub use node::CryXmlNode;
pub use attribute::CryXmlAttribute;
pub use parser::CryXml;

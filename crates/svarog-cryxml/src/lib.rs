//! CryXmlB binary XML parser and writer for Star Citizen files.
//!
//! Many Star Citizen configuration files use a binary XML format called CryXmlB.
//! This crate parses these files, converts them to standard XML, and can also
//! write CryXmlB files from XML or programmatic construction.
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
//! # Reading CryXmlB
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
//!
//! # Writing CryXmlB from XML
//!
//! ```no_run
//! use svarog_cryxml::builder::CryXmlBuilder;
//!
//! let xml = r#"<Material Name="MyMaterial">
//!     <Textures>
//!         <Texture Map="Diffuse" File="texture.dds"/>
//!     </Textures>
//! </Material>"#;
//!
//! let builder = CryXmlBuilder::from_xml(xml)?;
//! let cryxml_bytes = builder.build()?;
//! std::fs::write("material.mtl", cryxml_bytes)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Writing CryXmlB Programmatically
//!
//! ```no_run
//! use svarog_cryxml::builder::{CryXmlBuilder, BuilderNode};
//!
//! let root = BuilderNode::new("Material")
//!     .attr("Name", "MyMaterial")
//!     .child(BuilderNode::new("Textures")
//!         .child(BuilderNode::new("Texture")
//!             .attr("Map", "Diffuse")
//!             .attr("File", "texture.dds")));
//!
//! let builder = CryXmlBuilder::new(root);
//! let cryxml_bytes = builder.build()?;
//! std::fs::write("material.mtl", cryxml_bytes)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod header;
mod node;
mod attribute;
mod parser;
pub mod builder;
mod from_xml;

pub use error::{Error, Result};
pub use header::CryXmlHeader;
pub use node::CryXmlNode;
pub use attribute::CryXmlAttribute;
pub use parser::CryXml;

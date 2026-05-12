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

mod attribute;
pub mod builder;
mod error;
mod from_xml;
mod header;
mod node;
mod parser;

pub use attribute::CryXmlAttribute;
pub use error::{Error, Result};
pub use header::CryXmlHeader;
pub use node::CryXmlNode;
pub use parser::CryXml;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_layout_matches_decompiled_reader() {
        assert_eq!(CryXmlHeader::MAGIC_LEN, 8);
        assert_eq!(std::mem::size_of::<CryXmlHeader>(), 36);
        assert_eq!(std::mem::size_of::<CryXmlNode>(), 28);
        assert_eq!(std::mem::size_of::<CryXmlAttribute>(), 8);
    }
}

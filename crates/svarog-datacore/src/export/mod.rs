//! DataCore export functionality.
//!
//! This module provides the ability to export DataCore records to XML format,
//! walking the record graph and resolving references.
//!
//! # Performance
//!
//! With the `parallel` feature, exports can be parallelized using rayon for
//! significant speedups on multi-core systems. The exporter uses thread-local
//! buffers to minimize allocations and lock contention.

mod walker;
mod xml;

pub use walker::RecordWalker;
pub use xml::{ExportError, XmlExporter};

#[cfg(feature = "parallel")]
mod parallel;

#[cfg(feature = "parallel")]
pub use parallel::ParallelXmlExporter;

//! DataCore structure definitions.

mod definition;
mod property;
mod record;
mod reference;
mod string_id;

pub use definition::{DataCoreEnumDefinition, DataCoreStructDefinition};
pub use property::DataCorePropertyDefinition;
pub use record::{DataCoreDataMapping, DataCoreRecord};
pub use reference::{DataCorePointer, DataCoreReference};
pub use string_id::{DataCoreStringId, DataCoreStringId2};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_layout_matches_decompiled_reader() {
        assert_eq!(std::mem::size_of::<DataCoreStringId>(), 4);
        assert_eq!(std::mem::size_of::<DataCoreStringId2>(), 4);
        assert_eq!(std::mem::size_of::<DataCoreStructDefinition>(), 16);
        assert_eq!(std::mem::size_of::<DataCoreEnumDefinition>(), 8);
        assert_eq!(std::mem::size_of::<DataCorePropertyDefinition>(), 12);
        assert_eq!(std::mem::size_of::<DataCoreDataMapping>(), 8);
        assert_eq!(std::mem::size_of::<DataCoreRecord>(), 32);
        assert_eq!(std::mem::size_of::<DataCorePointer>(), 8);
        assert_eq!(std::mem::size_of::<DataCoreReference>(), 20);
    }
}

//! Query API for DataCore database.
//!
//! This module provides high-level query methods for finding and iterating
//! over records in the DataCore database.

use hashbrown::HashMap as FastHashMap;
use rustc_hash::FxHasher;
use std::hash::BuildHasherDefault;

use svarog_common::CigGuid;

use crate::instance::{Instance, Record};
use crate::structs::DataCoreRecord;
use crate::DataCoreDatabase;

type FxHashMap<K, V> = FastHashMap<K, V, BuildHasherDefault<FxHasher>>;

/// Extension trait for querying the DataCore database.
///
/// This trait provides high-level query methods that return the new
/// `Record` and `Instance` types for easier data access.
impl DataCoreDatabase {
    /// Get a record by its GUID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_datacore::DataCoreDatabase;
    /// use svarog_common::CigGuid;
    ///
    /// let db = DataCoreDatabase::open("Game.dcb")?;
    /// let guid = CigGuid::default(); // Your GUID here
    ///
    /// if let Some(record) = db.record(&guid) {
    ///     println!("Record: {:?}", record.name());
    ///     for prop in record.properties() {
    ///         println!("  {}: {}", prop.name, prop.value);
    ///     }
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn record(&self, guid: &CigGuid) -> Option<Record<'_>> {
        self.get_record(guid).map(|r| Record::new(self, r))
    }

    /// Get an instance by struct and instance index.
    ///
    /// This is useful when you have an `InstanceRef` from a property value.
    #[inline]
    pub fn instance(&self, struct_index: u32, instance_index: u32) -> Instance<'_> {
        Instance::new(self, struct_index, instance_index)
    }

    /// Iterate over all records in the database.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_datacore::DataCoreDatabase;
    ///
    /// let db = DataCoreDatabase::open("Game.dcb")?;
    ///
    /// for record in db.all_records() {
    ///     println!("{}: {}", record.type_name().unwrap_or("?"), record.name().unwrap_or("?"));
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn all_records(&self) -> impl Iterator<Item = Record<'_>> {
        self.records().iter().map(move |r| Record::new(self, r))
    }

    /// Iterate over all main records (one per file).
    ///
    /// Main records are the top-level records that correspond to separate
    /// exported XML files.
    pub fn all_main_records(&self) -> impl Iterator<Item = Record<'_>> {
        self.main_records().map(move |r| Record::new(self, r))
    }

    /// Find records by their struct type name.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_datacore::DataCoreDatabase;
    ///
    /// let db = DataCoreDatabase::open("Game.dcb")?;
    ///
    /// for weapon in db.records_by_type("EntityClassDefinition.Weapon") {
    ///     println!("Weapon: {}", weapon.name().unwrap_or("?"));
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn records_by_type<'a>(&'a self, type_name: &'a str) -> impl Iterator<Item = Record<'a>> {
        self.records().iter().filter_map(move |r| {
            let struct_name = self.struct_name(r.struct_index as usize)?;
            if struct_name == type_name {
                Some(Record::new(self, r))
            } else {
                None
            }
        })
    }

    /// Find records whose type name contains the given substring.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_datacore::DataCoreDatabase;
    ///
    /// let db = DataCoreDatabase::open("Game.dcb")?;
    ///
    /// for record in db.records_by_type_containing("Weapon") {
    ///     println!("{}: {}", record.type_name().unwrap_or("?"), record.name().unwrap_or("?"));
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn records_by_type_containing<'a>(
        &'a self,
        pattern: &'a str,
    ) -> impl Iterator<Item = Record<'a>> {
        self.records().iter().filter_map(move |r| {
            let struct_name = self.struct_name(r.struct_index as usize)?;
            if struct_name.contains(pattern) {
                Some(Record::new(self, r))
            } else {
                None
            }
        })
    }

    /// Find a record by name (first match).
    ///
    /// Note: Record names are not unique. Use `records_by_name` to find all matches.
    pub fn record_by_name(&self, name: &str) -> Option<Record<'_>> {
        self.records().iter().find_map(|r| {
            let record_name = self.record_name(r)?;
            if record_name == name {
                Some(Record::new(self, r))
            } else {
                None
            }
        })
    }

    /// Find all records with a given name.
    pub fn records_by_name<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Record<'a>> {
        self.records().iter().filter_map(move |r| {
            let record_name = self.record_name(r)?;
            if record_name == name {
                Some(Record::new(self, r))
            } else {
                None
            }
        })
    }

    /// Find records by file name/path.
    pub fn records_by_file<'a>(&'a self, file_name: &'a str) -> impl Iterator<Item = Record<'a>> {
        self.records().iter().filter_map(move |r| {
            let record_file = self.record_file_name(r)?;
            if record_file == file_name {
                Some(Record::new(self, r))
            } else {
                None
            }
        })
    }

    /// Get a list of all unique struct type names in the database.
    pub fn type_names(&self) -> Vec<&str> {
        self.struct_definitions()
            .iter()
            .enumerate()
            .filter_map(|(i, _)| self.struct_name(i))
            .collect()
    }

    /// Get a list of all unique enum type names in the database.
    pub fn enum_names(&self) -> Vec<&str> {
        self.enum_definitions()
            .iter()
            .enumerate()
            .filter_map(|(i, _)| self.enum_name(i))
            .collect()
    }

    /// Count records by type.
    ///
    /// Returns a map of type name to record count.
    pub fn count_by_type(&self) -> FxHashMap<&str, usize> {
        let mut counts: FxHashMap<&str, usize> = FxHashMap::default();

        for record in self.records() {
            if let Some(type_name) = self.struct_name(record.struct_index as usize) {
                *counts.entry(type_name).or_insert(0) += 1;
            }
        }

        counts
    }

    /// Resolve a record reference to a Record.
    pub fn resolve_reference(&self, guid: &CigGuid) -> Option<Record<'_>> {
        self.record(guid)
    }

    /// Resolve an instance reference to an Instance.
    pub fn resolve_instance(&self, struct_index: u32, instance_index: u32) -> Instance<'_> {
        self.instance(struct_index, instance_index)
    }
}

/// Query builder for complex record searches.
///
/// # Example
///
/// ```no_run
/// use svarog_datacore::{DataCoreDatabase, Query};
///
/// let db = DataCoreDatabase::open("Game.dcb")?;
///
/// let results: Vec<_> = Query::new(&db)
///     .type_contains("Weapon")
///     .main_only()
///     .collect();
///
/// for record in results {
///     println!("{}", record.name().unwrap_or("?"));
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Query<'a> {
    database: &'a DataCoreDatabase,
    type_filter: Option<TypeFilter<'a>>,
    name_filter: Option<&'a str>,
    file_filter: Option<&'a str>,
    main_only: bool,
}

enum TypeFilter<'a> {
    Exact(&'a str),
    Contains(&'a str),
}

impl<'a> Query<'a> {
    /// Create a new query builder.
    pub fn new(database: &'a DataCoreDatabase) -> Self {
        Self {
            database,
            type_filter: None,
            name_filter: None,
            file_filter: None,
            main_only: false,
        }
    }

    /// Filter by exact type name.
    pub fn type_exact(mut self, type_name: &'a str) -> Self {
        self.type_filter = Some(TypeFilter::Exact(type_name));
        self
    }

    /// Filter by type name containing substring.
    pub fn type_contains(mut self, pattern: &'a str) -> Self {
        self.type_filter = Some(TypeFilter::Contains(pattern));
        self
    }

    /// Filter by record name.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name_filter = Some(name);
        self
    }

    /// Filter by file name.
    pub fn file(mut self, file_name: &'a str) -> Self {
        self.file_filter = Some(file_name);
        self
    }

    /// Only return main records.
    pub fn main_only(mut self) -> Self {
        self.main_only = true;
        self
    }

    /// Execute the query and collect results.
    pub fn collect(self) -> Vec<Record<'a>> {
        self.into_iter().collect()
    }

    /// Execute the query and return the first result.
    pub fn first(self) -> Option<Record<'a>> {
        self.into_iter().next()
    }

    /// Execute the query and count results.
    pub fn count(self) -> usize {
        self.into_iter().count()
    }
}

impl<'a> IntoIterator for Query<'a> {
    type Item = Record<'a>;
    type IntoIter = QueryIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        QueryIterator {
            records: self.database.records().iter(),
            database: self.database,
            type_filter: self.type_filter,
            name_filter: self.name_filter,
            file_filter: self.file_filter,
            main_only: self.main_only,
        }
    }
}

/// Iterator for query results.
pub struct QueryIterator<'a> {
    records: std::slice::Iter<'a, DataCoreRecord>,
    database: &'a DataCoreDatabase,
    type_filter: Option<TypeFilter<'a>>,
    name_filter: Option<&'a str>,
    file_filter: Option<&'a str>,
    main_only: bool,
}

impl<'a> Iterator for QueryIterator<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let record = self.records.next()?;

            // Main record filter
            if self.main_only && !self.database.is_main_record(&record.id) {
                continue;
            }

            // Type filter
            if let Some(ref filter) = self.type_filter {
                let type_name = match self.database.struct_name(record.struct_index as usize) {
                    Some(n) => n,
                    None => continue,
                };

                match filter {
                    TypeFilter::Exact(name) => {
                        if type_name != *name {
                            continue;
                        }
                    }
                    TypeFilter::Contains(pattern) => {
                        if !type_name.contains(*pattern) {
                            continue;
                        }
                    }
                }
            }

            // Name filter
            if let Some(name) = self.name_filter {
                match self.database.record_name(record) {
                    Some(n) if n == name => {}
                    _ => continue,
                }
            }

            // File filter
            if let Some(file) = self.file_filter {
                match self.database.record_file_name(record) {
                    Some(f) if f == file => {}
                    _ => continue,
                }
            }

            return Some(Record::new(self.database, record));
        }
    }
}

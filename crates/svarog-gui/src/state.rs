//! Application state management

#![allow(dead_code)]

use crossbeam_channel::{Receiver, Sender};
use std::path::PathBuf;
use std::sync::Arc;

use svarog::datacore::DataCoreDatabase;
use svarog::p4k::P4kArchive;

/// Messages from background workers to UI
#[derive(Debug)]
pub enum WorkerMessage {
    P4kLoaded(Result<Arc<P4kArchive>, String>),
    P4kProgress { current: usize, total: usize, stage: String },
    DataCoreLoaded(Result<Arc<DataCoreDatabase>, String>),
    DataCoreProgress { current: usize, total: usize },
    ReferenceIndexReady(Arc<ReferenceIndex>),
    StructReferenceIndexReady(Arc<StructReferenceIndex>),
    ExtractionProgress { current: usize, total: usize, current_file: String },
    ExtractionComplete(Result<(), String>),
    FilePreviewReady(PreviewData),
    Error(String),
}

/// Preview data for different file types
#[derive(Debug, Clone)]
pub enum PreviewData {
    Text(String),
    Hex { data: Vec<u8>, offset: usize },
    Image(Vec<u8>), // PNG bytes
    None,
}

/// Represents a node in the P4K file tree
#[derive(Debug, Clone)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub compressed_size: u64,
    pub is_encrypted: bool,
    pub children: Vec<FileTreeNode>,
    pub expanded: bool,
    pub entry_index: Option<usize>,
}

impl FileTreeNode {
    pub fn new_directory(name: String, path: String) -> Self {
        Self {
            name,
            path,
            is_directory: true,
            size: 0,
            compressed_size: 0,
            is_encrypted: false,
            children: Vec::new(),
            expanded: false,
            entry_index: None,
        }
    }

    pub fn new_file(name: String, path: String, size: u64, compressed_size: u64, is_encrypted: bool, entry_index: usize) -> Self {
        Self {
            name,
            path,
            is_directory: false,
            size,
            compressed_size,
            is_encrypted,
            children: Vec::new(),
            expanded: false,
            entry_index: Some(entry_index),
        }
    }

    /// Sort children: directories first, then alphabetically
    pub fn sort_children(&mut self) {
        self.children.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        for child in &mut self.children {
            child.sort_children();
        }
    }
}

/// DataCore record for tree display
#[derive(Debug, Clone)]
pub struct DataCoreRecordNode {
    pub name: String,
    pub type_name: String,
    pub id: String,
    pub is_folder: bool,
    pub record_index: Option<usize>,
    pub children: Vec<DataCoreRecordNode>,
    pub expanded: bool,
    pub has_references: bool,
}

impl DataCoreRecordNode {
    pub fn new_folder(name: String) -> Self {
        Self {
            name,
            type_name: "Folder".to_string(),
            id: String::new(),
            is_folder: true,
            record_index: None,
            children: Vec::new(),
            expanded: false,
            has_references: false,
        }
    }

    pub fn new_record(name: String, type_name: String, id: String, record_index: usize, has_references: bool) -> Self {
        Self {
            name,
            type_name,
            id,
            is_folder: false,
            record_index: Some(record_index),
            children: Vec::new(),
            expanded: false,
            has_references,
        }
    }

    pub fn sort_children(&mut self) {
        self.children.sort_by(|a, b| {
            match (a.is_folder, b.is_folder) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        for child in &mut self.children {
            child.sort_children();
        }
    }
}

/// DataCore struct type for tree display
#[derive(Debug, Clone)]
pub struct DataCoreTypeNode {
    pub name: String,
    pub struct_index: Option<usize>,
    pub children: Vec<DataCoreTypeNode>,
    pub expanded: bool,
}

impl DataCoreTypeNode {
    pub fn new(name: String, struct_index: Option<usize>) -> Self {
        Self {
            name,
            struct_index,
            children: Vec::new(),
            expanded: false,
        }
    }

    pub fn sort_children(&mut self) {
        self.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for child in &mut self.children {
            child.sort_children();
        }
    }
}

/// A reference from one record to another (outgoing)
#[derive(Debug, Clone)]
pub struct RecordReference {
    pub property_name: String,
    pub ref_type: ReferenceType,
    pub target_name: String,
    pub target_type: String,
    pub target_guid: String,
    pub target_record_index: Option<usize>,
}

/// An incoming reference from another record
#[derive(Debug, Clone)]
pub struct IncomingReference {
    pub source_name: String,
    pub source_type: String,
    pub property_name: String,
    pub ref_type: ReferenceType,
    pub source_record_index: usize,
}

/// A reference from one struct to another type (outgoing)
#[derive(Debug, Clone)]
pub struct StructTypeReference {
    pub property_name: String,
    pub target_type: StructRefTarget,
    pub is_array: bool,
}

/// Target of a struct reference
#[derive(Debug, Clone)]
pub enum StructRefTarget {
    Struct { name: String, index: usize },
    Enum { name: String, index: usize },
}

/// An incoming reference to a struct (from another struct's property)
#[derive(Debug, Clone)]
pub struct IncomingStructReference {
    pub source_name: String,
    pub source_index: usize,
    pub property_name: String,
    pub is_array: bool,
}

/// Index mapping record indices to their incoming references
/// Built once when DCB is loaded for fast lookups
#[derive(Debug, Clone)]
pub struct ReferenceIndex {
    /// Maps target record index -> list of (source_record_index, property_name, ref_type)
    pub incoming: std::collections::HashMap<usize, Vec<(usize, String, ReferenceType)>>,
    /// Maps GUID string -> main record index for fast lookups
    pub guid_to_index: std::collections::HashMap<String, usize>,
}

/// Index mapping struct indices to their type references
#[derive(Debug, Clone)]
pub struct StructReferenceIndex {
    /// Maps struct index -> list of structs that reference it
    pub incoming: std::collections::HashMap<usize, Vec<IncomingStructReference>>,
    /// Maps enum index -> list of structs that reference it
    pub enum_incoming: std::collections::HashMap<usize, Vec<IncomingStructReference>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceType {
    Reference,
    StrongPointer,
    WeakPointer,
}

impl std::fmt::Display for ReferenceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReferenceType::Reference => write!(f, "Reference"),
            ReferenceType::StrongPointer => write!(f, "Strong Ptr"),
            ReferenceType::WeakPointer => write!(f, "Weak Ptr"),
        }
    }
}

/// Extraction options
#[derive(Debug, Clone)]
pub struct ExtractionOptions {
    pub output_path: PathBuf,
    pub filter_pattern: String,
    pub use_regex: bool,
    pub incremental: bool,
    pub expand_socpak: bool,
    pub extract_dcb: bool,
    pub parallel_workers: usize,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            output_path: PathBuf::new(),
            filter_pattern: String::new(),
            use_regex: false,
            incremental: true,
            expand_socpak: true,
            extract_dcb: true,
            parallel_workers: 0, // auto
        }
    }
}

/// Current active tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveTab {
    #[default]
    P4kBrowser,
    DataCoreBrowser,
}

/// Active page within the DataCore browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataCorePage {
    #[default]
    Records,
    Structs,
    Enums,
}

/// Main application state
pub struct AppState {
    // Current tab
    pub active_tab: ActiveTab,

    // P4K state
    pub p4k_path: Option<PathBuf>,
    pub p4k_archive: Option<Arc<P4kArchive>>,
    pub p4k_loading: bool,
    pub p4k_load_progress: (usize, usize, String),
    pub file_tree: Option<FileTreeNode>,
    pub selected_file: Option<String>,
    pub file_filter: String,

    // Preview state
    pub preview: PreviewData,
    pub preview_loading: bool,

    // DataCore state
    pub datacore: Option<Arc<DataCoreDatabase>>,
    pub datacore_loading: bool,
    pub datacore_progress: (usize, usize),
    pub datacore_tree: Option<DataCoreRecordNode>,
    pub datacore_type_tree: Option<DataCoreTypeNode>,
    pub datacore_search: String,
    pub selected_record: Option<usize>,
    pub selected_enum: Option<usize>,
    pub selected_type: Option<usize>,
    pub record_xml: String,
    pub enum_preview: String,
    pub type_preview: String,
    pub type_filter: Option<String>,
    pub record_references: Vec<RecordReference>,
    pub incoming_references: Vec<IncomingReference>,
    pub reference_index: Option<std::sync::Arc<ReferenceIndex>>,
    pub struct_reference_index: Option<std::sync::Arc<StructReferenceIndex>>,
    pub struct_outgoing_refs: Vec<StructTypeReference>,
    pub struct_incoming_refs: Vec<IncomingStructReference>,
    pub references_expanded: bool,
    pub incoming_expanded: bool,
    pub navigation_history: Vec<usize>,
    pub navigation_index: usize,
    pub selected_line: Option<usize>,
    pub datacore_page: DataCorePage,
    pub about_open: bool,

    // Extraction state
    pub extraction_options: ExtractionOptions,
    pub extraction_dialog_open: bool,
    pub extracting: bool,
    pub extraction_progress: (usize, usize, String),

    // Error display
    pub error_message: Option<String>,
    pub error_dismiss_time: Option<std::time::Instant>,

    // Communication channels
    pub worker_sender: Sender<WorkerMessage>,
    pub worker_receiver: Receiver<WorkerMessage>,
}

impl AppState {
    pub fn new() -> Self {
        let (sender, receiver) = crossbeam_channel::unbounded();
        Self {
            active_tab: ActiveTab::default(),
            p4k_path: None,
            p4k_archive: None,
            p4k_loading: false,
            p4k_load_progress: (0, 0, String::new()),
            file_tree: None,
            selected_file: None,
            file_filter: String::new(),
            preview: PreviewData::None,
            preview_loading: false,
            datacore: None,
            datacore_loading: false,
            datacore_progress: (0, 0),
            datacore_tree: None,
            datacore_type_tree: None,
            datacore_search: String::new(),
            selected_record: None,
            selected_enum: None,
            selected_type: None,
            record_xml: String::new(),
            enum_preview: String::new(),
            type_preview: String::new(),
            type_filter: None,
            record_references: Vec::new(),
            incoming_references: Vec::new(),
            reference_index: None,
            struct_reference_index: None,
            struct_outgoing_refs: Vec::new(),
            struct_incoming_refs: Vec::new(),
            references_expanded: true,
            incoming_expanded: true,
            navigation_history: Vec::new(),
            navigation_index: 0,
            selected_line: None,
            datacore_page: DataCorePage::default(),
            about_open: false,
            extraction_options: ExtractionOptions::default(),
            extraction_dialog_open: false,
            extracting: false,
            extraction_progress: (0, 0, String::new()),
            error_message: None,
            error_dismiss_time: None,
            worker_sender: sender,
            worker_receiver: receiver,
        }
    }

    pub fn show_error(&mut self, msg: impl Into<String>) {
        self.error_message = Some(msg.into());
        self.error_dismiss_time = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    }

    pub fn clear_error(&mut self) {
        self.error_message = None;
        self.error_dismiss_time = None;
    }

    /// Process messages from workers
    pub fn process_messages(&mut self) {
        while let Ok(msg) = self.worker_receiver.try_recv() {
            match msg {
                WorkerMessage::P4kLoaded(result) => {
                    self.p4k_loading = false;
                    match result {
                        Ok(archive) => {
                            self.p4k_archive = Some(archive);
                        self.build_file_tree();
                        }
                        Err(e) => self.show_error(format!("Failed to load P4K: {}", e)),
                    }
                }
                WorkerMessage::P4kProgress { current, total, stage } => {
                    self.p4k_load_progress = (current, total, stage);
                }
                WorkerMessage::DataCoreLoaded(result) => {
                    self.datacore_loading = false;
                    match result {
                        Ok(db) => {
                            self.datacore = Some(db.clone());
                            self.selected_record = None;
                            self.selected_type = None;
                            self.selected_enum = None;
                            self.record_xml.clear();
                            self.type_preview.clear();
                            self.enum_preview.clear();
                            self.datacore_page = DataCorePage::Records;
                            self.build_datacore_tree();
                            self.build_datacore_type_tree();
                            // Build reference index in background
                            crate::worker::build_reference_index(db, self.worker_sender.clone());
                        }
                        Err(e) => self.show_error(format!("Failed to load DataCore: {}", e)),
                    }
                }
                WorkerMessage::ReferenceIndexReady(index) => {
                    self.reference_index = Some(index);
                }
                WorkerMessage::StructReferenceIndexReady(index) => {
                    self.struct_reference_index = Some(index);
                }
                WorkerMessage::DataCoreProgress { current, total } => {
                    self.datacore_progress = (current, total);
                }
                WorkerMessage::ExtractionProgress { current, total, current_file } => {
                    self.extraction_progress = (current, total, current_file);
                }
                WorkerMessage::ExtractionComplete(result) => {
                    self.extracting = false;
                    if let Err(e) = result {
                        self.show_error(format!("Extraction failed: {}", e));
                    }
                }
                WorkerMessage::FilePreviewReady(data) => {
                    self.preview = data;
                    self.preview_loading = false;
                }
                WorkerMessage::Error(e) => {
                    self.show_error(e);
                }
            }
        }

        // Auto-dismiss errors
        if let Some(dismiss_time) = self.error_dismiss_time {
            if std::time::Instant::now() > dismiss_time {
                self.clear_error();
            }
        }
    }

    /// Build file tree from P4K archive
    fn build_file_tree(&mut self) {
        let Some(archive) = &self.p4k_archive else { return };

        let mut root = FileTreeNode::new_directory("root".to_string(), String::new());

        for (idx, entry) in archive.iter().enumerate() {
            let path = entry.name.replace('\\', "/");
            let parts: Vec<&str> = path.split('/').collect();

            let mut current_children = &mut root.children;
            let mut current_path = String::new();

            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    current_path.push('/');
                }
                current_path.push_str(part);

                let is_last = i == parts.len() - 1;

                if is_last {
                    // File node
                    current_children.push(FileTreeNode::new_file(
                        part.to_string(),
                        current_path.clone(),
                        entry.uncompressed_size,
                        entry.compressed_size,
                        entry.is_encrypted,
                        idx,
                    ));
                } else {
                    // Directory node
                    let pos = current_children.iter().position(|c| c.name == *part && c.is_directory);
                    if let Some(pos) = pos {
                        current_children = &mut current_children[pos].children;
                    } else {
                        current_children.push(FileTreeNode::new_directory(
                            part.to_string(),
                            current_path.clone(),
                        ));
                        let last = current_children.len() - 1;
                        current_children = &mut current_children[last].children;
                    }
                }
            }
        }

        root.sort_children();
        self.file_tree = Some(root);
    }

    /// Build DataCore record tree
    fn build_datacore_tree(&mut self) {
        use svarog::datacore::{Value, ArrayElementType};

        let Some(db) = &self.datacore else { return };

        let mut root = DataCoreRecordNode::new_folder("DataCore".to_string());

        for (idx, record) in db.main_records().enumerate() {
            let file_name = db.record_file_name(&record).unwrap_or_default();
            let path = file_name.replace('\\', "/");
            let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

            // Check if record has any references
            let instance = db.instance(record.struct_index as u32, record.instance_index as u32);
            let has_refs = instance.properties().any(|prop| {
                match &prop.value {
                    Value::Reference(Some(_)) => true,
                    Value::StrongPointer(Some(_)) => true,
                    Value::WeakPointer(Some(_)) => true,
                    Value::Array(arr) => {
                        matches!(
                            arr.element_type,
                            ArrayElementType::Reference | ArrayElementType::StrongPointer | ArrayElementType::WeakPointer
                        ) && arr.count > 0
                    }
                    _ => false,
                }
            });

            let mut current_children = &mut root.children;

            for (i, part) in parts.iter().enumerate() {
                let is_last = i == parts.len() - 1;

                if is_last {
                    // Record node
                    let name = db.record_name(record).unwrap_or("Unknown").to_string();
                    let type_name = db.struct_name(record.struct_index as usize).unwrap_or("Unknown").to_string();
                    current_children.push(DataCoreRecordNode::new_record(
                        name,
                        type_name,
                        format!("{}", record.id),
                        idx,
                        has_refs,
                    ));
                } else {
                    // Folder node
                    let pos = current_children.iter().position(|c| c.name == *part && c.is_folder);
                    if let Some(pos) = pos {
                        current_children = &mut current_children[pos].children;
                    } else {
                        current_children.push(DataCoreRecordNode::new_folder(part.to_string()));
                        let last = current_children.len() - 1;
                        current_children = &mut current_children[last].children;
                    }
                }
            }
        }

        root.sort_children();
        self.datacore_tree = Some(root);
    }

    /// Build DataCore type tree from struct definitions and inheritance
    fn build_datacore_type_tree(&mut self) {
        let Some(db) = &self.datacore else { return };
        let defs = db.struct_definitions();

        if defs.is_empty() {
            self.datacore_type_tree = None;
            return;
        }

        let names: Vec<String> = defs
            .iter()
            .enumerate()
            .map(|(i, _)| {
                db.struct_name(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Struct{}", i))
            })
            .collect();

        let mut children_map: Vec<Vec<usize>> = vec![Vec::new(); defs.len()];
        let mut roots = Vec::new();

        for (idx, def) in defs.iter().enumerate() {
            if def.parent_type_index >= 0 {
                let parent = def.parent_type_index as usize;
                if parent < defs.len() {
                    children_map[parent].push(idx);
                } else {
                    roots.push(idx);
                }
            } else {
                roots.push(idx);
            }
        }

        fn build_node(
            idx: usize,
            names: &[String],
            children_map: &[Vec<usize>],
        ) -> DataCoreTypeNode {
            let mut node = DataCoreTypeNode::new(names[idx].clone(), Some(idx));
            for &child in &children_map[idx] {
                node.children.push(build_node(child, names, children_map));
            }
            node
        }

        let mut root = DataCoreTypeNode::new("Types".to_string(), None);
        for idx in roots {
            root.children.push(build_node(idx, &names, &children_map));
        }
        root.sort_children();
        self.datacore_type_tree = Some(root);
    }

    /// Build the reference index for fast incoming reference lookups
    /// Note: This is deprecated in favor of the worker-based build_reference_index
    #[allow(dead_code)]
    fn build_reference_index(&mut self) {
        use svarog::datacore::{Value, ArrayElementType};

        let Some(db) = &self.datacore else { return };

        let mut incoming: std::collections::HashMap<usize, Vec<(usize, String, ReferenceType)>> =
            std::collections::HashMap::new();
        let mut guid_to_index: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        let main_records: Vec<_> = db.main_records().collect();

        // Build GUID -> index map
        for (idx, record) in main_records.iter().enumerate() {
            guid_to_index.insert(format!("{}", record.id), idx);
        }

        // Build instance -> index map
        let mut instance_to_index: std::collections::HashMap<(u32, u32), usize> =
            std::collections::HashMap::new();
        for (idx, record) in main_records.iter().enumerate() {
            instance_to_index.insert((record.struct_index as u32, record.instance_index as u32), idx);
        }

        for (source_idx, record) in main_records.iter().enumerate() {
            let instance = db.instance(record.struct_index as u32, record.instance_index as u32);

            for prop in instance.properties() {
                match &prop.value {
                    Value::Reference(Some(record_ref)) => {
                        let guid_str = format!("{}", record_ref.guid);
                        if let Some(&target_idx) = guid_to_index.get(&guid_str) {
                            incoming
                                .entry(target_idx)
                                .or_default()
                                .push((source_idx, prop.name.to_string(), ReferenceType::Reference));
                        }
                    }
                    Value::StrongPointer(Some(instance_ref)) => {
                        let key = (instance_ref.struct_index, instance_ref.instance_index);
                        if let Some(&target_idx) = instance_to_index.get(&key) {
                            incoming
                                .entry(target_idx)
                                .or_default()
                                .push((source_idx, prop.name.to_string(), ReferenceType::StrongPointer));
                        }
                    }
                    Value::WeakPointer(Some(instance_ref)) => {
                        let key = (instance_ref.struct_index, instance_ref.instance_index);
                        if let Some(&target_idx) = instance_to_index.get(&key) {
                            incoming
                                .entry(target_idx)
                                .or_default()
                                .push((source_idx, prop.name.to_string(), ReferenceType::WeakPointer));
                        }
                    }
                    Value::Array(array_ref) => {
                        if array_ref.count > 0 && array_ref.count < 1_000_000 {
                            match array_ref.element_type {
                                ArrayElementType::Reference => {
                                    for i in 0..array_ref.count.min(100) {
                                        let idx = array_ref.first_index as usize + i as usize;
                                        if let Some(ref_val) = db.reference_value(idx) {
                                            let guid_str = format!("{}", ref_val.record_id);
                                            if let Some(&target_idx) = guid_to_index.get(&guid_str) {
                                                incoming
                                                    .entry(target_idx)
                                                    .or_default()
                                                    .push((source_idx, format!("{}[{}]", prop.name, i), ReferenceType::Reference));
                                            }
                                        }
                                    }
                                }
                                ArrayElementType::StrongPointer | ArrayElementType::WeakPointer => {
                                    let ref_type = if array_ref.element_type == ArrayElementType::StrongPointer {
                                        ReferenceType::StrongPointer
                                    } else {
                                        ReferenceType::WeakPointer
                                    };

                                    for i in 0..array_ref.count.min(100) {
                                        let idx = array_ref.first_index as usize + i as usize;
                                        let ptr = match array_ref.element_type {
                                            ArrayElementType::StrongPointer => db.strong_value(idx),
                                            ArrayElementType::WeakPointer => db.weak_value(idx),
                                            _ => None,
                                        };

                                        if let Some(ptr) = ptr {
                                            let key = (ptr.struct_index as u32, ptr.instance_index as u32);
                                            if let Some(&target_idx) = instance_to_index.get(&key) {
                                                incoming
                                                    .entry(target_idx)
                                                    .or_default()
                                                    .push((source_idx, format!("{}[{}]", prop.name, i), ref_type));
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        self.reference_index = Some(std::sync::Arc::new(ReferenceIndex { incoming, guid_to_index }));
    }
}

//! Background worker tasks

use crossbeam_channel::Sender;
use std::path::Path;
use std::sync::Arc;

use svarog::cryxml::CryXml;
use svarog::datacore::DataCoreDatabase;
use svarog::p4k::P4kArchive;

use crate::state::{PreviewData, ReferenceIndex, ReferenceType, WorkerMessage};

/// Load a P4K archive in a background thread
pub fn load_p4k(path: impl AsRef<Path>, sender: Sender<WorkerMessage>) {
    let path = path.as_ref().to_owned();
    std::thread::spawn(move || {
        sender.send(WorkerMessage::P4kProgress {
            current: 0,
            total: 1,
            stage: "Opening archive...".to_string(),
        }).ok();

        match P4kArchive::open(&path) {
            Ok(archive) => {
                let count = archive.entry_count();
                sender.send(WorkerMessage::P4kProgress {
                    current: count,
                    total: count,
                    stage: format!("Loaded {} entries", count),
                }).ok();
                sender.send(WorkerMessage::P4kLoaded(Ok(Arc::new(archive)))).ok();
            }
            Err(e) => {
                sender.send(WorkerMessage::P4kLoaded(Err(e.to_string()))).ok();
            }
        }
    });
}

/// Load DataCore database in a background thread
pub fn load_datacore(data: Vec<u8>, sender: Sender<WorkerMessage>) {
    std::thread::spawn(move || {
        sender.send(WorkerMessage::DataCoreProgress {
            current: 0,
            total: 1,
        }).ok();

        match DataCoreDatabase::parse(&data) {
            Ok(db) => {
                let count = db.records().len();
                sender.send(WorkerMessage::DataCoreProgress {
                    current: count,
                    total: count,
                }).ok();
                sender.send(WorkerMessage::DataCoreLoaded(Ok(Arc::new(db)))).ok();
            }
            Err(e) => {
                sender.send(WorkerMessage::DataCoreLoaded(Err(e.to_string()))).ok();
            }
        }
    });
}

/// Load file preview in a background thread
pub fn load_preview(archive: Arc<P4kArchive>, entry_index: usize, sender: Sender<WorkerMessage>) {
    std::thread::spawn(move || {
        let entry = match archive.get(entry_index) {
            Some(e) => e,
            None => {
                sender.send(WorkerMessage::FilePreviewReady(PreviewData::None)).ok();
                return;
            }
        };

        // Read file data
        let data = match archive.read_index(entry_index) {
            Ok(d) => d,
            Err(e) => {
                sender.send(WorkerMessage::Error(format!("Failed to read file: {}", e))).ok();
                sender.send(WorkerMessage::FilePreviewReady(PreviewData::None)).ok();
                return;
            }
        };

        let name_lower = entry.name.to_lowercase();
        let preview = determine_preview(&data, &name_lower);
        sender.send(WorkerMessage::FilePreviewReady(preview)).ok();
    });
}

fn determine_preview(data: &[u8], name_lower: &str) -> PreviewData {
    // Check for CryXML binary
    if CryXml::is_cryxml(data) {
        match CryXml::parse(data) {
            Ok(xml) => {
                match xml.to_xml_string() {
                    Ok(text) => return PreviewData::Text(text),
                    Err(_) => {}
                }
            }
            Err(_) => {}
        }
    }

    // Check for text files
    let text_extensions = [
        ".xml", ".txt", ".cfg", ".json", ".eco", ".lua", ".mtl", ".cdf",
        ".chrparams", ".adb", ".animevents", ".bspace", ".log", ".ini",
        ".csv", ".md", ".html", ".css", ".js",
    ];

    for ext in &text_extensions {
        if name_lower.ends_with(ext) {
            // Try to parse as UTF-8 text
            if let Ok(text) = String::from_utf8(data.to_vec()) {
                // Check if it's actually text (no binary chars)
                if text.chars().all(|c| c.is_ascii() || c.is_alphanumeric() || c.is_whitespace() || c == '\n' || c == '\r' || c == '\t') || !text.contains('\0') {
                    return PreviewData::Text(text);
                }
            }
            break;
        }
    }

    // Check for images
    let image_extensions = [".png", ".jpg", ".jpeg", ".bmp"];
    for ext in &image_extensions {
        if name_lower.ends_with(ext) {
            return PreviewData::Image(data.to_vec());
        }
    }

    // Check for DDS - convert to PNG
    if name_lower.ends_with(".dds") {
        // For now, show as hex - DDS conversion requires additional libraries
        // In a full implementation, we'd use the ddsfile crate or similar
        return PreviewData::Hex {
            data: data.to_vec(),
            offset: 0,
        };
    }

    // Default to hex view for small files, or truncated hex for large
    let max_hex_size = 1024 * 1024; // 1MB
    let display_data = if data.len() > max_hex_size {
        data[..max_hex_size].to_vec()
    } else {
        data.to_vec()
    };

    PreviewData::Hex {
        data: display_data,
        offset: 0,
    }
}

/// Build reference index in a background thread
pub fn build_reference_index(db: Arc<DataCoreDatabase>, sender: Sender<WorkerMessage>) {
    std::thread::spawn(move || {
        use svarog::datacore::{Value, ArrayElementType};

        let mut incoming: std::collections::HashMap<usize, Vec<(usize, String, ReferenceType)>> =
            std::collections::HashMap::new();

        let main_records: Vec<_> = db.main_records().collect();

        for (source_idx, record) in main_records.iter().enumerate() {
            let instance = db.instance(record.struct_index as u32, record.instance_index as u32);

            for prop in instance.properties() {
                match &prop.value {
                    Value::Reference(Some(record_ref)) => {
                        if let Some(target_idx) = main_records.iter().position(|r| r.id == record_ref.guid) {
                            incoming
                                .entry(target_idx)
                                .or_default()
                                .push((source_idx, prop.name.to_string(), ReferenceType::Reference));
                        }
                    }
                    Value::StrongPointer(Some(instance_ref)) => {
                        let ptr_struct_index = instance_ref.struct_index;
                        let ptr_instance_index = instance_ref.instance_index;

                        if let Some(target_idx) = main_records.iter().position(|r| {
                            r.struct_index as u32 == ptr_struct_index && r.instance_index as u32 == ptr_instance_index
                        }) {
                            incoming
                                .entry(target_idx)
                                .or_default()
                                .push((source_idx, prop.name.to_string(), ReferenceType::StrongPointer));
                        }
                    }
                    Value::WeakPointer(Some(instance_ref)) => {
                        let ptr_struct_index = instance_ref.struct_index;
                        let ptr_instance_index = instance_ref.instance_index;

                        if let Some(target_idx) = main_records.iter().position(|r| {
                            r.struct_index as u32 == ptr_struct_index && r.instance_index as u32 == ptr_instance_index
                        }) {
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
                                            if let Some(target_idx) = main_records.iter().position(|r| r.id == ref_val.record_id) {
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
                                            let ptr_struct_index = ptr.struct_index;
                                            let ptr_instance_index = ptr.instance_index;

                                            if let Some(target_idx) = main_records.iter().position(|r| {
                                                r.struct_index as i32 == ptr_struct_index && r.instance_index as i32 == ptr_instance_index
                                            }) {
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

        sender.send(WorkerMessage::ReferenceIndexReady(Arc::new(ReferenceIndex { incoming }))).ok();
    });
}

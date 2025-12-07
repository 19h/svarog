//! Extraction dialog

use eframe::egui::{self, Color32, RichText, Ui};

use crate::state::AppState;
use crate::widgets::progress_bar;

pub struct ExtractionDialog;

impl ExtractionDialog {
    pub fn show(ctx: &egui::Context, state: &mut AppState) {
        if !state.extraction_dialog_open {
            return;
        }

        let mut open = state.extraction_dialog_open;

        egui::Window::new("Extract Files")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(500.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if state.extracting {
                    Self::show_progress(ui, state);
                } else {
                    Self::show_options(ui, state);
                }
            });

        state.extraction_dialog_open = open;
    }

    fn show_options(ui: &mut Ui, state: &mut AppState) {
        ui.heading("Extraction Options");
        ui.add_space(10.0);

        egui::Grid::new("extraction_options")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                // Output directory
                ui.label("Output directory:");
                ui.horizontal(|ui| {
                    let mut path_str = state.extraction_options.output_path.display().to_string();
                    ui.add(
                        egui::TextEdit::singleline(&mut path_str)
                            .desired_width(300.0)
                            .interactive(false)
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.extraction_options.output_path = path;
                        }
                    }
                });
                ui.end_row();

                // Filter pattern
                ui.label("Filter pattern:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut state.extraction_options.filter_pattern);
                    ui.checkbox(&mut state.extraction_options.use_regex, "Regex");
                });
                ui.end_row();

                // Options
                ui.label("Options:");
                ui.vertical(|ui| {
                    ui.checkbox(&mut state.extraction_options.incremental, "Incremental (skip existing)");
                    ui.checkbox(&mut state.extraction_options.expand_socpak, "Expand SOCPAK archives");
                    ui.checkbox(&mut state.extraction_options.extract_dcb, "Extract DataCore to XML");
                });
                ui.end_row();

                // Parallel workers
                ui.label("Parallel workers:");
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut state.extraction_options.parallel_workers, 0..=16));
                    if state.extraction_options.parallel_workers == 0 {
                        ui.label("(auto)");
                    }
                });
                ui.end_row();
            });

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        // Action buttons
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                state.extraction_dialog_open = false;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_extract = !state.extraction_options.output_path.as_os_str().is_empty()
                    && state.p4k_archive.is_some();

                if ui.add_enabled(can_extract, egui::Button::new(
                    RichText::new("📤 Start Extraction").strong()
                )).clicked() {
                    start_extraction(state);
                }
            });
        });
    }

    fn show_progress(ui: &mut Ui, state: &mut AppState) {
        ui.heading("Extracting...");
        ui.add_space(20.0);

        let (current, total, file) = &state.extraction_progress;
        progress_bar(ui, *current, *total, "");

        ui.add_space(10.0);
        ui.label(format!("{} / {} files", current, total));

        ui.add_space(5.0);
        ui.label(
            RichText::new(file)
                .monospace()
                .color(Color32::GRAY)
        );

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        // Cancel button would go here - but extraction is typically fast enough
        // that cancellation isn't usually needed
        ui.horizontal(|ui| {
            if ui.button("Close").clicked() {
                if !state.extracting {
                    state.extraction_dialog_open = false;
                }
            }
        });
    }
}

fn start_extraction(state: &mut AppState) {
    let Some(archive) = &state.p4k_archive else {
        return;
    };

    let options = state.extraction_options.clone();
    let sender = state.worker_sender.clone();
    let archive = archive.clone();

    state.extracting = true;
    state.extraction_progress = (0, 0, String::new());

    std::thread::spawn(move || {
        let output_path = &options.output_path;

        // Create output directory
        if let Err(e) = std::fs::create_dir_all(output_path) {
            sender.send(crate::state::WorkerMessage::ExtractionComplete(
                Err(format!("Failed to create output directory: {}", e))
            )).ok();
            return;
        }

        // Collect entries to extract
        let entries: Vec<_> = archive.iter().enumerate().collect();
        let total = entries.len();

        // Apply filter if specified
        let filter = if options.filter_pattern.is_empty() {
            None
        } else if options.use_regex {
            match regex::Regex::new(&options.filter_pattern) {
                Ok(re) => Some(FilterType::Regex(re)),
                Err(e) => {
                    sender.send(crate::state::WorkerMessage::ExtractionComplete(
                        Err(format!("Invalid regex: {}", e))
                    )).ok();
                    return;
                }
            }
        } else {
            match glob::Pattern::new(&options.filter_pattern) {
                Ok(pat) => Some(FilterType::Glob(pat)),
                Err(e) => {
                    sender.send(crate::state::WorkerMessage::ExtractionComplete(
                        Err(format!("Invalid glob pattern: {}", e))
                    )).ok();
                    return;
                }
            }
        };

        let mut extracted = 0;
        let mut errors = Vec::new();

        for (idx, entry) in entries {
            let name = entry.name.replace('\\', "/");

            // Apply filter
            if let Some(ref filter) = filter {
                let matches = match filter {
                    FilterType::Glob(pat) => pat.matches(&name),
                    FilterType::Regex(re) => re.is_match(&name),
                };
                if !matches {
                    continue;
                }
            }

            sender.send(crate::state::WorkerMessage::ExtractionProgress {
                current: extracted,
                total,
                current_file: name.clone(),
            }).ok();

            let file_path = output_path.join(&name);

            // Skip if incremental and file exists with same size
            if options.incremental {
                if let Ok(meta) = std::fs::metadata(&file_path) {
                    if meta.len() == entry.uncompressed_size {
                        extracted += 1;
                        continue;
                    }
                }
            }

            // Create parent directory
            if let Some(parent) = file_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    errors.push(format!("Failed to create directory for {}: {}", name, e));
                    continue;
                }
            }

            // Read and write file
            match archive.read_index(idx) {
                Ok(data) => {
                    // Check for CryXML and decode
                    let final_data = if svarog::cryxml::CryXml::is_cryxml(&data) {
                        match svarog::cryxml::CryXml::parse(&data) {
                            Ok(xml) => {
                                match xml.to_xml_string() {
                                    Ok(text) => text.into_bytes(),
                                    Err(_) => data,
                                }
                            }
                            Err(_) => data,
                        }
                    } else {
                        data
                    };

                    if let Err(e) = std::fs::write(&file_path, &final_data) {
                        errors.push(format!("Failed to write {}: {}", name, e));
                    }
                }
                Err(e) => {
                    errors.push(format!("Failed to read {}: {}", name, e));
                }
            }

            extracted += 1;
        }

        sender.send(crate::state::WorkerMessage::ExtractionProgress {
            current: extracted,
            total,
            current_file: "Complete!".to_string(),
        }).ok();

        if errors.is_empty() {
            sender.send(crate::state::WorkerMessage::ExtractionComplete(Ok(()))).ok();
        } else {
            sender.send(crate::state::WorkerMessage::ExtractionComplete(
                Err(format!("{} errors during extraction", errors.len()))
            )).ok();
        }
    });
}

enum FilterType {
    Glob(glob::Pattern),
    Regex(regex::Regex),
}

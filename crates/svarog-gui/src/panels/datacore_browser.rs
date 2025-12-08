//! DataCore database browser panel

use eframe::egui::{self, Color32, RichText, ScrollArea, Ui, Sense, Vec2, Key, CursorIcon};
use std::sync::Arc;

use crate::state::{AppState, DataCorePage, DataCoreRecordNode, DataCoreTypeNode, IncomingReference, NavigationEntry, RecordReference, ReferenceIndex, ReferenceType, StructRefTarget, StructTypeReference};
use crate::widgets::{progress_bar, search_box};
use crate::worker;

pub struct DataCoreBrowserPanel;

impl DataCoreBrowserPanel {
    pub fn show(ui: &mut Ui, state: &mut AppState) {
        // Auto-load DataCore from P4K if available and not yet loaded
        if state.p4k_archive.is_some() && state.datacore.is_none() && !state.datacore_loading {
            Self::load_datacore_from_p4k(state);
        }

        // Handle keyboard/mouse navigation
        Self::handle_navigation(ui, state);

        // Top toolbar (mirrors P4K ordering: open -> export -> nav/search/stats)
        ui.horizontal(|ui| {
            if ui.button("Open DCB").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("DataCore Database", &["dcb"])
                    .pick_file()
                {
                    match std::fs::read(&path) {
                        Ok(data) => {
                            state.datacore_loading = true;
                            worker::load_datacore(data, state.worker_sender.clone());
                        }
                        Err(e) => state.show_error(format!("Failed to read file: {}", e)),
                    }
                }
            }

            if state.datacore.is_some() {
                ui.separator();

                let can_export_current = match state.datacore_page {
                    DataCorePage::Structs => !state.type_preview.is_empty(),
                    DataCorePage::Enums => !state.enum_preview.is_empty(),
                    _ => state.selected_record.is_some(),
                };

                if ui.add_enabled(can_export_current, egui::Button::new("Export")).clicked() {
                    let db = state.datacore.clone();
                    if let Some(db) = db {
                        if let Err(e) = export_current(&db, state) {
                            state.show_error(e);
                        }
                    }
                }

                if ui.button("Export All").clicked() {
                    let db = state.datacore.clone();
                    if let Some(db) = db {
                        if let Err(e) = export_all(&db, state) {
                            state.show_error(e);
                        }
                    }
                }

                ui.separator();

                let can_go_back = state.navigation_index > 0;
                let can_go_forward = state.navigation_index + 1 < state.navigation_history.len();

                if ui.add_enabled(can_go_back, egui::Button::new("<").small())
                    .on_hover_text("Back (Mouse Back / Alt+Left)")
                    .clicked()
                {
                    Self::navigate_back(state);
                }

                if ui.add_enabled(can_go_forward, egui::Button::new(">").small())
                    .on_hover_text("Forward (Mouse Forward / Alt+Right)")
                    .clicked()
                {
                    Self::navigate_forward(state);
                }

                ui.separator();

                // Records / Types toggle
                if let Some(db) = &state.datacore {
                    let rec_btn = ui
                        .selectable_label(
                            state.datacore_page == DataCorePage::Records,
                            format!("Records ({})", db.records().len()),
                        )
                        .on_hover_text("Browse record tree");
                    if rec_btn.clicked() {
                        state.datacore_page = DataCorePage::Records;
                    }

                    let struct_btn = ui
                        .selectable_label(
                            state.datacore_page == DataCorePage::Structs,
                            format!("Structs ({})", db.struct_definitions().len()),
                        )
                        .on_hover_text("Browse struct definitions");
                    if struct_btn.clicked() {
                        state.datacore_page = DataCorePage::Structs;
                    }

                    let enum_btn = ui
                        .selectable_label(
                            state.datacore_page == DataCorePage::Enums,
                            format!("Enums ({})", db.enum_definitions().len()),
                        )
                        .on_hover_text("Browse enum definitions");
                    if enum_btn.clicked() {
                        state.datacore_page = DataCorePage::Enums;
                    }

                    ui.separator();
                }

                let search_label = match state.datacore_page {
                    DataCorePage::Records => "Search records...",
                    DataCorePage::Structs => "Search structs...",
                    DataCorePage::Enums => "Search enums...",
                };
                search_box(ui, &mut state.datacore_search, search_label);

                ui.separator();

                if state.datacore_page == DataCorePage::Records {
                    if let Some(type_filter) = &state.type_filter {
                        ui.label(
                            RichText::new(format!("Type: {}", type_filter))
                                .color(Color32::from_rgb(255, 200, 100))
                                .small()
                        );
                        if ui.small_button("x").on_hover_text("Clear type filter").clicked() {
                            state.type_filter = None;
                        }
                        ui.separator();
                    }
                }

                if let Some(db) = &state.datacore {
                    ui.label(
                        RichText::new(format!(
                            "{} records | {} structs | {} enums",
                            db.records().len(),
                            db.struct_definitions().len(),
                            db.enum_definitions().len()
                        ))
                        .color(Color32::from_gray(150))
                    );
                }
            }
        });

        ui.separator();

        // Loading state
        if state.datacore_loading {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.spinner();
                progress_bar(
                    ui,
                    state.datacore_progress.0,
                    state.datacore_progress.1,
                    "Loading DataCore database...",
                );
            });
            return;
        }

        match state.datacore_page {
            DataCorePage::Records => {
                if state.datacore_tree.is_some() && state.datacore.is_some() {
                    let panel_height = ui.available_height();
                    let available_width = ui.available_width();
                    let tree_width = (available_width * 0.4).max(200.0);

                    ui.columns(2, |columns| {
                        columns[0].set_max_width(tree_width);

                        let rect = columns[0].available_rect_before_wrap();
                        columns[0].painter().vline(
                            rect.right() + 4.0,
                            rect.top()..=rect.bottom(),
                            egui::Stroke::new(2.0, Color32::from_gray(55))
                        );

                        ScrollArea::vertical()
                            .id_salt("dcb_tree_scroll")
                            .auto_shrink([false, false])
                            .show(&mut columns[0], |ui| {
                                if let Some(tree) = &mut state.datacore_tree {
                                    let search = state.datacore_search.to_lowercase();
                                    let type_filter = state.type_filter.clone();
                                    let selected = &mut state.selected_record;
                                    let record_xml = &mut state.record_xml;
                                    let record_refs = &mut state.record_references;
                                    let db = state.datacore.clone();
                                    let mut new_type_filter: Option<String> = None;
                                    let mut navigate_to: Option<usize> = None;

                                    if !search.is_empty() || type_filter.is_some() {
                                        for child in &mut tree.children {
                                            check_and_expand_for_search(child, &search, type_filter.as_deref());
                                        }
                                    }

                                    let mut row_index = 0usize;
                                    for child in &mut tree.children {
                                        render_record_tree(
                                            ui, child, &search, type_filter.as_deref(),
                                            selected, record_xml, record_refs, db.clone(),
                                            0, &mut row_index, &mut new_type_filter, &mut navigate_to,
                                        );
                                    }

                                    if let Some(filter) = new_type_filter {
                                        state.type_filter = Some(filter);
                                    }
                                    if let Some(idx) = navigate_to {
                                        Self::navigate_to_record(state, idx);
                                    }
                                }
                            });

                        columns[1].vertical(|ui| {
                            if let Some(record_idx) = state.selected_record {
                                if let Some(db) = &state.datacore {
                                    let records: Vec<_> = db.main_records().collect();
                                    if let Some(record) = records.get(record_idx) {
                                        let name = db.record_name(record).unwrap_or("Unknown");
                                        let type_name = db.struct_name(record.struct_index as usize).unwrap_or("Unknown");
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("[R]").strong().color(Color32::from_rgb(100, 180, 255)));
                                            ui.label(RichText::new(name).monospace().color(Color32::from_rgb(100, 180, 255)));
                                            ui.label(RichText::new(format!("({})", type_name)).color(Color32::from_gray(120)).small());
                                        });
                                        ui.separator();
                                    }
                                }
                            }

                            let outgoing_count = state.record_references.len();
                            let incoming_count = state.incoming_references.len();
                            let has_outgoing = outgoing_count > 0;
                            let has_incoming = incoming_count > 0;
                            let refs_panel_height = 120.0;
                            let content_height = (panel_height - refs_panel_height - 60.0).max(100.0);

                            egui::Frame::none()
                                .fill(Color32::from_gray(25))
                                .show(ui, |ui| {
                                    ui.set_min_height(content_height);
                                    ui.set_max_height(content_height);

                                    if state.record_xml.is_empty() {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(RichText::new("Select a record to view its contents").color(Color32::from_gray(100)));
                                        });
                                    } else {
                                        render_text_with_line_numbers(ui, &state.record_xml, "dcb_xml_scroll");
                                    }
                                });

                            ui.add_space(8.0);

                            let mut navigate_to_idx: Option<usize> = None;

                            ui.horizontal(|ui| {
                                let half_width = (ui.available_width() / 2.0 - 8.0).max(100.0);

                                egui::Frame::none()
                                    .fill(Color32::from_gray(25))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(half_width);
                                        ui.set_height(refs_panel_height);

                                        ui.vertical(|ui| {
                                            let header_text = format!("Incoming{}", if has_incoming { format!(" ({})", incoming_count) } else { String::new() });
                                            let header_color = if has_incoming { Color32::from_rgb(255, 180, 150) } else { Color32::from_gray(100) };
                                            ui.label(RichText::new(header_text).strong().color(header_color));

                                            ui.add_space(4.0);

                                            if has_incoming {
                                                ScrollArea::vertical()
                                                    .id_salt("dcb_incoming_scroll")
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        for (i, ref_info) in state.incoming_references.iter().enumerate() {
                                                            let bg = if i % 2 == 0 { Color32::from_gray(25) } else { Color32::from_gray(26) };
                                                            egui::Frame::none().fill(bg).inner_margin(2.0).show(ui, |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let (badge, color) = match ref_info.ref_type {
                                                                        ReferenceType::Reference => ("R", Color32::from_rgb(100, 200, 100)),
                                                                        ReferenceType::StrongPointer => ("P", Color32::from_rgb(200, 150, 100)),
                                                                        ReferenceType::WeakPointer => ("W", Color32::from_rgb(150, 150, 200)),
                                                                    };
                                                                    ui.label(RichText::new(badge).color(color).monospace());
                                                                    let resp = ui.add(egui::Label::new(
                                                                        RichText::new(&ref_info.source_name).color(Color32::from_rgb(255, 180, 100))
                                                                    ).sense(Sense::click()).truncate());
                                                                    if resp.clicked() { navigate_to_idx = Some(ref_info.source_record_index); }
                                                                    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                                                    resp.on_hover_text(format!("{}\n.{}", ref_info.source_type, ref_info.property_name));
                                                                });
                                                            });
                                                        }
                                                    });
                                            } else {
                                                ui.label(RichText::new("none").color(Color32::from_gray(60)).italics());
                                            }
                                        });
                                    });

                                let sep_rect = ui.available_rect_before_wrap();
                                ui.painter().vline(
                                    sep_rect.left() + 4.0,
                                    sep_rect.top()..=sep_rect.bottom(),
                                    egui::Stroke::new(1.0, Color32::from_gray(50))
                                );
                                ui.add_space(8.0);

                                egui::Frame::none()
                                    .fill(Color32::from_gray(25))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(half_width);
                                        ui.set_height(refs_panel_height);

                                        ui.vertical(|ui| {
                                            let header_text = format!("Outgoing{}", if has_outgoing { format!(" ({})", outgoing_count) } else { String::new() });
                                            let header_color = if has_outgoing { Color32::from_rgb(200, 180, 255) } else { Color32::from_gray(100) };
                                            ui.label(RichText::new(header_text).strong().color(header_color));

                                            ui.add_space(4.0);

                                            if has_outgoing {
                                                ScrollArea::vertical()
                                                    .id_salt("dcb_outgoing_scroll")
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        for (i, ref_info) in state.record_references.iter().enumerate() {
                                                            let bg = if i % 2 == 0 { Color32::from_gray(25) } else { Color32::from_gray(26) };
                                                            egui::Frame::none().fill(bg).inner_margin(2.0).show(ui, |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let (badge, color) = match ref_info.ref_type {
                                                                        ReferenceType::Reference => ("R", Color32::from_rgb(100, 200, 100)),
                                                                        ReferenceType::StrongPointer => ("P", Color32::from_rgb(200, 150, 100)),
                                                                        ReferenceType::WeakPointer => ("W", Color32::from_rgb(150, 150, 200)),
                                                                    };
                                                                    ui.label(RichText::new(badge).color(color).monospace());
                                                                    ui.label(RichText::new(&ref_info.property_name).color(Color32::from_gray(140)));
                                                                    ui.label(RichText::new("->").color(Color32::from_gray(80)));

                                                                    if let Some(target_idx) = ref_info.target_record_index {
                                                                        let resp = ui.add(egui::Label::new(
                                                                            RichText::new(&ref_info.target_name).color(Color32::from_rgb(100, 180, 255))
                                                                        ).sense(Sense::click()).truncate());
                                                                        if resp.clicked() { navigate_to_idx = Some(target_idx); }
                                                                        if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                                                        resp.on_hover_text(&ref_info.target_type);
                                                                    } else {
                                                                        ui.label(RichText::new(&ref_info.target_name).color(Color32::from_gray(100)));
                                                                    }
                                                                });
                                                            });
                                                        }
                                                    });
                                            } else {
                                                ui.label(RichText::new("none").color(Color32::from_gray(60)).italics());
                                            }
                                        });
                                    });
                            });

                            if let Some(idx) = navigate_to_idx {
                                Self::navigate_to_record(state, idx);
                            }
                        });
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.label(RichText::new("[DCB]").size(48.0).color(Color32::from_gray(80)));
                            ui.add_space(20.0);
                            ui.label(
                                RichText::new("No DataCore loaded")
                                    .size(20.0)
                                    .color(Color32::from_gray(150))
                            );
                            ui.add_space(10.0);
                            if state.p4k_archive.is_some() {
                                ui.spinner();
                                ui.label(
                                    RichText::new("Loading from P4K archive...")
                                        .color(Color32::from_gray(100))
                                );
                            } else {
                                ui.label(
                                    RichText::new("Load a P4K archive first, or open a standalone DCB file")
                                        .color(Color32::from_gray(100))
                                );
                            }
                        });
                    });
                }
            }
            DataCorePage::Structs => {
                if state.datacore_type_tree.is_some() && state.datacore.is_some() {
                    let panel_height = ui.available_height();
                    let available_width = ui.available_width();
                    let tree_width = (available_width * 0.35).max(200.0);

                    ui.columns(2, |columns| {
                        columns[0].set_max_width(tree_width);

                        let rect = columns[0].available_rect_before_wrap();
                        columns[0].painter().vline(
                            rect.right() + 4.0,
                            rect.top()..=rect.bottom(),
                            egui::Stroke::new(2.0, Color32::from_gray(55))
                        );

                        let mut navigate_to_struct: Option<usize> = None;
                        let mut navigate_to_enum: Option<usize> = None;

                        let mut clicked_struct_from_tree: Option<usize> = None;
                        ScrollArea::vertical()
                            .id_salt("dcb_type_tree_scroll")
                            .auto_shrink([false, false])
                            .show(&mut columns[0], |ui| {
                                if let Some(tree) = &mut state.datacore_type_tree {
                                    let search = state.datacore_search.to_lowercase();
                                    let selected = state.selected_type;

                                    if !search.is_empty() {
                                        for child in &mut tree.children {
                                            check_type_expand_for_search(child, &search);
                                        }
                                    }

                                    let mut row_index = 0usize;
                                    for child in &mut tree.children {
                                        render_type_tree(
                                            ui,
                                            child,
                                            &search,
                                            selected,
                                            0,
                                            &mut row_index,
                                            &mut clicked_struct_from_tree,
                                        );
                                    }
                                }
                            });

                        // Handle click from tree
                        if let Some(idx) = clicked_struct_from_tree {
                            navigate_to_struct = Some(idx);
                        }

                        columns[1].vertical(|ui| {
                            // Header with selected struct name
                            if let Some(struct_idx) = state.selected_type {
                                if let Some(db) = &state.datacore {
                                    let name = db.struct_name(struct_idx).unwrap_or("Unknown");
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("[S]").strong().color(Color32::from_rgb(180, 220, 140)));
                                        ui.label(RichText::new(name).monospace().color(Color32::from_rgb(180, 220, 140)));
                                    });
                                    ui.separator();
                                }
                            }

                            let outgoing_count = state.struct_outgoing_refs.len();
                            let incoming_count = state.struct_incoming_refs.len();
                            let has_outgoing = outgoing_count > 0;
                            let has_incoming = incoming_count > 0;
                            let refs_panel_height = 120.0;
                            let content_height = (panel_height - refs_panel_height - 60.0).max(100.0);

                            egui::Frame::none()
                                .fill(Color32::from_gray(25))
                                .show(ui, |ui| {
                                    ui.set_min_height(content_height);
                                    ui.set_max_height(content_height);

                                    if state.type_preview.is_empty() {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                RichText::new("Select a struct to view its layout")
                                                    .color(Color32::from_gray(100)),
                                            );
                                        });
                                    } else {
                                        render_text_with_line_numbers(ui, &state.type_preview, "dcb_type_preview_scroll");
                                    }
                                });

                            ui.add_space(8.0);

                            // References panel (same layout as records)
                            ui.horizontal(|ui| {
                                let half_width = (ui.available_width() / 2.0 - 8.0).max(100.0);

                                // Incoming references (structs that reference this struct)
                                egui::Frame::none()
                                    .fill(Color32::from_gray(25))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(half_width);
                                        ui.set_height(refs_panel_height);

                                        ui.vertical(|ui| {
                                            let header_text = format!("Used By{}", if has_incoming { format!(" ({})", incoming_count) } else { String::new() });
                                            let header_color = if has_incoming { Color32::from_rgb(255, 180, 150) } else { Color32::from_gray(100) };
                                            ui.label(RichText::new(header_text).strong().color(header_color));

                                            ui.add_space(4.0);

                                            if has_incoming {
                                                ScrollArea::vertical()
                                                    .id_salt("dcb_struct_incoming_scroll")
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        for (i, ref_info) in state.struct_incoming_refs.iter().enumerate() {
                                                            let bg = if i % 2 == 0 { Color32::from_gray(25) } else { Color32::from_gray(26) };
                                                            egui::Frame::none().fill(bg).inner_margin(2.0).show(ui, |ui| {
                                                                ui.horizontal(|ui| {
                                                                    ui.label(RichText::new("[S]").color(Color32::from_rgb(180, 220, 140)).monospace().small());
                                                                    let resp = ui.add(egui::Label::new(
                                                                        RichText::new(&ref_info.source_name).color(Color32::from_rgb(255, 180, 100))
                                                                    ).sense(Sense::click()).truncate());
                                                                    if resp.clicked() { navigate_to_struct = Some(ref_info.source_index); }
                                                                    if resp.hovered() { ui.ctx().set_cursor_icon(CursorIcon::PointingHand); }
                                                                    let fields_str = ref_info.property_names.iter()
                                                                        .map(|s| format!(".{}", s))
                                                                        .collect::<Vec<_>>()
                                                                        .join(", ");
                                                                    resp.on_hover_text(&fields_str);
                                                                    // Show field count if multiple
                                                                    if ref_info.property_names.len() > 1 {
                                                                        ui.label(RichText::new(format!("({} fields)", ref_info.property_names.len()))
                                                                            .color(Color32::from_gray(100)).small());
                                                                    } else if let Some(field) = ref_info.property_names.first() {
                                                                        ui.label(RichText::new(format!(".{}", field))
                                                                            .color(Color32::from_gray(100)).small());
                                                                    }
                                                                });
                                                            });
                                                        }
                                                    });
                                            } else {
                                                ui.label(RichText::new("none").color(Color32::from_gray(60)).italics());
                                            }
                                        });
                                    });

                                let sep_rect = ui.available_rect_before_wrap();
                                ui.painter().vline(
                                    sep_rect.left() + 4.0,
                                    sep_rect.top()..=sep_rect.bottom(),
                                    egui::Stroke::new(1.0, Color32::from_gray(50))
                                );
                                ui.add_space(8.0);

                                // Outgoing references (structs/enums referenced by this struct)
                                egui::Frame::none()
                                    .fill(Color32::from_gray(25))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(half_width);
                                        ui.set_height(refs_panel_height);

                                        ui.vertical(|ui| {
                                            let header_text = format!("References{}", if has_outgoing { format!(" ({})", outgoing_count) } else { String::new() });
                                            let header_color = if has_outgoing { Color32::from_rgb(200, 180, 255) } else { Color32::from_gray(100) };
                                            ui.label(RichText::new(header_text).strong().color(header_color));

                                            ui.add_space(4.0);

                                            if has_outgoing {
                                                ScrollArea::vertical()
                                                    .id_salt("dcb_struct_outgoing_scroll")
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        for (i, ref_info) in state.struct_outgoing_refs.iter().enumerate() {
                                                            let bg = if i % 2 == 0 { Color32::from_gray(25) } else { Color32::from_gray(26) };
                                                            egui::Frame::none().fill(bg).inner_margin(2.0).show(ui, |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let (badge, badge_color, target_name, target_idx) = match &ref_info.target_type {
                                                                        StructRefTarget::Struct { name, index } => ("[S]", Color32::from_rgb(180, 220, 140), name.clone(), Some((*index, false))),
                                                                        StructRefTarget::Enum { name, index } => ("[E]", Color32::from_rgb(220, 180, 120), name.clone(), Some((*index, true))),
                                                                    };
                                                                    ui.label(RichText::new(badge).color(badge_color).monospace().small());
                                                                    ui.label(RichText::new(&ref_info.property_name).color(Color32::from_gray(140)));
                                                                    ui.label(RichText::new("->").color(Color32::from_gray(80)));

                                                                    if let Some((idx, is_enum)) = target_idx {
                                                                        let resp = ui.add(egui::Label::new(
                                                                            RichText::new(&target_name).color(Color32::from_rgb(100, 180, 255))
                                                                        ).sense(Sense::click()).truncate());
                                                                        if resp.clicked() {
                                                                            if is_enum {
                                                                                navigate_to_enum = Some(idx);
                                                                            } else {
                                                                                navigate_to_struct = Some(idx);
                                                                            }
                                                                        }
                                                                        if resp.hovered() { ui.ctx().set_cursor_icon(CursorIcon::PointingHand); }
                                                                    }
                                                                });
                                                            });
                                                        }
                                                    });
                                            } else {
                                                ui.label(RichText::new("none").color(Color32::from_gray(60)).italics());
                                            }
                                        });
                                    });
                            });
                        });

                        // Handle navigation
                        if let Some(idx) = navigate_to_struct {
                            Self::navigate_to_struct(state, idx);
                        }
                        if let Some(idx) = navigate_to_enum {
                            Self::navigate_to_enum(state, idx);
                        }
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.label(RichText::new("[Types]").size(32.0).color(Color32::from_gray(80)));
                            ui.add_space(10.0);
                            ui.label(RichText::new("No DataCore loaded").color(Color32::from_gray(150)));
                        });
                    });
                }
            }
            DataCorePage::Enums => {
                if state.datacore.is_some() {
                    let panel_height = ui.available_height();
                    let available_width = ui.available_width();
                    let list_width = (available_width * 0.35).max(200.0);

                    ui.columns(2, |columns| {
                        columns[0].set_max_width(list_width);

                        let rect = columns[0].available_rect_before_wrap();
                        columns[0].painter().vline(
                            rect.right() + 4.0,
                            rect.top()..=rect.bottom(),
                            egui::Stroke::new(2.0, Color32::from_gray(55))
                        );

                        let mut navigate_to_struct: Option<usize> = None;
                        let mut clicked_enum: Option<usize> = None;

                        ScrollArea::vertical()
                            .id_salt("dcb_enum_list_scroll")
                            .auto_shrink([false, false])
                            .show(&mut columns[0], |ui| {
                                if let Some(db) = &state.datacore {
                                    let search = state.datacore_search.to_lowercase();
                                    for (idx, _def) in db.enum_definitions().iter().enumerate() {
                                        let name = db.enum_name(idx).unwrap_or("Unknown");
                                        if !search.is_empty() && !name.to_lowercase().contains(&search) {
                                            continue;
                                        }
                                        let is_selected = state.selected_enum == Some(idx);

                                        ui.horizontal(|ui| {
                                            ui.add_space(16.0);
                                            ui.label(RichText::new("[E]").color(Color32::from_rgb(220, 180, 120)).small().monospace());
                                            let text = RichText::new(name)
                                                .monospace()
                                                .color(if is_selected { Color32::from_rgb(100, 180, 255) } else { Color32::from_gray(200) });
                                            let resp = ui.add(egui::Label::new(text).sense(Sense::click()).truncate())
                                                .on_hover_cursor(CursorIcon::Default);
                                            if resp.clicked() {
                                                clicked_enum = Some(idx);
                                            }
                                        });
                                    }
                                }
                            });

                        // Handle enum click from list
                        if let Some(idx) = clicked_enum {
                            Self::navigate_to_enum(state, idx);
                        }

                        columns[1].vertical(|ui| {
                            // Header with selected enum name
                            if let Some(enum_idx) = state.selected_enum {
                                if let Some(db) = &state.datacore {
                                    let name = db.enum_name(enum_idx).unwrap_or("Unknown");
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("[E]").strong().color(Color32::from_rgb(220, 180, 120)));
                                        ui.label(RichText::new(name).monospace().color(Color32::from_rgb(220, 180, 120)));
                                    });
                                    ui.separator();
                                }
                            }

                            let incoming_count = state.enum_incoming_refs.len();
                            let has_incoming = incoming_count > 0;
                            let refs_panel_height = 120.0;
                            let content_height = (panel_height - refs_panel_height - 60.0).max(100.0);

                            egui::Frame::none()
                                .fill(Color32::from_gray(25))
                                .show(ui, |ui| {
                                    ui.set_min_height(content_height);
                                    ui.set_max_height(content_height);

                                    if state.enum_preview.is_empty() {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                RichText::new("Select an enum to view its values")
                                                    .color(Color32::from_gray(100)),
                                            );
                                        });
                                    } else {
                                        render_text_with_line_numbers(ui, &state.enum_preview, "dcb_enum_preview_scroll");
                                    }
                                });

                            ui.add_space(8.0);

                            // Incoming references panel
                            egui::Frame::none()
                                .fill(Color32::from_gray(25))
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_height(refs_panel_height);

                                    ui.vertical(|ui| {
                                        let header_text = format!("Used By{}", if has_incoming { format!(" ({})", incoming_count) } else { String::new() });
                                        let header_color = if has_incoming { Color32::from_rgb(255, 180, 150) } else { Color32::from_gray(100) };
                                        ui.label(RichText::new(header_text).strong().color(header_color));

                                        ui.add_space(4.0);

                                        if has_incoming {
                                            ScrollArea::vertical()
                                                .id_salt("dcb_enum_incoming_scroll")
                                                .auto_shrink([false, false])
                                                .show(ui, |ui| {
                                                    for (i, ref_info) in state.enum_incoming_refs.iter().enumerate() {
                                                        let bg = if i % 2 == 0 { Color32::from_gray(25) } else { Color32::from_gray(26) };
                                                        egui::Frame::none().fill(bg).inner_margin(2.0).show(ui, |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.label(RichText::new("[S]").color(Color32::from_rgb(180, 220, 140)).monospace().small());
                                                                let resp = ui.add(egui::Label::new(
                                                                    RichText::new(&ref_info.source_name).color(Color32::from_rgb(255, 180, 100))
                                                                ).sense(Sense::click()).truncate());
                                                                if resp.clicked() { navigate_to_struct = Some(ref_info.source_index); }
                                                                if resp.hovered() { ui.ctx().set_cursor_icon(CursorIcon::PointingHand); }
                                                                let fields_str = ref_info.property_names.iter()
                                                                    .map(|s| format!(".{}", s))
                                                                    .collect::<Vec<_>>()
                                                                    .join(", ");
                                                                resp.on_hover_text(&fields_str);
                                                                if ref_info.property_names.len() > 1 {
                                                                    ui.label(RichText::new(format!("({} fields)", ref_info.property_names.len()))
                                                                        .color(Color32::from_gray(100)).small());
                                                                } else if let Some(field) = ref_info.property_names.first() {
                                                                    ui.label(RichText::new(format!(".{}", field))
                                                                        .color(Color32::from_gray(100)).small());
                                                                }
                                                            });
                                                        });
                                                    }
                                                });
                                        } else {
                                            ui.label(RichText::new("none").color(Color32::from_gray(60)).italics());
                                        }
                                    });
                                });
                        });

                        // Handle navigation
                        if let Some(idx) = navigate_to_struct {
                            Self::navigate_to_struct(state, idx);
                        }
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.label(RichText::new("[Enums]").size(32.0).color(Color32::from_gray(80)));
                            ui.add_space(10.0);
                            ui.label(RichText::new("No DataCore loaded").color(Color32::from_gray(150)));
                        });
                    });
                }
            }
        }
    }

    fn handle_navigation(ui: &mut Ui, state: &mut AppState) {
        let ctx = ui.ctx();

        // Mouse back/forward buttons
        if ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra1)) {
            Self::navigate_back(state);
        }
        if ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra2)) {
            Self::navigate_forward(state);
        }

        // Alt+Left/Right for navigation
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(Key::ArrowLeft)) {
            Self::navigate_back(state);
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(Key::ArrowRight)) {
            Self::navigate_forward(state);
        }
    }

    fn navigate_back(state: &mut AppState) {
        if state.navigation_index > 0 {
            state.navigation_index -= 1;
            let entry = state.navigation_history[state.navigation_index];
            Self::load_entry_without_history(state, entry);
        }
    }

    fn navigate_forward(state: &mut AppState) {
        if state.navigation_index + 1 < state.navigation_history.len() {
            state.navigation_index += 1;
            let entry = state.navigation_history[state.navigation_index];
            Self::load_entry_without_history(state, entry);
        }
    }

    fn navigate_to(state: &mut AppState, entry: NavigationEntry) {
        // Don't add duplicate if it's the same as current
        if let Some(&current) = state.navigation_history.get(state.navigation_index) {
            if current == entry {
                return;
            }
        }

        // Truncate forward history if we're not at the end
        if !state.navigation_history.is_empty() && state.navigation_index + 1 < state.navigation_history.len() {
            state.navigation_history.truncate(state.navigation_index + 1);
        }

        // Add to history
        state.navigation_history.push(entry);
        state.navigation_index = state.navigation_history.len() - 1;

        Self::load_entry_without_history(state, entry);
    }

    fn navigate_to_record(state: &mut AppState, idx: usize) {
        Self::navigate_to(state, NavigationEntry::Record(idx));
    }

    fn navigate_to_struct(state: &mut AppState, idx: usize) {
        Self::navigate_to(state, NavigationEntry::Struct(idx));
    }

    fn navigate_to_enum(state: &mut AppState, idx: usize) {
        Self::navigate_to(state, NavigationEntry::Enum(idx));
    }

    fn load_entry_without_history(state: &mut AppState, entry: NavigationEntry) {
        match entry {
            NavigationEntry::Record(idx) => {
                state.datacore_page = DataCorePage::Records;
                Self::load_record_without_history(state, idx);
            }
            NavigationEntry::Struct(idx) => {
                state.datacore_page = DataCorePage::Structs;
                Self::load_struct_without_history(state, idx);
            }
            NavigationEntry::Enum(idx) => {
                state.datacore_page = DataCorePage::Enums;
                Self::load_enum_without_history(state, idx);
            }
        }
    }

    fn load_record_without_history(state: &mut AppState, idx: usize) {
        state.selected_record = Some(idx);
        state.selected_line = None;

        if let Some(db) = &state.datacore {
            let records: Vec<_> = db.main_records().collect();
            if let Some(record) = records.get(idx) {
                // Generate XML with 4-space indentation
                match svarog::datacore::XmlExporter::new(db).export_record(record) {
                    Ok(xml) => {
                        // Convert 2-space to 4-space indentation
                        state.record_xml = xml.lines()
                            .map(|line| {
                                let spaces = line.len() - line.trim_start().len();
                                let indent = "    ".repeat(spaces / 2);
                                format!("{}{}", indent, line.trim_start())
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                    Err(e) => state.record_xml = format!("Error: {}", e),
                }
                state.record_references = extract_references(db, record, &state.reference_index);

                // Extract incoming references from the index
                state.incoming_references = extract_incoming_references(db, idx, &state.reference_index, &records);
            }
        }
    }

    fn load_struct_without_history(state: &mut AppState, idx: usize) {
        state.selected_type = Some(idx);

        if let Some(db) = &state.datacore {
            state.type_preview = generate_struct_preview(db, idx);
            state.struct_outgoing_refs = extract_struct_outgoing_refs(db, idx);
            if let Some(struct_ref_idx) = &state.struct_reference_index {
                state.struct_incoming_refs = struct_ref_idx.incoming.get(&idx).cloned().unwrap_or_default();
            } else {
                state.struct_incoming_refs.clear();
            }
        }
    }

    fn load_enum_without_history(state: &mut AppState, idx: usize) {
        state.selected_enum = Some(idx);

        if let Some(db) = &state.datacore {
            state.enum_preview = generate_enum_preview(db, idx);
            if let Some(struct_ref_idx) = &state.struct_reference_index {
                state.enum_incoming_refs = struct_ref_idx.enum_incoming.get(&idx).cloned().unwrap_or_default();
            } else {
                state.enum_incoming_refs.clear();
            }
        }
    }

    fn load_datacore_from_p4k(state: &mut AppState) {
        if let Some(archive) = &state.p4k_archive {
            let dcb_names = ["Data/Game.dcb", "Data/Game2.dcb", "Game.dcb", "Game2.dcb"];
            let mut found = None;

            for name in &dcb_names {
                if let Some(entry) = archive.find(name) {
                    match archive.read(&entry) {
                        Ok(data) => {
                            found = Some(data);
                            break;
                        }
                        Err(_) => continue,
                    }
                }
            }

            if let Some(data) = found {
                state.datacore_loading = true;
                worker::load_datacore(data, state.worker_sender.clone());
            }
        }
    }
}

fn generate_enum_preview(db: &svarog::datacore::DataCoreDatabase, enum_index: usize) -> String {
    let exporter = svarog::datacore::CHeaderExporter::new(db);
    exporter.generate_enum_preview(enum_index)
}

/// Render text content with line numbers and text selection support
fn render_text_with_line_numbers(ui: &mut Ui, text: &str, scroll_id: &str) {
    let text = text.trim_end();
    let line_count = text.lines().count();
    let num_width = format!("{}", line_count).len().max(2);
    let line_num_col_width = (num_width as f32 + 1.0) * 8.0;

    ScrollArea::both()
        .id_salt(scroll_id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                // Reserve space for line numbers column
                let line_num_x = ui.cursor().min.x + line_num_col_width - 4.0;
                ui.add_space(line_num_col_width + 12.0);

                // Text content (selectable)
                let mut text_copy = text.to_owned();
                let output = egui::TextEdit::multiline(&mut text_copy)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .interactive(true)
                    .frame(false)
                    .text_color(Color32::from_gray(200))
                    .show(ui);

                // Use galley_pos - the exact screen position where galley is drawn
                let galley_pos = output.galley_pos;
                let galley = &output.galley;
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());

                for (i, row) in galley.rows.iter().enumerate() {
                    if i >= line_count {
                        break;
                    }
                    let row_y = galley_pos.y + row.rect.center().y;

                    ui.painter().text(
                        egui::pos2(line_num_x, row_y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{}", i + 1),
                        font_id.clone(),
                        Color32::from_gray(80),
                    );
                }

                // Separator line
                ui.painter().vline(
                    line_num_x + 8.0,
                    galley_pos.y..=galley_pos.y + galley.rect.height(),
                    egui::Stroke::new(1.0, Color32::from_gray(50))
                );
            });
        });
}

/// Expand type tree nodes when search matches
fn check_type_expand_for_search(node: &mut DataCoreTypeNode, search: &str) -> bool {
    let self_matches = node.name.to_lowercase().contains(search);
    let mut any_child_matches = false;

    for child in &mut node.children {
        if check_type_expand_for_search(child, search) {
            any_child_matches = true;
        }
    }

    if any_child_matches {
        node.expanded = true;
    }

    self_matches || any_child_matches
}

fn type_node_matches(node: &DataCoreTypeNode, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    node.name.to_lowercase().contains(search)
        || node.children.iter().any(|c| type_node_matches(c, search))
}

fn render_type_tree(
    ui: &mut Ui,
    node: &mut DataCoreTypeNode,
    search: &str,
    selected: Option<usize>,
    depth: usize,
    row_index: &mut usize,
    clicked_struct: &mut Option<usize>,
) {
    let show_node = if search.is_empty() {
        true
    } else {
        type_node_matches(node, search)
    };

    if !show_node {
        return;
    }

    let is_selected = node.struct_index.is_some()
        && selected.map_or(false, |idx| Some(idx) == node.struct_index);

    *row_index += 1;

    ui.horizontal(|ui| {
        let indent = depth as f32 * 32.0;
        if depth > 0 {
            let rect = ui.available_rect_before_wrap();
            for d in 0..depth {
                let x = rect.left() + (d as f32 * 32.0) + 8.0;
                ui.painter().line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0, Color32::from_gray(60)),
                );
            }
        }
        ui.add_space(indent);

        // Expand/collapse triangle
        let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
        response = response.on_hover_cursor(CursorIcon::Default);
        if response.clicked() && !node.children.is_empty() {
            node.expanded = !node.expanded;
        }
        let center = rect.center();
        let size = 5.0;
        let color = if response.hovered() {
            Color32::WHITE
        } else {
            Color32::from_gray(180)
        };
        if !node.children.is_empty() {
            if node.expanded {
                let points = vec![
                    egui::pos2(center.x - size, center.y - size * 0.5),
                    egui::pos2(center.x + size, center.y - size * 0.5),
                    egui::pos2(center.x, center.y + size * 0.5),
                ];
                ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
            } else {
                let points = vec![
                    egui::pos2(center.x - size * 0.5, center.y - size),
                    egui::pos2(center.x + size * 0.5, center.y),
                    egui::pos2(center.x - size * 0.5, center.y + size),
                ];
                ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
            }
        }

        ui.label(RichText::new("[S]").color(Color32::from_rgb(180, 220, 140)).small().monospace());

        let label_text = RichText::new(&node.name)
            .monospace()
            .color(if is_selected { Color32::from_rgb(100, 180, 255) } else { Color32::from_gray(200) });
        let resp = ui.add(egui::Label::new(label_text).sense(Sense::click()).truncate())
            .on_hover_cursor(CursorIcon::Default);
        if resp.clicked() {
            if let Some(idx) = node.struct_index {
                *clicked_struct = Some(idx);
            }
        }
    });

    if node.expanded {
        for child in &mut node.children {
            render_type_tree(ui, child, search, selected, depth + 1, row_index, clicked_struct);
        }
    }
}

fn generate_struct_preview(db: &svarog::datacore::DataCoreDatabase, struct_index: usize) -> String {
    let exporter = svarog::datacore::CHeaderExporter::new(db);
    exporter.generate_struct_preview(struct_index)
}

/// Extract outgoing type references from a struct's properties
fn extract_struct_outgoing_refs(
    db: &svarog::datacore::DataCoreDatabase,
    struct_index: usize,
) -> Vec<StructTypeReference> {
    use svarog::datacore::DataType;

    let mut refs = Vec::new();
    let struct_defs = db.struct_definitions();
    let prop_defs = db.property_definitions();

    if struct_index >= struct_defs.len() {
        return refs;
    }

    let struct_def = &struct_defs[struct_index];
    let first_attr = struct_def.first_attribute_index as usize;
    let attr_count = struct_def.attribute_count as usize;

    for prop_idx in first_attr..(first_attr + attr_count) {
        if prop_idx >= prop_defs.len() {
            break;
        }
        let prop = &prop_defs[prop_idx];
        let prop_name = db.property_name(prop).unwrap_or("unknown").to_string();

        let data_type = DataType::from_u16(prop.data_type);
        let conv_type = DataType::from_u16(prop.conversion_type);

        match (data_type, conv_type) {
            (Some(DataType::Class), _) |
            (Some(DataType::StrongPointer), _) |
            (Some(DataType::WeakPointer), _) |
            (Some(DataType::Reference), _) => {
                let target_struct = prop.struct_index as usize;
                if target_struct < struct_defs.len() {
                    let target_name = db.struct_name(target_struct).unwrap_or("Unknown").to_string();
                    refs.push(StructTypeReference {
                        property_name: prop_name,
                        target_type: StructRefTarget::Struct { name: target_name, index: target_struct },
                        is_array: false,
                    });
                }
            }
            (Some(DataType::EnumChoice), _) | (_, Some(DataType::EnumChoice)) => {
                let target_enum = prop.struct_index as usize;
                if target_enum < db.enum_definitions().len() {
                    let target_name = db.enum_name(target_enum).unwrap_or("Unknown").to_string();
                    refs.push(StructTypeReference {
                        property_name: prop_name,
                        target_type: StructRefTarget::Enum { name: target_name, index: target_enum },
                        is_array: false,
                    });
                }
            }
            _ => {}
        }
    }

    refs
}

/// Extract references from a record's properties
fn extract_references(
    db: &Arc<svarog::datacore::DataCoreDatabase>,
    record: &svarog::datacore::structs::DataCoreRecord,
    reference_index: &Option<Arc<ReferenceIndex>>,
) -> Vec<RecordReference> {
    use svarog::datacore::Value;

    let mut refs = Vec::new();
    let instance = db.instance(record.struct_index as u32, record.instance_index as u32);

    // Build a map of record indices for quick lookup
    let main_records: Vec<_> = db.main_records().collect();

    // Use the GUID map from reference index if available, otherwise build a local one
    let guid_map: std::collections::HashMap<String, usize> = if let Some(ref_idx) = reference_index {
        ref_idx.guid_to_index.clone()
    } else {
        main_records.iter().enumerate()
            .map(|(idx, r)| (format!("{}", r.id), idx))
            .collect()
    };

    // Build instance map for pointer lookups
    let instance_map: std::collections::HashMap<(u32, u32), usize> = main_records.iter().enumerate()
        .map(|(idx, r)| ((r.struct_index as u32, r.instance_index as u32), idx))
        .collect();

    for prop in instance.properties() {
        match &prop.value {
            Value::Reference(Some(record_ref)) => {
                let guid_str = format!("{}", record_ref.guid);
                let target_idx = guid_map.get(&guid_str).copied();

                if let Some(idx) = target_idx {
                    let target_record = main_records[idx];
                    let target_name = db.record_name(target_record).unwrap_or("Unknown").to_string();
                    let target_type = db.struct_name(target_record.struct_index as usize).unwrap_or("Unknown").to_string();

                    refs.push(RecordReference {
                        property_name: prop.name.to_string(),
                        ref_type: ReferenceType::Reference,
                        target_name,
                        target_type,
                        target_guid: guid_str,
                        target_record_index: Some(idx),
                    });
                } else {
                    // Record not found - show GUID
                    refs.push(RecordReference {
                        property_name: prop.name.to_string(),
                        ref_type: ReferenceType::Reference,
                        target_name: guid_str.clone(),
                        target_type: "Unknown (not in DB)".to_string(),
                        target_guid: guid_str,
                        target_record_index: None,
                    });
                }
            }
            Value::StrongPointer(Some(instance_ref)) => {
                let ptr_struct_index = instance_ref.struct_index;
                let ptr_instance_index = instance_ref.instance_index;

                let target_type = db.struct_name(ptr_struct_index as usize).unwrap_or("Unknown").to_string();
                let target_idx = instance_map.get(&(ptr_struct_index, ptr_instance_index)).copied();

                let target_name = if let Some(idx) = target_idx {
                    db.record_name(main_records[idx]).unwrap_or("Unknown").to_string()
                } else {
                    format!("{}[{}]", target_type, ptr_instance_index)
                };

                refs.push(RecordReference {
                    property_name: prop.name.to_string(),
                    ref_type: ReferenceType::StrongPointer,
                    target_name,
                    target_type: target_type.clone(),
                    target_guid: format!("struct:{} instance:{}", ptr_struct_index, ptr_instance_index),
                    target_record_index: target_idx,
                });
            }
            Value::WeakPointer(Some(instance_ref)) => {
                let ptr_struct_index = instance_ref.struct_index;
                let ptr_instance_index = instance_ref.instance_index;

                let target_type = db.struct_name(ptr_struct_index as usize).unwrap_or("Unknown").to_string();
                let target_idx = instance_map.get(&(ptr_struct_index, ptr_instance_index)).copied();

                let target_name = if let Some(idx) = target_idx {
                    db.record_name(main_records[idx]).unwrap_or("Unknown").to_string()
                } else {
                    format!("{}[{}]", target_type, ptr_instance_index)
                };

                refs.push(RecordReference {
                    property_name: prop.name.to_string(),
                    ref_type: ReferenceType::WeakPointer,
                    target_name,
                    target_type: target_type.clone(),
                    target_guid: format!("struct:{} instance:{}", ptr_struct_index, ptr_instance_index),
                    target_record_index: target_idx,
                });
            }
            Value::Array(array_ref) => {
                use svarog::datacore::ArrayElementType;
                // Handle reference arrays
                if array_ref.count > 0 && array_ref.count < 1_000_000 {
                    match array_ref.element_type {
                        ArrayElementType::Reference => {
                            // Try to expand array and show individual references
                            let items = expand_reference_array(db, array_ref, &guid_map, &main_records);
                            if items.is_empty() {
                                refs.push(RecordReference {
                                    property_name: prop.name.to_string(),
                                    ref_type: ReferenceType::Reference,
                                    target_name: format!("[{} refs]", array_ref.count),
                                    target_type: "Array<Reference>".to_string(),
                                    target_guid: String::new(),
                                    target_record_index: None,
                                });
                            } else {
                                for (i, (name, type_name, idx)) in items.into_iter().enumerate() {
                                    refs.push(RecordReference {
                                        property_name: format!("{}[{}]", prop.name, i),
                                        ref_type: ReferenceType::Reference,
                                        target_name: name,
                                        target_type: type_name,
                                        target_guid: String::new(),
                                        target_record_index: idx,
                                    });
                                }
                            }
                        }
                        ArrayElementType::StrongPointer | ArrayElementType::WeakPointer => {
                            let ref_type = if array_ref.element_type == ArrayElementType::StrongPointer {
                                ReferenceType::StrongPointer
                            } else {
                                ReferenceType::WeakPointer
                            };
                            let type_name = db.struct_name(array_ref.struct_index as usize).unwrap_or("Unknown");

                            // Try to expand pointer arrays (up to 10 items)
                            if array_ref.count <= 10 {
                                let items = expand_pointer_array(db, array_ref, &instance_map, &main_records);
                                if !items.is_empty() {
                                    for (i, (name, type_name, idx)) in items.into_iter().enumerate() {
                                        refs.push(RecordReference {
                                            property_name: format!("{}[{}]", prop.name, i),
                                            ref_type,
                                            target_name: name,
                                            target_type: type_name,
                                            target_guid: String::new(),
                                            target_record_index: idx,
                                        });
                                    }
                                } else {
                                    refs.push(RecordReference {
                                        property_name: prop.name.to_string(),
                                        ref_type,
                                        target_name: format!("[{} x {}]", array_ref.count, type_name),
                                        target_type: format!("Array<{}>", type_name),
                                        target_guid: String::new(),
                                        target_record_index: None,
                                    });
                                }
                            } else {
                                refs.push(RecordReference {
                                    property_name: prop.name.to_string(),
                                    ref_type,
                                    target_name: format!("[{} x {}]", array_ref.count, type_name),
                                    target_type: format!("Array<{}>", type_name),
                                    target_guid: String::new(),
                                    target_record_index: None,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    refs
}

/// Extract incoming references from the reference index
fn extract_incoming_references(
    db: &Arc<svarog::datacore::DataCoreDatabase>,
    target_idx: usize,
    reference_index: &Option<std::sync::Arc<ReferenceIndex>>,
    records: &[&svarog::datacore::structs::DataCoreRecord],
) -> Vec<IncomingReference> {
    let mut incoming = Vec::new();

    let Some(index) = reference_index else { return incoming };

    if let Some(refs) = index.incoming.get(&target_idx) {
        for (source_idx, property_name, ref_type) in refs {
            if let Some(source_record) = records.get(*source_idx) {
                let source_name = db.record_name(source_record).unwrap_or("Unknown").to_string();
                let source_type = db.struct_name(source_record.struct_index as usize).unwrap_or("Unknown").to_string();

                incoming.push(IncomingReference {
                    source_name,
                    source_type,
                    property_name: property_name.clone(),
                    ref_type: *ref_type,
                    source_record_index: *source_idx,
                });
            }
        }
    }

    incoming
}

/// Expand a reference array to get individual items
fn expand_reference_array(
    db: &Arc<svarog::datacore::DataCoreDatabase>,
    array_ref: &svarog::datacore::ArrayRef,
    guid_map: &std::collections::HashMap<String, usize>,
    main_records: &[&svarog::datacore::structs::DataCoreRecord],
) -> Vec<(String, String, Option<usize>)> {
    let mut items = Vec::new();

    // Only expand small arrays to avoid performance issues
    if array_ref.count > 10 {
        return items;
    }

    for i in 0..array_ref.count {
        let idx = array_ref.first_index as usize + i as usize;
        if let Some(ref_val) = db.reference_value(idx) {
            let guid_str = format!("{}", ref_val.record_id);
            if let Some(&target_idx) = guid_map.get(&guid_str) {
                let target_record = main_records[target_idx];
                let target_name = db.record_name(target_record).unwrap_or("Unknown").to_string();
                let target_type = db.struct_name(target_record.struct_index as usize).unwrap_or("Unknown").to_string();
                items.push((target_name, target_type, Some(target_idx)));
            } else {
                items.push((guid_str, "Unknown".to_string(), None));
            }
        }
    }

    items
}

/// Expand a pointer array to get individual items
/// Pointers point to instances (struct_index, instance_index), not records
/// We try to find records that point to those instances
fn expand_pointer_array(
    db: &Arc<svarog::datacore::DataCoreDatabase>,
    array_ref: &svarog::datacore::ArrayRef,
    instance_map: &std::collections::HashMap<(u32, u32), usize>,
    main_records: &[&svarog::datacore::structs::DataCoreRecord],
) -> Vec<(String, String, Option<usize>)> {
    use svarog::datacore::ArrayElementType;

    let mut items = Vec::new();

    // Only expand small arrays
    if array_ref.count > 10 {
        return items;
    }

    let struct_idx = array_ref.struct_index;
    let type_name = db.struct_name(struct_idx as usize).unwrap_or("Unknown").to_string();

    for i in 0..array_ref.count {
        let idx = array_ref.first_index as usize + i as usize;

        // Get the pointer based on type
        let ptr = match array_ref.element_type {
            ArrayElementType::StrongPointer => db.strong_value(idx),
            ArrayElementType::WeakPointer => db.weak_value(idx),
            _ => None,
        };

        if let Some(ptr) = ptr {
            // Copy fields from packed struct to avoid alignment issues
            let ptr_instance_index = ptr.instance_index;
            let key = (ptr.struct_index as u32, ptr_instance_index as u32);
            let target_idx = instance_map.get(&key).copied();

            if let Some(record_idx) = target_idx {
                let record = main_records[record_idx];
                let name = db.record_name(record).unwrap_or("Unknown").to_string();
                items.push((name, type_name.clone(), Some(record_idx)));
            } else {
                // Instance not directly associated with a record
                items.push((
                    format!("{}[{}]", type_name, ptr_instance_index),
                    type_name.clone(),
                    None,
                ));
            }
        }
    }

    items
}

/// Check if node or any children match search and type filter, auto-expand if needed
fn check_and_expand_for_search(node: &mut DataCoreRecordNode, search: &str, type_filter: Option<&str>) -> bool {
    let self_matches = matches_filters(node, search, type_filter);
    let mut any_child_matches = false;

    for child in &mut node.children {
        if check_and_expand_for_search(child, search, type_filter) {
            any_child_matches = true;
        }
    }

    if any_child_matches && node.is_folder {
        node.expanded = true;
    }

    self_matches || any_child_matches
}

fn matches_filters(node: &DataCoreRecordNode, search: &str, type_filter: Option<&str>) -> bool {
    if let Some(tf) = type_filter {
        if !node.is_folder && node.type_name != tf {
            return false;
        }
    }

    if search.is_empty() {
        return true;
    }

    node.name.to_lowercase().contains(search)
        || node.type_name.to_lowercase().contains(search)
        || node.id.to_lowercase().contains(search)
}

#[allow(clippy::too_many_arguments)]
fn render_record_tree(
    ui: &mut Ui,
    node: &mut DataCoreRecordNode,
    search: &str,
    type_filter: Option<&str>,
    selected: &mut Option<usize>,
    record_xml: &mut String,
    record_refs: &mut Vec<RecordReference>,
    db: Option<Arc<svarog::datacore::DataCoreDatabase>>,
    depth: usize,
    row_index: &mut usize,
    new_type_filter: &mut Option<String>,
    navigate_to: &mut Option<usize>,
) {
    let show_node = if search.is_empty() && type_filter.is_none() {
        true
    } else {
        matches_filters(node, search, type_filter)
            || node.children.iter().any(|c| node_matches_search(c, search, type_filter))
    };

    if !show_node {
        return;
    }

    let is_selected = !node.is_folder
        && node.record_index.is_some()
        && selected.as_ref().map_or(false, |s| Some(*s) == node.record_index);

    *row_index += 1;

    ui.horizontal(|ui| {
                let indent = depth as f32 * 16.0;
                if depth > 0 {
                    let rect = ui.available_rect_before_wrap();
                    for d in 0..depth {
                        let x = rect.left() + (d as f32 * 16.0) + 8.0;
                        ui.painter().line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(1.0, Color32::from_gray(60)),
                        );
                    }
                }
                ui.add_space(indent);

                // Expand/collapse triangle
                if node.is_folder && !node.children.is_empty() {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());

                    if response.clicked() {
                        node.expanded = !node.expanded;
                    }

                    let center = rect.center();
                    let size = 5.0;
                    let color = if response.hovered() {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(180)
                    };

                    if node.expanded {
                        let points = vec![
                            egui::pos2(center.x - size, center.y - size * 0.5),
                            egui::pos2(center.x + size, center.y - size * 0.5),
                            egui::pos2(center.x, center.y + size * 0.5),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
                    } else {
                        let points = vec![
                            egui::pos2(center.x - size * 0.5, center.y - size),
                            egui::pos2(center.x + size * 0.5, center.y),
                            egui::pos2(center.x - size * 0.5, center.y + size),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
                    }
                } else {
                    ui.add_space(16.0);
                }

                // Icon - use text characters that render reliably
                let (icon, icon_color) = if node.is_folder {
                    ("[D]", Color32::from_rgb(255, 200, 100))
                } else if node.has_references {
                    ("[*]", Color32::from_rgb(180, 160, 220))
                } else {
                    ("[R]", Color32::from_rgb(150, 200, 255))
                };
                ui.label(RichText::new(icon).color(icon_color).small().monospace());

                // Name
                let name_color = if is_selected {
                    Color32::from_rgb(100, 180, 255)
                } else if !search.is_empty() && node.name.to_lowercase().contains(search) {
                    Color32::from_rgb(255, 220, 100)
                } else {
                    Color32::from_gray(220)
                };

                let name_text = RichText::new(&node.name).color(name_color);
                let name_response = ui.selectable_label(is_selected, name_text);

                if name_response.clicked() {
                    if node.is_folder {
                        node.expanded = !node.expanded;
                    } else if let Some(idx) = node.record_index {
                        *navigate_to = Some(idx);
                    }
                }

                // Type for records (right-aligned, clickable)
                if !node.is_folder {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let type_text = RichText::new(&node.type_name)
                            .color(Color32::from_gray(100))
                            .small();

                        let type_response = ui.add(
                            egui::Label::new(type_text).sense(Sense::click())
                        );

                        if type_response.clicked() {
                            *new_type_filter = Some(node.type_name.clone());
                        }

                        if type_response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

                        type_response.on_hover_text("Click to filter by this type");
                    });
                }
    });

    if node.expanded && node.is_folder {
        for child in &mut node.children {
            render_record_tree(
                ui,
                child,
                search,
                type_filter,
                selected,
                record_xml,
                record_refs,
                db.clone(),
                depth + 1,
                row_index,
                new_type_filter,
                navigate_to,
            );
        }
    }
}

fn node_matches_search(node: &DataCoreRecordNode, search: &str, type_filter: Option<&str>) -> bool {
    if matches_filters(node, search, type_filter) {
        return true;
    }
    node.children.iter().any(|c| node_matches_search(c, search, type_filter))
}
fn export_current(db: &Arc<svarog::datacore::DataCoreDatabase>, state: &mut AppState) -> Result<(), String> {
    let dialog = rfd::FileDialog::new();
    match state.datacore_page {
        DataCorePage::Structs => {
            if state.type_preview.is_empty() {
                return Err("No struct selected".into());
            }
            if let Some(path) = dialog.set_file_name("struct.txt").save_file() {
                std::fs::write(&path, &state.type_preview).map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        }
        DataCorePage::Enums => {
            if state.enum_preview.is_empty() {
                return Err("No enum selected".into());
            }
            if let Some(path) = dialog.set_file_name("enum.txt").save_file() {
                std::fs::write(&path, &state.enum_preview).map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        }
        DataCorePage::Records => {
            if let Some(record_idx) = state.selected_record {
                let records: Vec<_> = db.main_records().collect();
                if let Some(record) = records.get(record_idx) {
                    let file_name = db.record_file_name(record).unwrap_or("record.xml");
                    let suggested = file_name.replace('/', "_").replace('\\', "_");
                    let xml = svarog::datacore::XmlExporter::new(db)
                        .export_record(record)
                        .map_err(|e| e.to_string())?;
                    if let Some(path) = dialog.set_file_name(&suggested).save_file() {
                        std::fs::write(&path, xml).map_err(|e| e.to_string())
                    } else {
                        Ok(())
                    }
                } else {
                    Err("No record selected".into())
                }
            } else {
                Err("No record selected".into())
            }
        }
    }
}

fn export_all(db: &Arc<svarog::datacore::DataCoreDatabase>, state: &mut AppState) -> Result<(), String> {
    match state.datacore_page {
        DataCorePage::Structs => {
            let exporter = svarog::datacore::CHeaderExporter::new(db);
            let buf = exporter.export_all();
            if let Some(path) = rfd::FileDialog::new().set_file_name("structs.h").save_file() {
                std::fs::write(&path, buf).map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        }
        DataCorePage::Enums => {
            let mut buf = String::new();
            for idx in 0..db.enum_definitions().len() {
                buf.push_str(&generate_enum_preview(db, idx));
                buf.push('\n');
            }
            if let Some(path) = rfd::FileDialog::new().set_file_name("enums.txt").save_file() {
                std::fs::write(&path, buf).map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        }
        DataCorePage::Records => {
            let exporter = svarog::datacore::XmlExporter::new(db);
            if let Some(dir) = rfd::FileDialog::new().set_directory(".").pick_folder() {
                let records: Vec<_> = db.main_records().collect();
                for record in records {
                    let file_name = db
                        .record_file_name(record)
                        .unwrap_or("record.xml")
                        .replace('/', std::path::MAIN_SEPARATOR_STR);
                    let path = dir.join(file_name).with_extension("xml");
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let xml = exporter.export_record(record).map_err(|e| e.to_string())?;
                    std::fs::write(&path, xml).map_err(|e| e.to_string())?;
                }
                Ok(())
            } else {
                Ok(())
            }
        }
    }
}

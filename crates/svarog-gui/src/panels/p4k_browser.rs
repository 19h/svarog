//! P4K archive browser panel

use eframe::egui::{self, Color32, RichText, ScrollArea, Sense, Ui, Vec2};
use std::sync::Arc;

use svarog_common::DiffStatus;

use crate::preview::render_preview;
use crate::state::{AppState, FileTreeNode};
use crate::widgets::{format_size, progress_bar, render_diff_view, search_box};
use crate::worker;

/// Text-based file type icon
fn text_file_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".xml") || lower.ends_with(".mtl") || lower.ends_with(".cdf") {
        "[X]" // XML
    } else if lower.ends_with(".dds") || lower.ends_with(".png") || lower.ends_with(".jpg") {
        "[I]" // Image
    } else if lower.ends_with(".socpak") {
        "[P]" // Package
    } else if lower.ends_with(".dcb") {
        "[B]" // Binary database
    } else if lower.ends_with(".chf") {
        "[C]" // Character
    } else if lower.ends_with(".lua") || lower.ends_with(".cfg") {
        "[S]" // Script/config
    } else {
        "[F]" // File
    }
}

pub struct P4kBrowserPanel;

impl P4kBrowserPanel {
    pub fn show(ui: &mut Ui, state: &mut AppState) {
        // Top toolbar
        ui.horizontal(|ui| {
            if ui.button("Open P4K").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("P4K Archive", &["p4k"])
                    .pick_file()
                {
                    state.p4k_loading = true;
                    state.p4k_path = Some(path.clone());
                    worker::load_p4k(path, state.worker_sender.clone());
                }
            }

            if state.p4k_archive.is_some() {
                ui.separator();

                if ui.button("Extract...").clicked() {
                    state.extraction_dialog_open = true;
                }

                ui.separator();

                // Compare button
                if state.p4k_comparison.is_none() && !state.p4k_comparing {
                    if ui.button("Compare...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("P4K Archive", &["p4k"])
                            .set_title("Select P4K to compare against")
                            .pick_file()
                        {
                            state.p4k_comparing = true;
                            if let Some(old_path) = &state.p4k_path {
                                worker::compare_p4k(
                                    old_path.clone(),
                                    path,
                                    state.worker_sender.clone(),
                                );
                            }
                        }
                    }
                }

                ui.separator();

                // File filter
                search_box(ui, &mut state.file_filter, "Filter files...");

                ui.separator();

                // Stats
                if let Some(archive) = &state.p4k_archive {
                    ui.label(format!("{} files", archive.entry_count()));
                }
            }

            if let Some(path) = &state.p4k_path {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(path.file_name().unwrap_or_default().to_string_lossy())
                            .color(Color32::LIGHT_BLUE),
                    );
                });
            }
        });

        ui.separator();

        // Loading state
        if state.p4k_loading {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.spinner();
                progress_bar(
                    ui,
                    state.p4k_load_progress.0,
                    state.p4k_load_progress.1,
                    &state.p4k_load_progress.2,
                );
            });
            return;
        }

        // Comparison loading state
        if state.p4k_comparing {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.spinner();
                ui.label("Comparing archives...");
            });
            return;
        }

        // Comparison view
        if state.p4k_comparison.is_some() {
            Self::show_comparison_view(ui, state);
            return;
        }

        // Main content area with split view
        if state.file_tree.is_some() {
            ui.columns(2, |columns| {
                // Left panel: File tree
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(&mut columns[0], |ui| {
                        if let Some(tree) = &mut state.file_tree {
                            let filter = state.file_filter.to_lowercase();
                            let selected = &mut state.selected_file;
                            let archive = state.p4k_archive.clone();
                            let sender = state.worker_sender.clone();
                            let preview_loading = &mut state.preview_loading;

                            // Auto-expand matching paths when filtering
                            if !filter.is_empty() {
                                for child in &mut tree.children {
                                    check_and_expand_for_filter(child, &filter);
                                }
                            }

                            // Skip the root node and render its children directly
                            let mut row_index = 0usize;
                            for child in &mut tree.children {
                                render_tree_node(
                                    ui,
                                    child,
                                    &filter,
                                    selected,
                                    archive.clone(),
                                    sender.clone(),
                                    preview_loading,
                                    0,
                                    &mut row_index,
                                );
                            }
                        }
                    });

                // Right panel: Preview
                columns[1].vertical(|ui| {
                    // Preview header
                    if let Some(selected) = &state.selected_file {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(text_file_icon(selected))
                                    .monospace()
                                    .color(Color32::from_gray(150)),
                            );
                            ui.label(
                                RichText::new(selected)
                                    .monospace()
                                    .color(Color32::LIGHT_BLUE),
                            );
                        });
                        ui.separator();
                    }

                    render_preview(ui, &state.preview, state.preview_loading);
                });
            });
        } else {
            // Empty state
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label(
                        RichText::new("[P4K]")
                            .size(48.0)
                            .color(Color32::from_gray(80)),
                    );
                    ui.add_space(20.0);
                    ui.label(RichText::new("No P4K archive loaded").size(20.0));
                    ui.add_space(10.0);
                    ui.label("Click 'Open P4K' to browse a Star Citizen archive");
                });
            });
        }
    }

    /// Show comparison view between two P4K archives
    fn show_comparison_view(ui: &mut Ui, state: &mut AppState) {
        // Extract data we need before any mutable operations
        let (added_count, removed_count, modified_count, old_name, new_name) = {
            let comparison = state.p4k_comparison.as_ref().unwrap();
            (
                comparison.result.added.len(),
                comparison.result.removed.len(),
                comparison.result.modified.len(),
                comparison
                    .old_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                comparison
                    .new_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            )
        };

        let mut exit_compare = false;

        // Comparison toolbar
        ui.horizontal(|ui| {
            if ui.button("Exit Compare").clicked() {
                exit_compare = true;
            }

            ui.separator();

            search_box(ui, &mut state.p4k_comparison_filter, "Filter changes...");

            ui.separator();

            // Summary stats
            ui.label(
                RichText::new(format!("+{}", added_count))
                    .color(Color32::from_rgb(100, 200, 100))
                    .monospace(),
            );
            ui.label(
                RichText::new(format!("-{}", removed_count))
                    .color(Color32::from_rgb(200, 100, 100))
                    .monospace(),
            );
            ui.label(
                RichText::new(format!("~{}", modified_count))
                    .color(Color32::from_rgb(200, 200, 100))
                    .monospace(),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} vs {}", old_name, new_name))
                        .color(Color32::LIGHT_BLUE),
                );
            });
        });

        if exit_compare {
            state.p4k_comparison = None;
            state.p4k_comparison_filter.clear();
            return;
        }

        ui.separator();

        // Split view: left = changed files, right = diff
        ui.columns(2, |columns| {
            // Left panel: Changed files list
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(&mut columns[0], |ui| {
                    let filter = state.p4k_comparison_filter.to_lowercase();

                    // Get comparison data we need before mutable borrow
                    let comparison = state.p4k_comparison.as_ref().unwrap();
                    let added: Vec<_> = comparison
                        .result
                        .added
                        .iter()
                        .filter(|f| filter.is_empty() || f.path.to_lowercase().contains(&filter))
                        .map(|f| (f.path.clone(), f.status.clone(), f.size_new))
                        .collect();
                    let removed: Vec<_> = comparison
                        .result
                        .removed
                        .iter()
                        .filter(|f| filter.is_empty() || f.path.to_lowercase().contains(&filter))
                        .map(|f| (f.path.clone(), f.status.clone(), f.size_old))
                        .collect();
                    let modified: Vec<_> = comparison
                        .result
                        .modified
                        .iter()
                        .filter(|f| filter.is_empty() || f.path.to_lowercase().contains(&filter))
                        .map(|f| (f.path.clone(), f.status.clone(), f.size_new))
                        .collect();

                    let mut row_index = 0usize;

                    // Added files
                    if !added.is_empty() {
                        ui.label(
                            RichText::new(format!("Added ({})", added.len()))
                                .color(Color32::from_rgb(100, 200, 100))
                                .strong(),
                        );
                        for (path, status, size) in &added {
                            render_comparison_item(
                                ui,
                                path,
                                status,
                                *size,
                                state,
                                &mut row_index,
                            );
                        }
                        ui.add_space(8.0);
                    }

                    // Removed files
                    if !removed.is_empty() {
                        ui.label(
                            RichText::new(format!("Removed ({})", removed.len()))
                                .color(Color32::from_rgb(200, 100, 100))
                                .strong(),
                        );
                        for (path, status, size) in &removed {
                            render_comparison_item(
                                ui,
                                path,
                                status,
                                *size,
                                state,
                                &mut row_index,
                            );
                        }
                        ui.add_space(8.0);
                    }

                    // Modified files
                    if !modified.is_empty() {
                        ui.label(
                            RichText::new(format!("Modified ({})", modified.len()))
                                .color(Color32::from_rgb(200, 200, 100))
                                .strong(),
                        );
                        for (path, status, size) in &modified {
                            render_comparison_item(
                                ui,
                                path,
                                status,
                                *size,
                                state,
                                &mut row_index,
                            );
                        }
                    }
                });

            // Right panel: Diff view
            columns[1].vertical(|ui| {
                let comparison = state.p4k_comparison.as_ref().unwrap();

                if let Some(selected) = &comparison.selected_path {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(text_file_icon(selected))
                                .monospace()
                                .color(Color32::from_gray(150)),
                        );
                        ui.label(
                            RichText::new(selected)
                                .monospace()
                                .color(Color32::LIGHT_BLUE),
                        );
                    });
                    ui.separator();

                    if comparison.diff_loading {
                        ui.spinner();
                        ui.label("Loading diff...");
                    } else if let Some(diff) = &comparison.current_diff {
                        render_diff_view(ui, diff);
                    } else {
                        ui.label("Select a text file to view diff");
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Select a file to view changes");
                    });
                }
            });
        });
    }
}

/// Render a single comparison item row
fn render_comparison_item(
    ui: &mut Ui,
    path: &str,
    status: &DiffStatus,
    size: Option<u64>,
    state: &mut AppState,
    row_index: &mut usize,
) {
    let comparison = state.p4k_comparison.as_ref().unwrap();
    let is_selected = comparison
        .selected_path
        .as_ref()
        .map_or(false, |s| s == path);

    // Alternating row background
    let row_bg = if *row_index % 2 == 0 {
        Color32::TRANSPARENT
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 1)
    };
    *row_index += 1;

    let (prefix, color) = match status {
        DiffStatus::Added => ("+", Color32::from_rgb(100, 200, 100)),
        DiffStatus::Removed => ("-", Color32::from_rgb(200, 100, 100)),
        DiffStatus::Modified => ("~", Color32::from_rgb(200, 200, 100)),
        DiffStatus::Unchanged => (" ", Color32::from_gray(180)),
    };

    egui::Frame::none()
        .fill(row_bg)
        .inner_margin(egui::Margin::symmetric(4.0, 2.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(prefix).monospace().color(color));

                // Just the filename
                let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let name_color = if is_selected {
                    Color32::from_rgb(100, 180, 255)
                } else {
                    color
                };

                let response = ui.selectable_label(is_selected, RichText::new(filename).color(name_color));

                if response.clicked() {
                    let path_owned = path.to_string();
                    let comparison = state.p4k_comparison.as_mut().unwrap();
                    comparison.selected_path = Some(path_owned.clone());

                    // Trigger diff generation for modified files
                    if matches!(status, DiffStatus::Modified) {
                        comparison.diff_loading = true;
                        comparison.current_diff = None;
                        worker::generate_file_diff(
                            comparison.old_archive.clone(),
                            comparison.new_archive.clone(),
                            path_owned,
                            state.worker_sender.clone(),
                        );
                    } else if matches!(status, DiffStatus::Added) {
                        // For added files, generate a diff showing all lines as added
                        comparison.diff_loading = true;
                        comparison.current_diff = None;
                        worker::generate_file_diff(
                            comparison.old_archive.clone(),
                            comparison.new_archive.clone(),
                            path_owned,
                            state.worker_sender.clone(),
                        );
                    } else if matches!(status, DiffStatus::Removed) {
                        // For removed files, generate a diff showing all lines as removed
                        comparison.diff_loading = true;
                        comparison.current_diff = None;
                        worker::generate_file_diff(
                            comparison.old_archive.clone(),
                            comparison.new_archive.clone(),
                            path_owned,
                            state.worker_sender.clone(),
                        );
                    }
                }

                // Size and full path on hover
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(sz) = size {
                        ui.label(
                            RichText::new(format_size(sz))
                                .color(Color32::from_gray(120))
                                .small(),
                        );
                    }
                });
            });
        })
        .response
        .on_hover_text(path);
}

/// Check if node or any children match filter, and auto-expand if needed
fn check_and_expand_for_filter(node: &mut FileTreeNode, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }

    let self_matches = node.name.to_lowercase().contains(filter);
    let mut any_child_matches = false;

    for child in &mut node.children {
        if check_and_expand_for_filter(child, filter) {
            any_child_matches = true;
        }
    }

    // Auto-expand if any child matches
    if any_child_matches && node.is_directory {
        node.expanded = true;
    }

    self_matches || any_child_matches
}

fn render_tree_node(
    ui: &mut Ui,
    node: &mut FileTreeNode,
    filter: &str,
    selected: &mut Option<String>,
    archive: Option<Arc<svarog::p4k::P4kArchive>>,
    sender: crossbeam_channel::Sender<crate::state::WorkerMessage>,
    preview_loading: &mut bool,
    depth: usize,
    row_index: &mut usize,
) {
    // Filter check - skip non-matching nodes
    if !filter.is_empty() {
        let matches = node.name.to_lowercase().contains(filter)
            || node.children.iter().any(|c| node_matches_filter(c, filter));
        if !matches {
            return;
        }
    }

    let is_selected = selected.as_ref().map_or(false, |s| s == &node.path);

    // Alternating row background (very subtle)
    let row_bg = if *row_index % 2 == 0 {
        Color32::TRANSPARENT
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 1) // 75% less opaque
    };
    *row_index += 1;

    // Row frame with hover effect
    egui::Frame::none()
        .fill(row_bg)
        .inner_margin(egui::Margin::symmetric(4.0, 2.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Indentation with visual guide line
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
                if node.is_directory && !node.children.is_empty() {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());

                    if response.clicked() {
                        node.expanded = !node.expanded;
                    }

                    // Draw triangle
                    let center = rect.center();
                    let size = 5.0;
                    let color = if response.hovered() {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(180)
                    };

                    if node.expanded {
                        // Down triangle
                        let points = vec![
                            egui::pos2(center.x - size, center.y - size * 0.5),
                            egui::pos2(center.x + size, center.y - size * 0.5),
                            egui::pos2(center.x, center.y + size * 0.5),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(
                            points,
                            color,
                            egui::Stroke::NONE,
                        ));
                    } else {
                        // Right triangle
                        let points = vec![
                            egui::pos2(center.x - size * 0.5, center.y - size),
                            egui::pos2(center.x + size * 0.5, center.y),
                            egui::pos2(center.x - size * 0.5, center.y + size),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(
                            points,
                            color,
                            egui::Stroke::NONE,
                        ));
                    }
                } else {
                    ui.add_space(16.0);
                }

                // Icon with color (text-based icons)
                let (icon, icon_color) = if node.is_directory {
                    ("[D]", Color32::from_rgb(255, 200, 100))
                } else {
                    (text_file_icon(&node.name), Color32::from_gray(180))
                };
                ui.label(RichText::new(icon).color(icon_color).monospace().small());

                // Name
                let name_color = if is_selected {
                    Color32::from_rgb(100, 180, 255)
                } else if !filter.is_empty() && node.name.to_lowercase().contains(filter) {
                    Color32::from_rgb(255, 220, 100) // Highlight matching text
                } else {
                    Color32::from_gray(220)
                };

                let name_text = RichText::new(&node.name).color(name_color);
                let name_response = ui.selectable_label(is_selected, name_text);

                if name_response.clicked() {
                    if node.is_directory {
                        node.expanded = !node.expanded;
                    } else {
                        *selected = Some(node.path.clone());
                        if let (Some(archive), Some(idx)) = (&archive, node.entry_index) {
                            *preview_loading = true;
                            worker::load_preview(archive.clone(), idx, sender.clone());
                        }
                    }
                }

                // Size for files (right-aligned)
                if !node.is_directory {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if node.is_encrypted {
                            ui.label(
                                RichText::new("[E]")
                                    .small()
                                    .monospace()
                                    .color(Color32::from_rgb(255, 150, 150)),
                            );
                        }
                        ui.label(
                            RichText::new(format_size(node.size))
                                .color(Color32::from_gray(120))
                                .small(),
                        );
                    });
                }
            });
        });

    // Render children if expanded
    if node.expanded && node.is_directory {
        for child in &mut node.children {
            render_tree_node(
                ui,
                child,
                filter,
                selected,
                archive.clone(),
                sender.clone(),
                preview_loading,
                depth + 1,
                row_index,
            );
        }
    }
}

fn node_matches_filter(node: &FileTreeNode, filter: &str) -> bool {
    if node.name.to_lowercase().contains(filter) {
        return true;
    }
    node.children.iter().any(|c| node_matches_filter(c, filter))
}

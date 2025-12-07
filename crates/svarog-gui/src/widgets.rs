//! Reusable UI widgets

#![allow(dead_code)]

use eframe::egui::{self, Color32, Response, RichText, Ui, WidgetText};

/// Format bytes into human-readable size
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Render a progress bar with label
pub fn progress_bar(ui: &mut Ui, current: usize, total: usize, label: &str) {
    let progress = if total > 0 {
        current as f32 / total as f32
    } else {
        0.0
    };

    ui.horizontal(|ui| {
        ui.add(egui::ProgressBar::new(progress).show_percentage());
        if !label.is_empty() {
            ui.label(label);
        }
    });
}

/// Render a search box with clear button
pub fn search_box(ui: &mut Ui, search: &mut String, placeholder: &str) -> Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new("?").monospace().color(Color32::from_gray(120)));
        let response = ui.add(
            egui::TextEdit::singleline(search)
                .hint_text(placeholder)
                .desired_width(200.0)
        );
        if !search.is_empty() && ui.button("x").clicked() {
            search.clear();
        }
        response
    }).inner
}

/// Render a tree node with expand/collapse
pub fn tree_node<R>(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: impl Into<WidgetText>,
    expanded: &mut bool,
    is_leaf: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(id);

    ui.horizontal(|ui| {
        // Expand/collapse button for non-leaf nodes
        if is_leaf {
            ui.add_space(18.0); // Space where button would be
        } else {
            let symbol = if *expanded { "▼" } else { "▶" };
            if ui.small_button(symbol).clicked() {
                *expanded = !*expanded;
            }
        }

        ui.label(label);
    });

    if *expanded && !is_leaf {
        ui.indent(id, |ui| {
            Some(add_contents(ui))
        }).inner
    } else {
        None
    }
}

/// Icon for file/folder (text-based)
pub fn file_icon(is_directory: bool, is_encrypted: bool) -> &'static str {
    if is_directory {
        "[D]"
    } else if is_encrypted {
        "[E]"
    } else {
        "[F]"
    }
}

/// File type icon based on extension (text-based)
pub fn file_type_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".xml") || lower.ends_with(".mtl") || lower.ends_with(".cdf") {
        "[X]"
    } else if lower.ends_with(".dds") || lower.ends_with(".png") || lower.ends_with(".jpg") {
        "[I]"
    } else if lower.ends_with(".socpak") {
        "[P]"
    } else if lower.ends_with(".dcb") {
        "[B]"
    } else if lower.ends_with(".chf") {
        "[C]"
    } else if lower.ends_with(".lua") || lower.ends_with(".cfg") {
        "[S]"
    } else {
        "[F]"
    }
}

/// Error toast notification
pub fn error_toast(ui: &mut Ui, message: &str) {
    egui::Frame::none()
        .fill(Color32::from_rgb(100, 30, 30))
        .inner_margin(8.0)
        .rounding(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("[!]").color(Color32::YELLOW));
                ui.label(RichText::new(message).color(Color32::WHITE));
            });
        });
}

/// Confirmation dialog
pub fn confirmation_dialog(
    ctx: &egui::Context,
    title: &str,
    message: &str,
    open: &mut bool,
) -> Option<bool> {
    let mut result = None;
    let mut should_close = false;

    if *open {
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        result = Some(false);
                        should_close = true;
                    }
                    if ui.button("Confirm").clicked() {
                        result = Some(true);
                        should_close = true;
                    }
                });
            });

        if should_close {
            *open = false;
        }
    }

    result
}

/// Styled button for primary actions
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(
        RichText::new(text).strong()
    ))
}

/// Styled button for secondary actions
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.button(text)
}

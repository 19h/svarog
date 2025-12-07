//! File preview rendering

use eframe::egui::{self, Color32, RichText, ScrollArea, TextEdit, TextStyle, Ui};

use crate::state::PreviewData;

/// Render a file preview
pub fn render_preview(ui: &mut Ui, preview: &PreviewData, loading: bool) {
    if loading {
        ui.centered_and_justified(|ui| {
            ui.spinner();
            ui.label("Loading preview...");
        });
        return;
    }

    match preview {
        PreviewData::None => {
            ui.centered_and_justified(|ui| {
                ui.label("Select a file to preview");
            });
        }
        PreviewData::Text(text) => {
            render_text_preview(ui, text);
        }
        PreviewData::Hex { data, offset } => {
            render_hex_preview(ui, data, *offset);
        }
        PreviewData::Image(data) => {
            render_image_preview(ui, data);
        }
    }
}

fn render_text_preview(ui: &mut Ui, text: &str) {
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Use monospace font for code/XML
            let mut text_edit = text.to_string();
            ui.add(
                TextEdit::multiline(&mut text_edit)
                    .font(TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .interactive(false)
            );
        });
}

fn render_hex_preview(ui: &mut Ui, data: &[u8], _offset: usize) {
    const BYTES_PER_LINE: usize = 16;

    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.style_mut().override_text_style = Some(TextStyle::Monospace);

            // Header
            ui.horizontal(|ui| {
                ui.label(RichText::new("Offset    ").color(Color32::GRAY));
                for i in 0..BYTES_PER_LINE {
                    ui.label(RichText::new(format!("{:02X} ", i)).color(Color32::GRAY));
                }
                ui.label(RichText::new(" ASCII").color(Color32::GRAY));
            });
            ui.separator();

            // Limit display to prevent UI lag
            let max_lines = 1000;
            let display_bytes = data.len().min(max_lines * BYTES_PER_LINE);

            for line_start in (0..display_bytes).step_by(BYTES_PER_LINE) {
                let line_end = (line_start + BYTES_PER_LINE).min(data.len());
                let line_data = &data[line_start..line_end];

                ui.horizontal(|ui| {
                    // Offset column
                    ui.label(RichText::new(format!("{:08X}  ", line_start)).color(Color32::LIGHT_BLUE));

                    // Hex bytes
                    for byte in line_data {
                        let color = if *byte == 0 {
                            Color32::DARK_GRAY
                        } else if byte.is_ascii_alphanumeric() {
                            Color32::LIGHT_GREEN
                        } else {
                            Color32::WHITE
                        };
                        ui.label(RichText::new(format!("{:02X} ", byte)).color(color));
                    }

                    // Padding for incomplete lines
                    for _ in line_data.len()..BYTES_PER_LINE {
                        ui.label("   ");
                    }

                    // ASCII column
                    ui.label(" ");
                    let ascii: String = line_data
                        .iter()
                        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' })
                        .collect();
                    ui.label(RichText::new(ascii).color(Color32::LIGHT_GRAY));
                });
            }

            if data.len() > display_bytes {
                ui.label(RichText::new(format!(
                    "\n... and {} more bytes",
                    data.len() - display_bytes
                )).color(Color32::YELLOW));
            }
        });
}

fn render_image_preview(ui: &mut Ui, data: &[u8]) {
    // Try to load and display the image
    match image::load_from_memory(data) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let pixels = rgba.into_raw();

            let texture = ui.ctx().load_texture(
                "preview_image",
                egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
                egui::TextureOptions::LINEAR,
            );

            ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.image(&texture);

                    ui.label(format!(
                        "{}x{} pixels",
                        size[0], size[1]
                    ));
                });
        }
        Err(e) => {
            ui.label(format!("Failed to load image: {}", e));
        }
    }
}

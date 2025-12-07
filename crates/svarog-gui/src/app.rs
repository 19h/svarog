//! Main application

use eframe::egui::{self, RichText};

use crate::panels::{DataCoreBrowserPanel, ExtractionDialog, P4kBrowserPanel};
use crate::state::{ActiveTab, AppState};
use crate::widgets::error_toast;

pub struct SvarogApp {
    state: AppState,
}

impl SvarogApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            state: AppState::new(),
        }
    }
}

impl eframe::App for SvarogApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process background worker messages
        self.state.process_messages();

        // Request repaint if we have active operations
        if self.state.p4k_loading
            || self.state.datacore_loading
            || self.state.extracting
            || self.state.preview_loading
        {
            ctx.request_repaint();
        }

        // Top menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open P4K...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("P4K Archive", &["p4k"])
                            .pick_file()
                        {
                            self.state.p4k_loading = true;
                            self.state.p4k_path = Some(path.clone());
                            crate::worker::load_p4k(path, self.state.worker_sender.clone());
                        }
                        ui.close_menu();
                    }

                    if ui.button("Open DCB...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("DataCore Database", &["dcb"])
                            .pick_file()
                        {
                            match std::fs::read(&path) {
                                Ok(data) => {
                                    self.state.datacore_loading = true;
                                    crate::worker::load_datacore(data, self.state.worker_sender.clone());
                                }
                                Err(e) => {
                                    self.state.show_error(format!("Failed to read file: {}", e));
                                }
                            }
                        }
                        ui.close_menu();
                    }

                    ui.separator();

                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        // Could show about dialog
                        ui.close_menu();
                    }
                });
            });
        });

        // Tab bar
        egui::TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.state.active_tab,
                    ActiveTab::P4kBrowser,
                    RichText::new("[P4K] Browser").size(14.0),
                );
                ui.selectable_value(
                    &mut self.state.active_tab,
                    ActiveTab::DataCoreBrowser,
                    RichText::new("[DCB] DataCore").size(14.0),
                );
            });
        });

        // Status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Error toast
                if let Some(error) = &self.state.error_message {
                    error_toast(ui, error);
                } else {
                    // Status info
                    if let Some(path) = &self.state.p4k_path {
                        ui.label(format!("P4K: {}", path.display()));
                    }

                    if self.state.p4k_loading {
                        ui.separator();
                        ui.spinner();
                        ui.label("Loading P4K...");
                    }

                    if self.state.datacore_loading {
                        ui.separator();
                        ui.spinner();
                        ui.label("Loading DataCore...");
                    }

                    if self.state.extracting {
                        ui.separator();
                        ui.spinner();
                        ui.label(format!(
                            "Extracting: {} / {}",
                            self.state.extraction_progress.0,
                            self.state.extraction_progress.1
                        ));
                    }
                }
            });
        });

        // Main content area
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state.active_tab {
                ActiveTab::P4kBrowser => P4kBrowserPanel::show(ui, &mut self.state),
                ActiveTab::DataCoreBrowser => DataCoreBrowserPanel::show(ui, &mut self.state),
            }
        });

        // Extraction dialog
        ExtractionDialog::show(ctx, &mut self.state);
    }
}

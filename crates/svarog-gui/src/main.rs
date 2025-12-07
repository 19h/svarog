#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod panels;
mod preview;
mod state;
mod widgets;
mod worker;

use app::SvarogApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Svarog - Star Citizen File Browser",
        options,
        Box::new(|cc| Ok(Box::new(SvarogApp::new(cc)))),
    )
}

fn load_icon() -> egui::IconData {
    // Simple default icon - could be replaced with actual icon
    egui::IconData::default()
}

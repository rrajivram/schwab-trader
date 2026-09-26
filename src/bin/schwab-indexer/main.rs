//! schwab-indexer — egui app for building/rebalancing an S&P 500 basket.
//! Shares its config/tokens/blacklist with the `schwab` CLI.

use eframe::egui;

mod app;
mod execute;
mod home;
mod modes;
mod theme;
mod worker;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Schwab Indexer")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "schwab-indexer",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(app::IndexerApp::new(cc)))
        }),
    )
}

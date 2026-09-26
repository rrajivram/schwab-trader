//! schwab-indexer — egui app for building/rebalancing an S&P 500 basket.
//! Shares its config/tokens/blacklist with the `schwab` CLI.

use eframe::egui;

mod app;
mod worker;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Schwab Indexer")
            .with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "schwab-indexer",
        options,
        Box::new(|cc| Ok(Box::new(app::IndexerApp::new(cc)))),
    )
}

//! Plan tab: shows a local HTML page (the divestiture plan) in a native
//! web view placed over the tab's area. egui can't render HTML, so a wry
//! child web view is positioned to match the panel each frame and hidden
//! whenever another tab (or a dialog) is showing.

use std::path::PathBuf;

use eframe::egui::{self, Margin, RichText};
use schwab::config::Config;
use wry::dpi::{LogicalPosition, LogicalSize};

use crate::app::IndexerApp;
use crate::theme::{self, pal};

pub const DEFAULT_PLAN_PAGE: &str = "~/Documents/personal/website/amzn-nvda-exit-plan.html";

#[derive(Default)]
pub struct PlanView {
    webview: Option<wry::WebView>,
    /// Path the current content was read from.
    loaded: Option<PathBuf>,
    error: Option<String>,
    visible: bool,
}

/// The configured page path with `~` expanded.
pub fn plan_path() -> PathBuf {
    let raw = Config::load()
        .ok()
        .and_then(|c| c.plan_page)
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PLAN_PAGE.to_string());
    match raw.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(&raw)),
        None => PathBuf::from(raw),
    }
}

/// The page is saved as an HTML fragment (no doctype/charset), so give it a
/// document shell; without the charset its dashes and symbols garble.
fn as_document(body: &str) -> String {
    let head = body.trim_start().to_ascii_lowercase();
    if head.starts_with("<!doctype") || head.starts_with("<html") {
        return body.to_string();
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"></head><body>{body}</body></html>"
    )
}

impl PlanView {
    /// Hide the web view (another tab or a dialog is in front).
    pub fn hide(&mut self) {
        if self.visible {
            if let Some(w) = &self.webview {
                let _ = w.set_visible(false);
            }
            self.visible = false;
        }
    }

    fn load(&mut self, frame: &eframe::Frame, bounds: wry::Rect) {
        let path = plan_path();
        self.error = None;
        let html = match std::fs::read_to_string(&path) {
            Ok(s) => as_document(&s),
            Err(e) => {
                self.error = Some(format!("Couldn't read {}: {e}", path.display()));
                self.loaded = None;
                return;
            }
        };
        let result = match &self.webview {
            Some(w) => w.load_html(&html),
            None => wry::WebViewBuilder::new()
                .with_html(html)
                .with_bounds(bounds)
                .build_as_child(frame)
                .map(|w| self.webview = Some(w)),
        };
        match result {
            Ok(()) => self.loaded = Some(path),
            Err(e) => self.error = Some(format!("Couldn't show the page: {e}")),
        }
    }
}

impl IndexerApp {
    pub(crate) fn plan_ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let p = pal(ui);
        let path = plan_path();
        let mut reload = self.plan.loaded.as_ref() != Some(&path) && self.plan.error.is_none();

        let bar = egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(18, 8));
        egui::Panel::top("plan_bar").frame(bar).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Divestiture plan").font(theme::sans_semibold(theme::TITLE)));
                ui.add(egui::Label::new(RichText::new(path.display().to_string()).color(p.muted).font(theme::mono(12.0))).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Open in browser").on_hover_text("Open the same file in your default browser").clicked() {
                        let _ = webbrowser::open(&format!("file://{}", path.display()));
                    }
                    if ui.button("Reload").on_hover_text("Re-read the file from disk").clicked() {
                        reload = true;
                    }
                });
            });
        });

        let frame_fill = egui::Frame::new().fill(p.bg).inner_margin(Margin::same(0));
        egui::CentralPanel::default().frame(frame_fill).show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            // egui points → window logical pixels.
            let z = ui.ctx().zoom_factor() as f64;
            let bounds = wry::Rect {
                position: LogicalPosition::new(rect.left() as f64 * z, rect.top() as f64 * z).into(),
                size: LogicalSize::new(rect.width().max(1.0) as f64 * z, rect.height().max(1.0) as f64 * z).into(),
            };
            if reload {
                self.plan.load(frame, bounds);
            }
            if let Some(err) = &self.plan.error {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(err).color(p.loss));
                    ui.label(RichText::new("Change the path in Settings, or click Reload once the file is there.").color(p.muted));
                });
                self.plan.hide();
                return;
            }
            // A dialog would be drawn underneath the native view, so hide it then.
            if self.settings.is_some() {
                self.plan.hide();
                return;
            }
            if let Some(w) = &self.plan.webview {
                let _ = w.set_bounds(bounds);
                if !self.plan.visible {
                    let _ = w.set_visible(true);
                    self.plan.visible = true;
                }
            }
        });
    }
}

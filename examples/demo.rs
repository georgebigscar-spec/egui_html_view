//! Run with `cargo run --example demo [path/to/file.html]`.

use eframe::egui;
use egui_html_view::{HtmlView, ScrollBarVisibility};

struct DemoApp {
    view: HtmlView,
    source: String,
    show_source: bool,
    scroll_bars: bool,
    last_link: Option<String>,
}

impl DemoApp {
    fn new(cc: &eframe::CreationContext<'_>, source: String) -> Self {
        // Let the view handle Ctrl +/-/0 instead of zooming the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        Self {
            view: HtmlView::new(&source),
            source,
            show_source: false,
            scroll_bars: true,
            last_link: None,
        }
    }
}

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("−").on_hover_text("Zoom out (Ctrl -)").clicked() {
                    self.view.zoom_out();
                }
                if ui
                    .button(format!("{:.0}%", self.view.zoom() * 100.0))
                    .on_hover_text("Reset zoom (Ctrl 0)")
                    .clicked()
                {
                    self.view.reset_zoom();
                }
                if ui.button("+").on_hover_text("Zoom in (Ctrl +)").clicked() {
                    self.view.zoom_in();
                }
                ui.separator();
                if ui.checkbox(&mut self.scroll_bars, "Scroll bars").changed() {
                    self.view.set_scroll_bar_visibility(if self.scroll_bars {
                        ScrollBarVisibility::VisibleWhenNeeded
                    } else {
                        ScrollBarVisibility::AlwaysHidden
                    });
                }
                ui.checkbox(&mut self.show_source, "Edit HTML");
                ui.separator();
                egui::widgets::global_theme_preference_buttons(ui);
                ui.separator();
                if let Some(link) = &self.last_link {
                    ui.label(format!("Clicked: {link}"));
                }
            });
        });

        if self.show_source {
            egui::Panel::right("source")
                .resizable(true)
                .default_size(400.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let edit = ui.add(
                            egui::TextEdit::multiline(&mut self.source)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                        if edit.changed() {
                            self.view.set_html(&self.source);
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let response = self.view.show(ui);
            if let Some(link) = response.clicked_link {
                self.last_link = Some(link);
            }
            if let Some(link) = response.hovered_link {
                response.response.on_hover_text_at_pointer(link);
            }
        });
    }
}

fn main() -> eframe::Result {
    let source = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|e| format!("<p>Cannot read {path}: {e}</p>")),
        None => include_str!("demo.html").to_owned(),
    };
    eframe::run_native(
        "egui_html_view demo",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(DemoApp::new(cc, source)))),
    )
}

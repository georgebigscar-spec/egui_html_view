//! An HTML + CSS view widget for [egui](https://github.com/emilk/egui).
//!
//! [`HtmlView`] parses HTML with `html5ever` (via `scraper`), applies CSS (a default style
//! sheet, `<style>` elements, `style` attributes and your own style sheets) and renders the
//! result natively with egui — no browser engine or native webview involved. It supports:
//!
//! * block, inline, list and table layout with margins, padding, borders and backgrounds;
//! * text styling: colors, sizes, bold/italic, monospace, decorations, alignment,
//!   `line-height`, `white-space`, `text-transform`, `letter-spacing`, CSS variables,
//!   `@media` queries (width and `prefers-color-scheme`);
//! * optional scroll bars ([`ScrollMode`], [`egui::scroll_area::ScrollBarVisibility`]);
//! * zooming (Ctrl + wheel, pinch, Ctrl +/-/0, or [`HtmlView::set_zoom`]);
//! * text selection across blocks (drag, Shift+click, double/triple click, Ctrl+A),
//!   copying (Ctrl+C or the context menu);
//! * links (reported in [`HtmlViewResponse::clicked_link`]; `#fragment` links scroll),
//!   and images through egui's image loaders.
//!
//! JavaScript, forms, floats, flexbox/grid and positioning are not supported:
//! flex and grid containers are laid out as normal blocks.
//!
//! ```no_run
//! struct App {
//!     view: egui_html_view::HtmlView,
//! }
//!
//! impl App {
//!     fn new() -> Self {
//!         let view = egui_html_view::HtmlView::new("<h1>Hello</h1><p>Some <b>HTML</b>.</p>")
//!             .with_css("p { color: gray }");
//!         Self { view }
//!     }
//!
//!     fn ui(&mut self, ui: &mut egui::Ui) {
//!         let response = self.view.show(ui);
//!         if let Some(href) = response.clicked_link {
//!             println!("link clicked: {href}");
//!         }
//!     }
//! }
//! ```

mod boxes;
mod css;
mod layout;
mod style;
mod view;

pub use layout::{BoldMode, FontOptions};
pub use view::{HtmlView, HtmlViewResponse, ScrollMode};

pub use egui::scroll_area::ScrollBarVisibility;

#[cfg(test)]
mod tests;

//! The [`HtmlView`] widget: caching, painting, scrolling, zooming, selection and links.

use std::{
    hash::Hash,
    ops::{Range, RangeInclusive},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use egui::{
    pos2, scroll_area::ScrollBarVisibility, text::CharIndex, vec2, Color32, CursorIcon, Event,
    EventFilter, FontFamily, FontId, Galley, Id, Key, Modifiers, OpenUrl, Painter, PointerButton,
    Pos2, Rect, Response, ScrollArea, Sense, Ui, Vec2,
};
use scraper::{Html, Selector};

use crate::{
    boxes::{BlockBox, BoxBuilder},
    css::{self, MediaEnv, Rule},
    layout::{layout_document, BoldMode, DocLayout, FontOptions, Item, LayoutParams, TextBlock},
    style::{CascadeContext, ComputedStyle, StyleEngine, Theme},
};

/// Which axes of an [`HtmlView`] scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollMode {
    /// No scroll area: the view takes the full height of its content
    /// (useful inside an outer [`ScrollArea`]).
    None,
    /// Vertical scrolling only; content wider than the view is clipped.
    Vertical,
    /// Vertical scrolling, plus horizontal scrolling when content is wider than the view.
    #[default]
    Both,
}

/// What [`HtmlView::show`] returns.
pub struct HtmlViewResponse {
    /// Response of the document area.
    pub response: Response,
    /// The `href` of a link that was clicked this frame.
    ///
    /// Fragment links (`#id`) also scroll the view to the target automatically.
    pub clicked_link: Option<String>,
    /// The `href` of the link under the mouse pointer.
    pub hovered_link: Option<String>,
    /// Size of the laid out document, in points.
    pub content_size: Vec2,
}

/// A position in the document text: text block index + character index within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TextPos {
    block: usize,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    anchor: TextPos,
    focus: TextPos,
}

impl Selection {
    fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

enum PendingScroll {
    Offset(Vec2),
    Anchor(String),
}

#[derive(PartialEq)]
struct BoxKey {
    theme: Theme,
    media: Vec<bool>,
    base_font_size: f32,
    generation: u64,
}

struct BoxCache {
    key: BoxKey,
    root: BlockBox,
    canvas_bg: Color32,
    generation: u64,
}

#[derive(PartialEq)]
struct LayoutKey {
    width: f32,
    viewport_height: f32,
    zoom: f32,
    pixels_per_point: f32,
    box_generation: u64,
    fonts: FontOptions,
    base_uri: Option<String>,
}

pub(crate) struct LayoutCache {
    key: LayoutKey,
    pub(crate) layout: DocLayout,
    /// A galley requested every frame: when egui throws away its font atlas (and with it the
    /// glyph positions of all galleys), we get a new one back and must lay out again.
    probe: Arc<Galley>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// An egui widget that renders HTML with CSS.
///
/// Keep it in your app state and call [`HtmlView::show`] every frame.
///
/// ```no_run
/// # fn ui(ui: &mut egui::Ui, view: &mut egui_html_view::HtmlView) {
/// let response = view.show(ui);
/// if let Some(href) = response.clicked_link {
///     println!("clicked {href}");
/// }
/// # }
/// ```
pub struct HtmlView {
    id: Id,
    document: Html,
    title: Option<String>,
    document_css: Vec<Rule>,
    user_css: Vec<Vec<Rule>>,
    engine: Option<StyleEngine>,
    /// Bumped whenever the document or style sheets change.
    generation: u64,
    box_generation: u64,
    boxes: Option<BoxCache>,
    pub(crate) layout: Option<LayoutCache>,

    scroll_mode: ScrollMode,
    scroll_bar_visibility: ScrollBarVisibility,
    auto_shrink: [bool; 2],
    selectable: bool,
    zoomable: bool,
    zoom: f32,
    zoom_range: RangeInclusive<f32>,
    base_font_size: f32,
    fonts: FontOptions,
    base_uri: Option<String>,
    open_external_links: bool,

    selection: Option<Selection>,
    pending_scroll: Option<PendingScroll>,
    /// Screen rect of the view in the last frame.
    last_rect: Option<Rect>,
    last_offset: Vec2,
}

impl HtmlView {
    /// Parses `html` (a full document or a fragment).
    pub fn new(html: &str) -> Self {
        let mut view = Self {
            id: Id::new(("egui_html_view", NEXT_ID.fetch_add(1, Ordering::Relaxed))),
            document: Html::new_document(),
            title: None,
            document_css: Vec::new(),
            user_css: Vec::new(),
            engine: None,
            generation: 0,
            box_generation: 0,
            boxes: None,
            layout: None,
            scroll_mode: ScrollMode::default(),
            scroll_bar_visibility: ScrollBarVisibility::VisibleWhenNeeded,
            auto_shrink: [false, false],
            selectable: true,
            zoomable: true,
            zoom: 1.0,
            zoom_range: 0.25..=5.0,
            base_font_size: 16.0,
            fonts: FontOptions::default(),
            base_uri: None,
            open_external_links: false,
            selection: None,
            pending_scroll: None,
            last_rect: None,
            last_offset: Vec2::ZERO,
        };
        view.set_html(html);
        view
    }

    // ------------------------------------------------------------------------
    // Builder-style options

    /// Use a stable id (e.g. when the view is re-created between frames).
    #[must_use]
    pub fn id_salt(mut self, salt: impl Hash + std::fmt::Debug) -> Self {
        self.id = Id::new(salt);
        self
    }

    /// Adds a style sheet applied after the document's own `<style>` elements.
    #[must_use]
    pub fn with_css(mut self, css: &str) -> Self {
        self.add_css(css);
        self
    }

    #[must_use]
    pub fn scroll_mode(mut self, mode: ScrollMode) -> Self {
        self.scroll_mode = mode;
        self
    }

    /// Show the scroll bars always, only when needed, or never (scrolling with the
    /// mouse wheel / keyboard still works when hidden).
    #[must_use]
    pub fn scroll_bar_visibility(mut self, visibility: ScrollBarVisibility) -> Self {
        self.scroll_bar_visibility = visibility;
        self
    }

    /// Whether the scroll area shrinks to the content size (default: fill the available space).
    #[must_use]
    pub fn auto_shrink(mut self, auto_shrink: [bool; 2]) -> Self {
        self.auto_shrink = auto_shrink;
        self
    }

    /// Allow selecting and copying text (default `true`).
    #[must_use]
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Allow zooming with Ctrl + mouse wheel, pinch gestures and Ctrl +/-/0 (default `true`).
    ///
    /// Note: egui handles Ctrl +/-/0 itself (zooming the whole UI) unless
    /// `ctx.options_mut(|o| o.zoom_with_keyboard = false)` is set.
    #[must_use]
    pub fn zoomable(mut self, zoomable: bool) -> Self {
        self.zoomable = zoomable;
        self
    }

    #[must_use]
    pub fn zoom_range(mut self, range: RangeInclusive<f32>) -> Self {
        self.zoom_range = range;
        self.set_zoom(self.zoom);
        self
    }

    /// Initial zoom factor.
    #[must_use]
    pub fn with_zoom(mut self, zoom: f32) -> Self {
        self.set_zoom(zoom);
        self
    }

    /// The default font size (`medium`) in CSS pixels. Default: 16.
    #[must_use]
    pub fn base_font_size(mut self, size: f32) -> Self {
        self.base_font_size = size.max(1.0);
        self
    }

    #[must_use]
    pub fn bold_mode(mut self, mode: BoldMode) -> Self {
        self.fonts.bold = mode;
        self
    }

    /// Maps a CSS `font-family` name to an egui font family.
    #[must_use]
    pub fn font_family(mut self, css_name: &str, family: FontFamily) -> Self {
        self.fonts
            .families
            .push((css_name.to_ascii_lowercase(), family));
        self
    }

    /// Base URI for resolving relative image sources, e.g. `"file:///home/me/page/"`.
    #[must_use]
    pub fn base_uri(mut self, uri: impl Into<String>) -> Self {
        self.base_uri = Some(uri.into());
        self
    }

    /// Open clicked `http(s):`/`mailto:` links with [`egui::Context::open_url`] (default `false`).
    #[must_use]
    pub fn open_external_links(mut self, open: bool) -> Self {
        self.open_external_links = open;
        self
    }

    // ------------------------------------------------------------------------
    // Content

    /// Replaces the document. Resets selection and scroll position.
    pub fn set_html(&mut self, html: &str) {
        self.document = Html::parse_document(html);
        let title = Selector::parse("title").expect("valid selector");
        self.title = self
            .document
            .select(&title)
            .next()
            .map(|t| {
                t.text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|t| !t.is_empty());
        let style = Selector::parse("style").expect("valid selector");
        let css: String = self
            .document
            .select(&style)
            .filter(|s| {
                s.value()
                    .attr("media")
                    .is_none_or(|m| m.is_empty() || !m.contains("print") || m.contains("screen"))
            })
            .flat_map(|s| s.text())
            .collect::<Vec<_>>()
            .join("\n");
        self.document_css = css::parse_stylesheet(&css);
        self.selection = None;
        self.pending_scroll = Some(PendingScroll::Offset(Vec2::ZERO));
        self.invalidate_styles();
    }

    /// Adds a style sheet applied after the document's own style sheets.
    pub fn add_css(&mut self, css: &str) {
        self.user_css.push(css::parse_stylesheet(css));
        self.invalidate_styles();
    }

    /// Removes all style sheets added with [`Self::add_css`].
    pub fn clear_css(&mut self) {
        self.user_css.clear();
        self.invalidate_styles();
    }

    fn invalidate_styles(&mut self) {
        self.engine = None;
        self.generation += 1;
    }

    /// The document's `<title>`.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_scroll_mode(&mut self, mode: ScrollMode) {
        self.scroll_mode = mode;
    }

    pub fn set_scroll_bar_visibility(&mut self, visibility: ScrollBarVisibility) {
        self.scroll_bar_visibility = visibility;
    }

    pub fn set_selectable(&mut self, selectable: bool) {
        self.selectable = selectable;
        if !selectable {
            self.selection = None;
        }
    }

    // ------------------------------------------------------------------------
    // Zoom

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(*self.zoom_range.start(), *self.zoom_range.end());
    }

    pub fn zoom_in(&mut self) {
        self.zoom_around(1.1, None);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_around(1.0 / 1.1, None);
    }

    pub fn reset_zoom(&mut self) {
        self.zoom_around(1.0 / self.zoom, None);
    }

    /// Multiplies the zoom by `factor`, keeping the content under `anchor`
    /// (relative to the top-left of the view) in place.
    fn zoom_around(&mut self, factor: f32, anchor: Option<Vec2>) {
        let old = self.zoom;
        self.set_zoom(old * factor);
        let ratio = self.zoom / old;
        if (ratio - 1.0).abs() < 1e-6 {
            return;
        }
        let anchor = anchor.unwrap_or(Vec2::ZERO);
        let offset = (self.last_offset + anchor) * ratio - anchor;
        self.pending_scroll = Some(PendingScroll::Offset(offset.max(Vec2::ZERO)));
    }

    // ------------------------------------------------------------------------
    // Scrolling

    /// Scrolls to the element with the given `id` (or `<a name>`), with or without a leading `#`.
    pub fn scroll_to_anchor(&mut self, id: &str) {
        self.pending_scroll = Some(PendingScroll::Anchor(id.trim_start_matches('#').to_owned()));
    }

    pub fn scroll_to_offset(&mut self, offset: Vec2) {
        self.pending_scroll = Some(PendingScroll::Offset(offset));
    }

    /// The current scroll offset (as of the last frame).
    pub fn scroll_offset(&self) -> Vec2 {
        self.last_offset
    }

    // ------------------------------------------------------------------------
    // Selection

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection.filter(|s| !s.is_empty())?;
        let layout = &self.layout.as_ref()?.layout;
        Some(selected_text(layout, &selection)).filter(|t| !t.is_empty())
    }

    /// Selects all text (once the document has been laid out).
    pub fn select_all(&mut self) {
        if let Some(cache) = &self.layout {
            self.selection = select_all(&cache.layout);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    // ------------------------------------------------------------------------
    // Showing

    pub fn show(&mut self, ui: &mut Ui) -> HtmlViewResponse {
        if self.zoomable {
            if let Some(rect) = self.last_rect {
                if ui.rect_contains_pointer(rect) {
                    let (zoom_delta, pointer) =
                        ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
                    if zoom_delta != 1.0 {
                        let anchor = pointer.map(|p| p - rect.min);
                        self.zoom_around(zoom_delta, anchor);
                    }
                }
            }
        }

        if self.scroll_mode == ScrollMode::None {
            let width = ui.available_width();
            let out = self.show_content(ui, width, None);
            self.last_rect = Some(out.response.rect);
            self.last_offset = Vec2::ZERO;
            // Without our own scroll area, anchors scroll the enclosing one.
            if let Some(PendingScroll::Anchor(id)) = self.pending_scroll.take() {
                let y = self
                    .layout
                    .as_ref()
                    .and_then(|c| c.layout.anchors.get(&id).copied());
                if let Some(y) = y {
                    let top = out.response.rect.min + vec2(0.0, y);
                    let rect = Rect::from_min_size(top, vec2(out.response.rect.width(), 1.0));
                    ui.scroll_to_rect(rect, Some(egui::Align::TOP));
                }
            }
            return out;
        }

        let mut area = ScrollArea::new([self.scroll_mode == ScrollMode::Both, true])
            .id_salt(self.id.with("scroll"))
            .auto_shrink(self.auto_shrink)
            .scroll_bar_visibility(self.scroll_bar_visibility)
            .content_margin(0.0);
        if let Some(offset) = self.resolve_pending_scroll() {
            area = area.scroll_offset(offset);
        }
        let output = area.show_viewport(ui, |ui, viewport| {
            let width = ui.available_width();
            self.show_content(ui, width, Some(viewport))
        });
        self.last_rect = Some(output.inner_rect);
        self.last_offset = output.state.offset;
        output.inner
    }

    fn resolve_pending_scroll(&mut self) -> Option<Vec2> {
        match self.pending_scroll.take()? {
            PendingScroll::Offset(offset) => Some(offset),
            PendingScroll::Anchor(id) => {
                let y = self
                    .layout
                    .as_ref()
                    .and_then(|c| c.layout.anchors.get(&id).copied());
                match y {
                    Some(y) => Some(vec2(self.last_offset.x, y)),
                    None => {
                        // Not laid out yet: try again next frame.
                        if self.layout.is_none() {
                            self.pending_scroll = Some(PendingScroll::Anchor(id));
                        }
                        None
                    }
                }
            }
        }
    }

    /// Updates the style and layout caches for the given content width.
    fn ensure_layout(&mut self, ui: &Ui, width: f32, viewport_height: f32) {
        let ctx = ui.ctx();
        let theme = Theme::from_visuals(ui.visuals());
        let width = (width.max(16.0) * 2.0).round() / 2.0;

        let engine = self.engine.get_or_insert_with(|| {
            let mut sheets = vec![self.document_css.clone()];
            sheets.extend(self.user_css.iter().cloned());
            StyleEngine::new(&sheets)
        });

        let media = engine.media_state(&MediaEnv {
            width: width / self.zoom,
            height: viewport_height / self.zoom,
            dark: theme.dark,
        });
        let box_key = BoxKey {
            theme,
            media,
            base_font_size: self.base_font_size,
            generation: self.generation,
        };
        if self.boxes.as_ref().is_none_or(|b| b.key != box_key) {
            let initial = ComputedStyle::initial(&theme, self.base_font_size);
            let mut builder = BoxBuilder {
                engine,
                ctx: CascadeContext {
                    theme: &theme,
                    initial: &initial,
                    media: &box_key.media,
                    root_font_size: self.base_font_size,
                    base_font_size: self.base_font_size,
                },
            };
            let (root, canvas_bg) = builder.build_document(&self.document);
            self.box_generation += 1;
            self.boxes = Some(BoxCache {
                key: box_key,
                root,
                canvas_bg,
                generation: self.box_generation,
            });
        }
        let boxes = self.boxes.as_ref().expect("built above");

        let probe = ctx.fonts_mut(|f| {
            f.layout_job(egui::text::LayoutJob::simple_singleline(
                "egui_html_view probe".to_owned(),
                FontId::proportional(9.0),
                Color32::WHITE,
            ))
        });
        let layout_key = LayoutKey {
            width,
            viewport_height: viewport_height.round(),
            zoom: self.zoom,
            pixels_per_point: ctx.pixels_per_point(),
            box_generation: boxes.generation,
            fonts: self.fonts.clone(),
            base_uri: self.base_uri.clone(),
        };
        let valid = self.layout.as_ref().is_some_and(|c| {
            c.key == layout_key && Arc::ptr_eq(&c.probe, &probe) && !c.layout.pending_images
        });
        if !valid {
            let params = LayoutParams {
                ctx,
                zoom: self.zoom,
                fonts: &self.fonts,
                viewport: vec2(width, viewport_height),
                base_uri: self.base_uri.as_deref(),
            };
            let layout = layout_document(&boxes.root, &params, width);
            if let Some(sel) = &mut self.selection {
                clamp_selection(sel, &layout);
            }
            self.layout = Some(LayoutCache {
                key: layout_key,
                layout,
                probe,
            });
        }
    }

    fn show_content(
        &mut self,
        ui: &mut Ui,
        width: f32,
        viewport: Option<Rect>,
    ) -> HtmlViewResponse {
        let viewport_height =
            viewport.map_or_else(|| ui.ctx().content_rect().height(), |v| v.height());
        self.ensure_layout(ui, width, viewport_height);
        let canvas_bg = self
            .boxes
            .as_ref()
            .map_or(Color32::TRANSPARENT, |b| b.canvas_bg);
        let layout = &self.layout.as_ref().expect("laid out above").layout;

        let origin = ui.max_rect().min;
        let offset = origin.to_vec2();
        let doc_rect = Rect::from_min_size(origin, layout.size);
        ui.allocate_rect(doc_rect, Sense::hover());

        let visible_screen = match viewport {
            Some(v) => v.translate(offset),
            None => ui.clip_rect().intersect(doc_rect),
        };
        let visible_doc = visible_screen.translate(-offset);
        let interact_rect = doc_rect.union(visible_screen);

        let sense = if self.selectable {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let response = ui.interact(interact_rect, self.id.with("content"), sense);

        // ---- Painting ----
        let painter = ui.painter_at(visible_screen.intersect(ui.clip_rect()));
        if canvas_bg.a() > 0 {
            painter.rect_filled(visible_screen, 0.0, canvas_bg);
        }
        let selection = self
            .selection
            .filter(|s| !s.is_empty())
            .map(|s| s.ordered());
        let selection_color = ui.visuals().selection.bg_fill;
        let fallback_color = ui.visuals().text_color();
        let cull = visible_doc.expand(4.0);
        for item in &layout.items {
            match item {
                Item::Shape(shape) => {
                    if shape.visual_bounding_rect().intersects(cull) {
                        let mut shape = shape.clone();
                        shape.translate(offset);
                        painter.add(shape);
                    }
                }
                Item::Text(index) => {
                    let block = &layout.texts[*index];
                    if !block.rect().intersects(cull) {
                        continue;
                    }
                    if let Some((start, end)) = selection {
                        if (start.block..=end.block).contains(index) {
                            let from = if *index == start.block {
                                start.index
                            } else {
                                0
                            };
                            let to = if *index == end.block {
                                end.index
                            } else {
                                block.char_count
                            };
                            let continues = *index != end.block;
                            paint_selection(
                                &painter,
                                block,
                                from..to,
                                continues,
                                offset,
                                selection_color,
                            );
                        }
                    }
                    let pos = block.origin + offset + block.glyph_offset;
                    painter.galley(pos, block.galley.clone(), fallback_color);
                    if let Some((overlay, dx)) = &block.bold_overlay {
                        painter.galley(pos + vec2(*dx, 0.0), overlay.clone(), fallback_color);
                    }
                }
                Item::Image { rect, uri, .. } => {
                    if rect.intersects(cull) {
                        egui::Image::new(uri.as_str())
                            .show_loading_spinner(true)
                            .paint_at(ui, rect.translate(offset));
                    }
                }
                Item::Placeholder => {}
            }
        }

        // ---- Interaction ----
        let to_doc = |p: Pos2| p - offset;
        let hover_doc = response.hover_pos().map(to_doc);
        let hovered_link = hover_doc.and_then(|p| link_at(layout, p));
        if hovered_link.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        } else if self.selectable
            && (response.dragged() || hover_doc.is_some_and(|p| text_at(layout, p)))
        {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }

        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        tab: false,
                        escape: false,
                    },
                );
            });
        }

        let mut clicked_link = None;
        let shift = ui.input(|i| i.modifiers.shift);
        if self.selectable {
            if response.drag_started_by(PointerButton::Primary) {
                let press = ui.input(|i| i.pointer.press_origin()).map(to_doc);
                if let Some(pos) = press.and_then(|p| hit_test(layout, p)) {
                    match &mut self.selection {
                        Some(sel) if shift => sel.focus = pos,
                        _ => {
                            self.selection = Some(Selection {
                                anchor: pos,
                                focus: pos,
                            });
                        }
                    }
                }
            }
            if response.dragged_by(PointerButton::Primary) {
                if let Some(pointer) = response.interact_pointer_pos() {
                    if let (Some(sel), Some(pos)) =
                        (&mut self.selection, hit_test(layout, to_doc(pointer)))
                    {
                        sel.focus = pos;
                    }
                    // Auto-scroll when dragging beyond the visible area.
                    if viewport.is_some() {
                        let clip = visible_screen;
                        let mut delta = Vec2::ZERO;
                        for d in 0..2 {
                            if pointer[d] < clip.min[d] {
                                delta[d] = (clip.min[d] - pointer[d]).min(60.0);
                            } else if pointer[d] > clip.max[d] {
                                delta[d] = -(pointer[d] - clip.max[d]).min(60.0);
                            }
                        }
                        if delta != Vec2::ZERO {
                            ui.scroll_with_delta(delta * 0.5);
                            ui.ctx().request_repaint();
                        }
                    }
                }
            }
        }

        if response.clicked_by(PointerButton::Primary) {
            let click = response.interact_pointer_pos().map(to_doc);
            if let Some(link) = click.and_then(|p| link_at(layout, p)) {
                clicked_link = Some(link.to_string());
            } else if self.selectable {
                if let Some(p) = click {
                    let triple = response.triple_clicked();
                    let double = response.double_clicked();
                    if triple || double {
                        if let Some(pos) = hit_test(layout, p) {
                            let block = &layout.texts[pos.block];
                            let char_index = char_at(block, p - block.origin).unwrap_or(pos.index);
                            let range = if triple {
                                paragraph_range(block.galley.text(), char_index)
                            } else {
                                word_range(block.galley.text(), char_index)
                            };
                            self.selection = Some(Selection {
                                anchor: TextPos {
                                    block: pos.block,
                                    index: range.start,
                                },
                                focus: TextPos {
                                    block: pos.block,
                                    index: range.end,
                                },
                            });
                        }
                    } else if shift && self.selection.is_some() {
                        if let (Some(sel), Some(pos)) = (&mut self.selection, hit_test(layout, p)) {
                            sel.focus = pos;
                        }
                    } else {
                        self.selection = None;
                    }
                }
            }
        }

        // ---- Keyboard ----
        let mut copy = false;
        let mut zoom_request = None;
        if response.has_focus() {
            let zoomable = self.zoomable;
            let (copy_event, select_all_key, escape, zoom_key, scroll_key) = ui.input_mut(|i| {
                let copy_event = i
                    .events
                    .iter()
                    .any(|e| matches!(e, Event::Copy | Event::Cut));
                let select_all_key = i.consume_key(Modifiers::COMMAND, Key::A);
                let escape = i.key_pressed(Key::Escape);
                let mut zoom_key = None;
                if zoomable {
                    if i.consume_key(Modifiers::COMMAND, Key::Plus)
                        || i.consume_key(Modifiers::COMMAND, Key::Equals)
                    {
                        zoom_key = Some(1.1);
                    } else if i.consume_key(Modifiers::COMMAND, Key::Minus) {
                        zoom_key = Some(1.0 / 1.1);
                    } else if i.consume_key(Modifiers::COMMAND, Key::Num0) {
                        zoom_key = Some(0.0);
                    }
                }
                let line = 40.0;
                let page = (visible_screen.height() - line).max(line);
                let scroll_key = [
                    (Key::ArrowDown, vec2(0.0, -line)),
                    (Key::ArrowUp, vec2(0.0, line)),
                    (Key::ArrowRight, vec2(-line, 0.0)),
                    (Key::ArrowLeft, vec2(line, 0.0)),
                    (Key::PageDown, vec2(0.0, -page)),
                    (Key::PageUp, vec2(0.0, page)),
                    (Key::Space, vec2(0.0, -page)),
                    (Key::Home, vec2(0.0, f32::INFINITY)),
                    (Key::End, vec2(0.0, f32::NEG_INFINITY)),
                ]
                .into_iter()
                .find(|(key, _)| i.consume_key(Modifiers::NONE, *key))
                .map(|(_, delta)| delta);
                (copy_event, select_all_key, escape, zoom_key, scroll_key)
            });
            copy = copy_event;
            if select_all_key && self.selectable {
                self.selection = select_all(layout);
            }
            if escape {
                self.selection = None;
            }
            zoom_request = zoom_key;
            if let Some(delta) = scroll_key {
                if viewport.is_some() {
                    if delta.y.is_infinite() {
                        let y = if delta.y > 0.0 { 0.0 } else { layout.size.y };
                        self.pending_scroll =
                            Some(PendingScroll::Offset(vec2(self.last_offset.x, y)));
                    } else {
                        ui.scroll_with_delta(delta);
                    }
                }
            }
        }

        // ---- Context menu ----
        let has_selection = self.selection.is_some_and(|s| !s.is_empty());
        let mut select_all_clicked = false;
        response.context_menu(|ui| {
            if ui
                .add_enabled(has_selection, egui::Button::new("Copy"))
                .clicked()
            {
                copy = true;
                ui.close();
            }
            if ui
                .add_enabled(self.selectable, egui::Button::new("Select all"))
                .clicked()
            {
                select_all_clicked = true;
                ui.close();
            }
            if let Some(link) = &hovered_link {
                if ui.button("Copy link address").clicked() {
                    ui.ctx().copy_text(link.to_string());
                    ui.close();
                }
            }
        });
        if select_all_clicked {
            self.selection = select_all(layout);
        }
        if copy {
            if let Some(sel) = self.selection.filter(|s| !s.is_empty()) {
                let text = selected_text(layout, &sel);
                if !text.is_empty() {
                    ui.ctx().copy_text(text);
                }
            }
        }

        // ---- Links ----
        if let Some(href) = &clicked_link {
            if let Some(fragment) = href.strip_prefix('#') {
                if layout.anchors.contains_key(fragment) {
                    self.pending_scroll = Some(PendingScroll::Anchor(fragment.to_owned()));
                }
            } else if self.open_external_links {
                let lower = href.to_ascii_lowercase();
                if lower.starts_with("http://")
                    || lower.starts_with("https://")
                    || lower.starts_with("mailto:")
                {
                    ui.ctx().open_url(OpenUrl::new_tab(href));
                }
            }
        }
        let content_size = layout.size;
        match zoom_request {
            Some(0.0) => self.reset_zoom(),
            Some(factor) => self.zoom_around(factor, None),
            None => {}
        }
        if self.pending_scroll.is_some() {
            ui.ctx().request_repaint();
        }

        HtmlViewResponse {
            response,
            clicked_link,
            hovered_link: hovered_link.map(|l| l.to_string()),
            content_size,
        }
    }
}

// ----------------------------------------------------------------------------
// Selection helpers

fn clamp_selection(sel: &mut Selection, layout: &DocLayout) {
    let clamp = |p: &mut TextPos| match layout.texts.len() {
        0 => *p = TextPos { block: 0, index: 0 },
        n => {
            p.block = p.block.min(n - 1);
            p.index = p.index.min(layout.texts[p.block].char_count);
        }
    };
    clamp(&mut sel.anchor);
    clamp(&mut sel.focus);
}

fn select_all(layout: &DocLayout) -> Option<Selection> {
    let last = layout.texts.len().checked_sub(1)?;
    Some(Selection {
        anchor: TextPos { block: 0, index: 0 },
        focus: TextPos {
            block: last,
            index: layout.texts[last].char_count,
        },
    })
}

fn selected_text(layout: &DocLayout, sel: &Selection) -> String {
    let (start, end) = sel.ordered();
    let mut out = String::new();
    for (i, block) in layout
        .texts
        .iter()
        .enumerate()
        .take(end.block + 1)
        .skip(start.block)
    {
        let from = if i == start.block { start.index } else { 0 };
        let to = if i == end.block {
            end.index
        } else {
            block.char_count
        };
        if i != start.block {
            out.push_str(block.separator);
        }
        out.extend(
            block
                .galley
                .text()
                .chars()
                .skip(from)
                .take(to.saturating_sub(from))
                .filter(|&c| c != '\u{200B}')
                .map(|c| if c == '\u{a0}' { ' ' } else { c }),
        );
    }
    out
}

/// Finds the text position closest to `p` (document coordinates).
fn hit_test(layout: &DocLayout, p: Pos2) -> Option<TextPos> {
    let mut best: Option<(f32, f32, usize)> = None;
    for (i, block) in layout.texts.iter().enumerate() {
        let r = block.rect();
        let dist = |v: f32, min: f32, max: f32| {
            if v < min {
                min - v
            } else if v > max {
                v - max
            } else {
                0.0
            }
        };
        let dy = dist(p.y, r.top(), r.bottom());
        let dx = dist(p.x, r.left(), r.right());
        if best.is_none_or(|(by, bx, _)| dy < by || (dy == by && dx < bx)) {
            best = Some((dy, dx, i));
        }
    }
    let (_, _, i) = best?;
    let block = &layout.texts[i];
    let r = block.rect();
    let index = if p.y < r.top() {
        0
    } else if p.y > r.bottom() {
        block.char_count
    } else {
        block.galley.cursor_from_pos(p - block.origin).index.0
    };
    Some(TextPos {
        block: i,
        index: index.min(block.char_count),
    })
}

/// Index of the character whose glyph contains `local` (galley coordinates).
fn char_at(block: &TextBlock, local: Vec2) -> Option<usize> {
    let mut row_start = 0;
    for row in &block.galley.rows {
        if (row.min_y()..=row.max_y()).contains(&local.y) {
            let x = local.x - row.pos.x;
            return row
                .glyphs
                .iter()
                .position(|g| x >= g.pos.x && x < g.pos.x + g.advance_width.max(1.0))
                .map(|i| row_start + i);
        }
        row_start += row.char_count_including_newline().0;
    }
    None
}

fn text_at(layout: &DocLayout, p: Pos2) -> bool {
    layout
        .texts
        .iter()
        .any(|b| b.rect().contains(p) && char_at(b, p - b.origin).is_some())
}

fn link_at(layout: &DocLayout, p: Pos2) -> Option<Arc<str>> {
    for item in &layout.items {
        if let Item::Image {
            rect,
            link: Some(link),
            ..
        } = item
        {
            if rect.contains(p) {
                return Some(link.clone());
            }
        }
    }
    for block in &layout.texts {
        if block.links.is_empty() || !block.rect().expand(1.0).contains(p) {
            continue;
        }
        if let Some(index) = char_at(block, p - block.origin) {
            if let Some((_, link)) = block.links.iter().find(|(r, _)| r.contains(&index)) {
                return Some(link.clone());
            }
        }
    }
    None
}

fn word_range(text: &str, index: usize) -> Range<usize> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return 0..0;
    }
    let i = index.min(chars.len() - 1);
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let k = class(chars[i]);
    if k == 2 {
        return i..i + 1;
    }
    let mut start = i;
    while start > 0 && class(chars[start - 1]) == k {
        start -= 1;
    }
    let mut end = i + 1;
    while end < chars.len() && class(chars[end]) == k {
        end += 1;
    }
    start..end
}

fn paragraph_range(text: &str, index: usize) -> Range<usize> {
    let chars: Vec<char> = text.chars().collect();
    let i = index.min(chars.len());
    let start = chars[..i]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |p| p + 1);
    let end = chars[i..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |p| i + p);
    start..end
}

fn paint_selection(
    painter: &Painter,
    block: &TextBlock,
    range: Range<usize>,
    continues: bool,
    offset: Vec2,
    color: Color32,
) {
    let origin = block.origin.to_vec2() + offset;
    let rows = &block.galley.rows;
    let mut row_start = 0;
    for (ri, row) in rows.iter().enumerate() {
        let row_end = row_start + row.char_count_excluding_newline().0;
        let next_start = row_start + row.char_count_including_newline().0;
        if range.end <= row_start
            && !(range.end == row_start && row_start == row_end && range.start < row_start)
        {
            break;
        }
        if range.start <= row_end {
            let is_last = ri + 1 == rows.len();
            // The selection runs past the end of this row, through a newline or into the next block.
            let past_end = range.end > row_end && (row.ends_with_newline || (is_last && continues));
            let from = range.start.max(row_start);
            let to = range.end.min(row_end);
            if from < to || past_end {
                let x0 = row.pos.x + row.x_offset(CharIndex(from - row_start));
                let mut x1 = row.pos.x + row.x_offset(CharIndex(to.max(from) - row_start));
                if past_end {
                    x1 += row.height() * 0.3;
                }
                let rect = Rect::from_min_max(pos2(x0, row.min_y()), pos2(x1, row.max_y()));
                painter.rect_filled(rect.translate(origin), 0.0, color);
            }
        }
        row_start = next_start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(word_range("hello world", 2), 0..5);
        assert_eq!(word_range("hello world", 5), 5..6);
        assert_eq!(word_range("hello, world", 5), 5..6);
        assert_eq!(word_range("hello world", 11), 6..11);
    }

    #[test]
    fn paragraphs() {
        assert_eq!(paragraph_range("ab\ncd\nef", 4), 3..5);
        assert_eq!(paragraph_range("abc", 1), 0..3);
    }
}

//! Block, inline and table layout. Produces a flat display list in document coordinates
//! (points, with zoom applied).

use std::{borrow::Cow, collections::HashMap, ops::Range, sync::Arc};

use egui::{
    load::{SizeHint, TexturePoll},
    pos2,
    text::{ByteIndex, CCursor, CharIndex, LayoutJob, LayoutSection, TextFormat, TextWrapping},
    vec2, Align, Color32, Context, FontFamily, FontId, Galley, Pos2, Rect, Shape, Stroke,
    StrokeKind, TextureOptions, Vec2,
};

use crate::{
    boxes::{BlockBox, BlockKind, BoxChild, ImageBox, InlineContent, InlineItem, TableBox},
    css::Length,
    style::{BorderStyle, ComputedStyle, TextAlign, VerticalAlign},
};

/// How bold text (`font-weight` ≥ 600) is rendered.
///
/// The default egui fonts have no bold variant, so by default bold is synthesized.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum BoldMode {
    /// Paint bold glyphs a second time with a small horizontal offset.
    /// Works with any font, including the default egui fonts.
    #[default]
    Synthetic,
    /// Use the `wght` axis of variable fonts (set via [`egui::text::TextFormat::coords`]).
    VariableWeight,
    /// Use separately registered font families for bold text,
    /// e.g. `FontFamily::Name("bold".into())`.
    Family {
        proportional: FontFamily,
        monospace: Option<FontFamily>,
    },
}

/// Font configuration of an [`crate::HtmlView`].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FontOptions {
    pub bold: BoldMode,
    /// Maps lower-case CSS `font-family` names to egui font families.
    ///
    /// Families registered in egui under [`FontFamily::Name`] with the same (lower-case) name
    /// as used in CSS are picked up automatically.
    pub families: Vec<(String, FontFamily)>,
}

pub(crate) enum Item {
    Shape(Shape),
    Text(usize),
    Image {
        rect: Rect,
        uri: String,
        link: Option<Arc<str>>,
    },
    Placeholder,
}

pub(crate) struct TextBlock {
    /// Galley coordinates are relative to this point (top-left of the first line box).
    pub origin: Pos2,
    /// Extra offset for painting glyphs (half-leading when `line-height` is set).
    pub glyph_offset: Vec2,
    pub galley: Arc<Galley>,
    /// Synthetic bold: the bold runs only, painted again with this x-offset.
    pub bold_overlay: Option<(Arc<Galley>, f32)>,
    /// Character ranges that are links.
    pub links: Vec<(Range<usize>, Arc<str>)>,
    /// Inserted before this block's text when copying a multi-block selection.
    pub separator: &'static str,
    pub char_count: usize,
}

impl TextBlock {
    pub fn rect(&self) -> Rect {
        self.galley.rect.translate(self.origin.to_vec2())
    }
}

#[derive(Default)]
pub(crate) struct DocLayout {
    pub items: Vec<Item>,
    pub texts: Vec<TextBlock>,
    /// `id` → y position (points).
    pub anchors: HashMap<String, f32>,
    pub size: Vec2,
    /// Some images are still loading: lay out again once they are ready.
    pub pending_images: bool,
}

pub(crate) struct LayoutParams<'a> {
    pub ctx: &'a Context,
    pub zoom: f32,
    pub fonts: &'a FontOptions,
    /// Size of the viewport in points (for `vw`/`vh`).
    pub viewport: Vec2,
    pub base_uri: Option<&'a str>,
}

pub(crate) fn layout_document(root: &BlockBox, params: &LayoutParams<'_>, width: f32) -> DocLayout {
    let registered = params.ctx.fonts(|f| f.families());
    let mut engine = Engine {
        p: params,
        out: DocLayout::default(),
        registered,
        next_separator: "",
        max_x: width,
        max_y: 0.0,
    };
    let geo = engine.layout_block(root, 0.0, 0.0, width, None, true);
    let mut out = engine.out;
    out.size = vec2(engine.max_x.max(width), geo.rect.bottom().max(engine.max_y)).ceil();
    out
}

struct BlockGeometry {
    rect: Rect,
    bg_slot: Option<usize>,
    border_slot: Option<usize>,
    content_items: usize,
    content_texts: usize,
}

enum ImageLayout {
    Image { uri: String, size: Vec2 },
    Alt(String),
    Nothing,
}

enum Seg<'b> {
    Text {
        text: Cow<'b, str>,
        style: &'b ComputedStyle,
        link: Option<&'b Arc<str>>,
        background: Color32,
    },
    Anchor(&'b str),
    Image {
        uri: String,
        size: Vec2,
        link: Option<&'b Arc<str>>,
    },
}

impl Seg<'_> {
    fn is_blank_text(&self) -> bool {
        matches!(self, Seg::Text { text, .. } if text.trim().is_empty())
    }
}

/// Text decorations of one layout section. They are painted by us rather than by egui,
/// because egui places them relative to the whole line box when `line_height` is set.
#[derive(Clone, Copy, Default)]
struct SectionDeco {
    background: Color32,
    underline: Option<Stroke>,
    overline: Option<Stroke>,
    line_through: Option<Stroke>,
}

impl SectionDeco {
    fn is_none(&self) -> bool {
        self.background == Color32::TRANSPARENT
            && self.underline.is_none()
            && self.overline.is_none()
            && self.line_through.is_none()
    }
}

struct Job {
    job: LayoutJob,
    decos: Vec<SectionDeco>,
    links: Vec<(Range<usize>, Arc<str>)>,
    anchors: Vec<(String, usize)>,
    /// Per section: is it bold (for synthetic bold)?
    bold_sections: Vec<bool>,
    max_bold_size: f32,
    char_count: usize,
}

fn collapse_margins(a: f32, b: f32) -> f32 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else if a < 0.0 && b < 0.0 {
        a.min(b)
    } else {
        a + b
    }
}

fn is_zero(l: Length) -> bool {
    matches!(l, Length::Px(v) | Length::Percent(v) if v == 0.0)
}

struct Engine<'a> {
    p: &'a LayoutParams<'a>,
    out: DocLayout,
    registered: Vec<FontFamily>,
    next_separator: &'static str,
    max_x: f32,
    max_y: f32,
}

impl Engine<'_> {
    fn zoom(&self) -> f32 {
        self.p.zoom
    }

    fn resolve(&self, l: Length, cw: f32) -> Option<f32> {
        let z = self.zoom();
        match l {
            Length::Auto => None,
            Length::Px(v) => Some(v * z),
            Length::Percent(p) => Some(cw * p / 100.0),
            Length::Vw(v) => Some(self.p.viewport.x * v / 100.0),
            Length::Vh(v) => Some(self.p.viewport.y * v / 100.0),
        }
    }

    /// Heights: percentages of an auto-height container are treated as `auto`.
    fn resolve_height(&self, l: Length) -> Option<f32> {
        match l {
            Length::Percent(_) => None,
            other => self.resolve(other, 0.0),
        }
    }

    fn push(&mut self, item: Item) -> usize {
        self.out.items.push(item);
        self.out.items.len() - 1
    }

    // ------------------------------------------------------------------------
    // Blocks

    fn collapses_top(&self, b: &BlockBox) -> bool {
        matches!(b.kind, BlockKind::Normal | BlockKind::ListItem { .. })
            && b.style.border[0].used_width() == 0.0
            && is_zero(b.style.padding[0])
            && matches!(b.children.first(), Some(BoxChild::Block(_)))
    }

    fn collapses_bottom(&self, b: &BlockBox) -> bool {
        matches!(b.kind, BlockKind::Normal | BlockKind::ListItem { .. })
            && b.style.border[2].used_width() == 0.0
            && is_zero(b.style.padding[2])
            && b.style.height.is_auto()
            && matches!(b.style.min_height, Length::Auto | Length::Px(0.0))
            && matches!(b.children.last(), Some(BoxChild::Block(_)))
    }

    /// Top margin of `b`, collapsed with the top margins of its first descendants.
    fn collapsed_margin_top(&self, b: &BlockBox, cw: f32) -> f32 {
        let own = self.resolve(b.style.margin[0], cw).unwrap_or(0.0);
        if self.collapses_top(b) {
            if let Some(BoxChild::Block(first)) = b.children.first() {
                return collapse_margins(own, self.collapsed_margin_top(first, cw));
            }
        }
        own
    }

    fn collapsed_margin_bottom(&self, b: &BlockBox, cw: f32) -> f32 {
        let own = self.resolve(b.style.margin[2], cw).unwrap_or(0.0);
        if self.collapses_bottom(b) {
            if let Some(BoxChild::Block(last)) = b.children.last() {
                return collapse_margins(own, self.collapsed_margin_bottom(last, cw));
            }
        }
        own
    }

    fn paddings(&self, s: &ComputedStyle, cw: f32) -> [f32; 4] {
        [0, 1, 2, 3].map(|i| self.resolve(s.padding[i], cw).unwrap_or(0.0).max(0.0))
    }

    fn borders(&self, s: &ComputedStyle) -> [f32; 4] {
        [0, 1, 2, 3].map(|i| s.border[i].used_width() * self.zoom())
    }

    /// Lays out a block whose border box starts at `y`, inside a containing block spanning
    /// `x..x + cw`. Margins on the block axis are handled by the caller.
    ///
    /// `forced_width` is the border-box width (used for table cells).
    /// `bfc` is true for boxes whose margins never collapse with their children.
    fn layout_block(
        &mut self,
        b: &BlockBox,
        x: f32,
        y: f32,
        cw: f32,
        forced_width: Option<f32>,
        bfc: bool,
    ) -> BlockGeometry {
        let s = &*b.style;
        let pad = self.paddings(s, cw);
        let bor = self.borders(s);
        let pb_h = pad[1] + pad[3] + bor[1] + bor[3];
        let pb_v = pad[0] + pad[2] + bor[0] + bor[2];

        let mut ml = self.resolve(s.margin[3], cw);
        let mr = self.resolve(s.margin[1], cw);

        let mut image = None;
        let content_w;
        if let Some(fw) = forced_width {
            content_w = (fw - pb_h).max(0.0);
            ml = Some(0.0);
        } else {
            let avail = cw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - pb_h;
            let to_content = |w: f32| if s.border_box { w - pb_h } else { w };
            let specified = self.resolve(s.width, cw).map(to_content);
            let mut explicit = specified.is_some();
            let mut w = match (&b.kind, specified) {
                (_, Some(w)) => w,
                (BlockKind::Table(t), None) => {
                    explicit = true;
                    self.table_columns(t, s, avail.max(0.0), None).1
                }
                (BlockKind::Image(img), None) => {
                    explicit = true;
                    let layout = self.image_layout(img, avail.max(0.0));
                    let w = match &layout {
                        ImageLayout::Image { size, .. } => size.x,
                        _ => avail,
                    };
                    image = Some(layout);
                    w
                }
                _ => avail,
            };
            if let Some(max_w) = self.resolve(s.max_width, cw).map(to_content) {
                if w > max_w {
                    w = max_w;
                    explicit = true;
                }
            }
            if let Some(min_w) = self.resolve(s.min_width, cw).map(to_content) {
                w = w.max(min_w);
            }
            content_w = w.max(0.0);
            if explicit {
                let remaining =
                    (cw - content_w - pb_h - ml.unwrap_or(0.0) - mr.unwrap_or(0.0)).max(0.0);
                match (ml, mr) {
                    (None, None) => ml = Some(remaining / 2.0),
                    (None, Some(_)) => ml = Some(remaining),
                    _ => {}
                }
            }
        }
        let bx = x + ml.unwrap_or(0.0);
        let border_w = content_w + pb_h;

        let bg_slot = (s.background.a() > 0 && s.visible).then(|| self.push(Item::Placeholder));
        let border_slot =
            (bor.iter().any(|&w| w > 0.0) && s.visible).then(|| self.push(Item::Placeholder));
        let content_items = self.out.items.len();
        let content_texts = self.out.texts.len();
        if let Some(anchor) = &b.anchor {
            self.out.anchors.entry(anchor.clone()).or_insert(y);
        }

        let cx = bx + bor[3] + pad[3];
        let cy = y + bor[0] + pad[0];
        let mut content_h = match &b.kind {
            BlockKind::Normal | BlockKind::ListItem { .. } => {
                self.layout_children(b, cx, cy, content_w, bfc)
            }
            BlockKind::Table(t) => {
                let (cols, _) = self.table_columns(t, s, content_w, Some(content_w));
                self.layout_table(t, &cols, cx, cy, content_w, s)
            }
            BlockKind::Image(img) => {
                let layout = image.unwrap_or_else(|| self.image_layout(img, content_w));
                match layout {
                    ImageLayout::Image { uri, size } => {
                        let rect = Rect::from_min_size(pos2(cx, cy), size);
                        self.add_image(rect, uri, img.link.as_ref());
                        size.y
                    }
                    ImageLayout::Alt(alt) => {
                        let segs = [Seg::Text {
                            text: Cow::Owned(alt),
                            style: &img.style,
                            link: img.link.as_ref(),
                            background: Color32::TRANSPARENT,
                        }];
                        self.layout_text_run(&segs, cx, cy, content_w, s, true)
                    }
                    ImageLayout::Nothing => 0.0,
                }
            }
        };
        let to_content_h = |h: f32| if s.border_box { h - pb_v } else { h };
        if let Some(h) = self.resolve_height(s.height) {
            content_h = to_content_h(h);
        }
        if let Some(min_h) = self.resolve_height(s.min_height) {
            content_h = content_h.max(to_content_h(min_h));
        }
        let height = content_h.max(0.0) + pb_v;

        let rect = Rect::from_min_size(pos2(bx, y), vec2(border_w, height));
        self.max_x = self.max_x.max(rect.right());
        let geo = BlockGeometry {
            rect,
            bg_slot,
            border_slot,
            content_items,
            content_texts,
        };
        self.decorate(s, &geo);
        if let BlockKind::ListItem {
            marker: Some(marker),
        } = &b.kind
        {
            self.place_marker(marker, s, cx, cy, content_texts);
        }
        geo
    }

    fn decorate(&mut self, s: &ComputedStyle, geo: &BlockGeometry) {
        let radius = s.border_radius * self.zoom();
        if let Some(slot) = geo.bg_slot {
            self.out.items[slot] = Item::Shape(Shape::rect_filled(geo.rect, radius, s.background));
        }
        if let Some(slot) = geo.border_slot {
            let w = self.borders(s);
            let colors = [0, 1, 2, 3].map(|i| s.border_color(i));
            let uniform = w.iter().all(|&v| v == w[0])
                && colors.iter().all(|&c| c == colors[0])
                && s.border.iter().all(|b| b.style == BorderStyle::Solid);
            let r = geo.rect;
            let shape = if uniform {
                Shape::rect_stroke(r, radius, Stroke::new(w[0], colors[0]), StrokeKind::Inside)
            } else {
                let mut shapes = Vec::new();
                let sides = [
                    Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + w[0])),
                    Rect::from_min_max(pos2(r.right() - w[1], r.top()), r.right_bottom()),
                    Rect::from_min_max(pos2(r.left(), r.bottom() - w[2]), r.right_bottom()),
                    Rect::from_min_max(r.left_top(), pos2(r.left() + w[3], r.bottom())),
                ];
                for (i, side) in sides.into_iter().enumerate() {
                    if w[i] > 0.0 {
                        side_shapes(&mut shapes, side, s.border[i].style, colors[i], i % 2 == 0);
                    }
                }
                Shape::Vec(shapes)
            };
            self.out.items[slot] = Item::Shape(shape);
        }
    }

    /// Lays out the children of `b` in its content box; returns the content height.
    fn layout_children(&mut self, b: &BlockBox, x: f32, y: f32, w: f32, bfc: bool) -> f32 {
        let children = &b.children;
        let style = &*b.style;
        // Margins of the first/last child that collapse through `b` were already applied
        // by `b`'s parent.
        let through_top = !bfc && self.collapses_top(b);
        let through_bottom = !bfc && self.collapses_bottom(b);
        let mut cursor = y;
        let mut pending_margin = 0.0;
        let last = children.len().saturating_sub(1);
        for (i, child) in children.iter().enumerate() {
            match child {
                BoxChild::Block(cb) => {
                    if cb.is_empty_collapsible() {
                        let mt = self.resolve(cb.style.margin[0], w).unwrap_or(0.0);
                        let mb = self.resolve(cb.style.margin[2], w).unwrap_or(0.0);
                        if let Some(anchor) = &cb.anchor {
                            self.out.anchors.entry(anchor.clone()).or_insert(cursor);
                        }
                        pending_margin = collapse_margins(pending_margin, collapse_margins(mt, mb));
                        continue;
                    }
                    let mut mt = self.collapsed_margin_top(cb, w);
                    if i == 0 && through_top {
                        mt = 0.0;
                    }
                    let top = cursor + collapse_margins(pending_margin, mt);
                    self.next_separator = "\n";
                    let geo = self.layout_block(cb, x, top, w, None, false);
                    cursor = geo.rect.bottom();
                    pending_margin = if i == last && through_bottom {
                        0.0
                    } else {
                        self.collapsed_margin_bottom(cb, w)
                    };
                }
                BoxChild::Inline(content) => {
                    cursor += pending_margin;
                    pending_margin = 0.0;
                    cursor += self.layout_inline(content, x, cursor, w, style);
                }
            }
        }
        cursor + pending_margin - y
    }

    // ------------------------------------------------------------------------
    // Inline content

    fn segments<'b>(&mut self, content: &'b InlineContent, avail_w: f32) -> Vec<Seg<'b>> {
        let mut segs = Vec::with_capacity(content.items.len());
        for item in &content.items {
            match item {
                InlineItem::Text {
                    text,
                    style,
                    link,
                    background,
                } => segs.push(Seg::Text {
                    text: Cow::Borrowed(text),
                    style,
                    link: link.as_ref(),
                    background: *background,
                }),
                InlineItem::Anchor(id) => segs.push(Seg::Anchor(id)),
                InlineItem::Image(img) => match self.image_layout(img, avail_w) {
                    ImageLayout::Image { uri, size } => segs.push(Seg::Image {
                        uri,
                        size,
                        link: img.link.as_ref(),
                    }),
                    ImageLayout::Alt(alt) => segs.push(Seg::Text {
                        text: Cow::Owned(alt),
                        style: &img.style,
                        link: img.link.as_ref(),
                        background: Color32::TRANSPARENT,
                    }),
                    ImageLayout::Nothing => {}
                },
            }
        }
        segs
    }

    fn layout_inline(
        &mut self,
        content: &InlineContent,
        x: f32,
        y: f32,
        w: f32,
        block_style: &ComputedStyle,
    ) -> f32 {
        let segs = self.segments(content, w);
        let mut cursor = y;
        let mut first = true;
        let mut i = 0;
        while i < segs.len() {
            let start = i;
            while i < segs.len() && !matches!(segs[i], Seg::Image { .. }) {
                i += 1;
            }
            // Leave whitespace that only separates images out of the text run.
            let mut end = i;
            while end > start && i < segs.len() && segs[end - 1].is_blank_text() {
                end -= 1;
            }
            if end > start {
                cursor += self.layout_text_run(&segs[start..end], x, cursor, w, block_style, first);
                first = false;
            }

            let mut images = Vec::new();
            while i < segs.len() {
                match &segs[i] {
                    Seg::Image { uri, size, link } => images.push((uri.clone(), *size, *link)),
                    seg if seg.is_blank_text()
                        && segs
                            .get(i + 1)
                            .is_some_and(|s| matches!(s, Seg::Image { .. })) => {}
                    _ => break,
                }
                i += 1;
            }
            if !images.is_empty() {
                cursor += self.layout_image_run(&images, x, cursor, w, block_style);
                first = false;
            }
        }
        cursor - y
    }

    /// Lays out images side by side, wrapping onto new rows as needed.
    fn layout_image_run(
        &mut self,
        images: &[(String, Vec2, Option<&Arc<str>>)],
        x: f32,
        y: f32,
        w: f32,
        block_style: &ComputedStyle,
    ) -> f32 {
        let gap = block_style.font_size * self.zoom() * 0.25;
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut row_w = 0.0;
        for (i, (_, size, _)) in images.iter().enumerate() {
            let needed = if rows.last().is_some_and(|r| r.is_empty()) {
                size.x
            } else {
                gap + size.x
            };
            if row_w + needed > w && !rows.last().is_some_and(|r| r.is_empty()) {
                rows.push(Vec::new());
                row_w = size.x;
            } else {
                row_w += needed;
            }
            rows.last_mut().unwrap().push(i);
        }
        let mut cursor = y;
        for row in rows {
            let total: f32 = row.iter().map(|&i| images[i].1.x).sum::<f32>()
                + gap * row.len().saturating_sub(1) as f32;
            let height = row.iter().map(|&i| images[i].1.y).fold(0.0, f32::max);
            let mut cx = match block_style.text_align {
                TextAlign::Center => x + ((w - total) / 2.0).max(0.0),
                TextAlign::Right => x + (w - total).max(0.0),
                _ => x,
            };
            for i in row {
                let (uri, size, link) = &images[i];
                let rect = Rect::from_min_size(pos2(cx, cursor + height - size.y), *size);
                self.add_image(rect, uri.clone(), *link);
                cx += size.x + gap;
            }
            cursor += height;
        }
        cursor - y
    }

    fn add_image(&mut self, rect: Rect, uri: String, link: Option<&Arc<str>>) {
        self.max_x = self.max_x.max(rect.right());
        self.max_y = self.max_y.max(rect.bottom());
        self.push(Item::Image {
            rect,
            uri,
            link: link.cloned(),
        });
    }

    fn family(&self, style: &ComputedStyle) -> FontFamily {
        let bold = style.is_bold();
        let monospace = style.is_monospace();
        if bold {
            if let BoldMode::Family {
                proportional,
                monospace: mono_bold,
            } = &self.p.fonts.bold
            {
                return match (monospace, mono_bold) {
                    (true, Some(m)) => m.clone(),
                    (true, None) => FontFamily::Monospace,
                    (false, _) => proportional.clone(),
                };
            }
        }
        for name in style.font_family.iter() {
            if let Some((_, family)) = self.p.fonts.families.iter().find(|(n, _)| n == name) {
                return family.clone();
            }
            if let Some(family) = self
                .registered
                .iter()
                .find(|f| matches!(f, FontFamily::Name(n) if n.eq_ignore_ascii_case(name)))
            {
                return family.clone();
            }
        }
        if monospace {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        }
    }

    fn font_id(&self, style: &ComputedStyle) -> FontId {
        FontId::new((style.font_size * self.zoom()).max(1.0), self.family(style))
    }

    fn text_format(&self, style: &ComputedStyle) -> TextFormat {
        let z = self.zoom();
        let mut format = TextFormat {
            font_id: self.font_id(style),
            extra_letter_spacing: style.letter_spacing * z,
            line_height: style.line_height_px().map(|l| (l * z).max(1.0)),
            color: if style.visible {
                style.color
            } else {
                Color32::TRANSPARENT
            },
            italics: style.italic,
            valign: match style.vertical_align {
                VerticalAlign::Super | VerticalAlign::Top => Align::TOP,
                VerticalAlign::Middle => Align::Center,
                _ => Align::BOTTOM,
            },
            ..Default::default()
        };
        if self.p.fonts.bold == BoldMode::VariableWeight && style.font_weight != 400 {
            format.coords.push(b"wght", f32::from(style.font_weight));
        }
        format
    }

    fn section_deco(&self, style: &ComputedStyle, background: Color32) -> SectionDeco {
        if !style.visible {
            return SectionDeco::default();
        }
        let deco = &style.decoration;
        let size = style.font_size * self.zoom();
        let stroke = Stroke::new((size / 14.0).max(1.0), deco.color.unwrap_or(style.color));
        SectionDeco {
            background,
            underline: deco.underline.then_some(stroke),
            overline: deco.overline.then_some(stroke),
            line_through: deco.line_through.then_some(stroke),
        }
    }

    fn build_job(&self, segs: &[Seg<'_>], wrap_width: f32, align: TextAlign, indent: f32) -> Job {
        let mut job = LayoutJob {
            wrap: TextWrapping {
                max_width: wrap_width,
                ..Default::default()
            },
            halign: match align {
                TextAlign::Center => Align::Center,
                TextAlign::Right => Align::RIGHT,
                _ => Align::LEFT,
            },
            justify: align == TextAlign::Justify,
            ..Default::default()
        };
        let synthetic_bold = self.p.fonts.bold == BoldMode::Synthetic;
        let mut links: Vec<(Range<usize>, Arc<str>)> = Vec::new();
        let mut anchors = Vec::new();
        let mut decos = Vec::new();
        let mut bold_sections = Vec::new();
        let mut max_bold_size: f32 = 0.0;
        let mut chars = 0;
        for seg in segs {
            match seg {
                Seg::Text {
                    text,
                    style,
                    link,
                    background,
                } => {
                    if text.is_empty() {
                        continue;
                    }
                    let n = text.chars().count();
                    if let Some(link) = link {
                        match links.last_mut() {
                            Some((range, l)) if range.end == chars && Arc::ptr_eq(l, link) => {
                                range.end = chars + n;
                            }
                            _ => links.push((chars..chars + n, (*link).clone())),
                        }
                    }
                    let format = self.text_format(style);
                    let bold = synthetic_bold && style.is_bold();
                    if bold {
                        max_bold_size = max_bold_size.max(format.font_id.size);
                    }
                    let leading = if job.sections.is_empty() { indent } else { 0.0 };
                    // Sections are never merged so that the synthetic-bold overlay job gets
                    // exactly the same shaping and line breaks.
                    let start = job.text.len();
                    job.text.push_str(text);
                    job.sections.push(LayoutSection {
                        leading_space: leading,
                        byte_range: ByteIndex(start)..ByteIndex(job.text.len()),
                        format,
                    });
                    bold_sections.push(bold);
                    decos.push(self.section_deco(style, *background));
                    chars += n;
                }
                Seg::Anchor(id) => anchors.push(((*id).to_owned(), chars)),
                Seg::Image { .. } => {}
            }
        }
        // A trailing `<br>` does not create an empty line in browsers.
        if job.text.ends_with('\n') && job.text.len() > 1 {
            job.text.pop();
            if let Some(last) = job.sections.last_mut() {
                last.byte_range.end = ByteIndex(job.text.len());
            }
            if job.sections.last().is_some_and(|s| s.byte_range.is_empty()) {
                job.sections.pop();
                bold_sections.pop();
                decos.pop();
            }
            chars -= 1;
        }
        Job {
            job,
            decos,
            links,
            anchors,
            bold_sections,
            max_bold_size,
            char_count: chars,
        }
    }

    fn layout_job(&self, job: LayoutJob) -> Arc<Galley> {
        self.p.ctx.fonts_mut(|f| f.layout_job(job))
    }

    fn layout_text_run(
        &mut self,
        segs: &[Seg<'_>],
        x: f32,
        y: f32,
        w: f32,
        block_style: &ComputedStyle,
        first_line: bool,
    ) -> f32 {
        let wrap_width = if block_style.white_space.wraps() {
            w.max(1.0)
        } else {
            f32::INFINITY
        };
        let indent = if first_line {
            self.resolve(block_style.text_indent, w)
                .unwrap_or(0.0)
                .max(0.0)
        } else {
            0.0
        };
        let Job {
            job,
            decos,
            links,
            anchors,
            bold_sections,
            max_bold_size,
            char_count,
        } = self.build_job(segs, wrap_width, block_style.text_align, indent);
        if job.sections.is_empty() {
            for (id, _) in anchors {
                self.out.anchors.entry(id).or_insert(y);
            }
            return 0.0;
        }

        let bold_overlay = (max_bold_size > 0.0).then(|| {
            let mut overlay = job.clone();
            for (section, &bold) in overlay.sections.iter_mut().zip(&bold_sections) {
                if !bold {
                    section.format.color = Color32::TRANSPARENT;
                }
            }
            let offset = (max_bold_size * 0.05).clamp(0.5, 3.0);
            (self.layout_job(overlay), offset)
        });

        // `line-height`: egui puts the extra space below the text; CSS splits it evenly.
        let glyph_offset = match block_style.line_height_px() {
            Some(lh) => {
                let font_id = self.font_id(block_style);
                let natural = self.p.ctx.fonts_mut(|f| f.row_height(&font_id));
                vec2(0.0, ((lh * self.zoom() - natural) / 2.0).round())
            }
            None => Vec2::ZERO,
        };

        let halign = job.halign;
        let galley = self.layout_job(job);
        let origin_x = match halign {
            Align::Center => x + w / 2.0,
            Align::RIGHT => x + w,
            _ => x,
        };
        let origin = pos2(origin_x, y);
        for (id, index) in anchors {
            let pos = galley.pos_from_cursor(CCursor::new(CharIndex(index)));
            self.out.anchors.entry(id).or_insert(y + pos.min.y);
        }
        let height = galley.size().y;
        let block = TextBlock {
            origin,
            glyph_offset,
            galley,
            bold_overlay,
            links,
            separator: std::mem::replace(&mut self.next_separator, "\n"),
            char_count,
        };
        let rect = block.rect();
        self.max_x = self.max_x.max(rect.right());
        self.max_y = self.max_y.max(rect.bottom());

        let mut backgrounds = Vec::new();
        let mut lines = Vec::new();
        if decos.iter().any(|d| !d.is_none()) {
            let ppp = self.p.ctx.pixels_per_point();
            decoration_shapes(
                &block.galley,
                &decos,
                (origin + glyph_offset).to_vec2(),
                ppp,
                &mut backgrounds,
                &mut lines,
            );
        }
        for shape in backgrounds {
            self.push(Item::Shape(shape));
        }
        self.out.texts.push(block);
        let index = self.out.texts.len() - 1;
        self.push(Item::Text(index));
        for shape in lines {
            self.push(Item::Shape(shape));
        }
        height
    }

    /// (min-content, max-content) width of inline content.
    fn measure_inline(
        &mut self,
        content: &InlineContent,
        block_style: &ComputedStyle,
    ) -> (f32, f32) {
        let segs = self.segments(content, f32::INFINITY);
        let gap = block_style.font_size * self.zoom() * 0.25;
        let (mut min_w, mut max_w): (f32, f32) = (0.0, 0.0);
        let mut i = 0;
        while i < segs.len() {
            let start = i;
            while i < segs.len() && !matches!(segs[i], Seg::Image { .. }) {
                i += 1;
            }
            if i > start {
                let job = self.build_job(&segs[start..i], f32::INFINITY, TextAlign::Left, 0.0);
                if !job.job.sections.is_empty() {
                    let galley = self.layout_job(job.job);
                    max_w = max_w.max(galley.size().x);
                    min_w = min_w.max(longest_word(&galley));
                }
            }
            let mut run = 0.0;
            while i < segs.len() {
                match &segs[i] {
                    Seg::Image { size, .. } => {
                        min_w = min_w.max(size.x);
                        run += size.x + gap;
                    }
                    seg if seg.is_blank_text() => {}
                    _ => break,
                }
                i += 1;
            }
            max_w = max_w.max(run - gap);
        }
        (min_w.ceil(), max_w.ceil())
    }

    /// (min-content, max-content) border-box width of a block including horizontal margins.
    fn measure_block(&mut self, b: &BlockBox, cw: f32) -> (f32, f32) {
        let s = &*b.style;
        let pad = self.paddings(s, cw);
        let bor = self.borders(s);
        let margins = self.resolve(s.margin[1], cw).unwrap_or(0.0)
            + self.resolve(s.margin[3], cw).unwrap_or(0.0);
        let pb = pad[1] + pad[3] + bor[1] + bor[3];
        let extra = margins + if s.border_box { 0.0 } else { pb };
        if let Some(w) = self.resolve(s.width, cw) {
            // Explicit widths are both the minimum and the maximum.
            let w = w + extra;
            return (w, w);
        }
        let (mut min_w, mut max_w) = match &b.kind {
            BlockKind::Image(img) => match self.image_layout(img, cw) {
                ImageLayout::Image { size, .. } => (size.x, size.x),
                ImageLayout::Alt(_) | ImageLayout::Nothing => (0.0, 0.0),
            },
            BlockKind::Table(t) => {
                let (cols, _) = self.table_columns(t, s, f32::INFINITY, None);
                let spacing = s.border_spacing * self.zoom() * (cols.len() + 1) as f32;
                let min = self.table_min_width(t, s, cw);
                (min, cols.iter().sum::<f32>() + spacing)
            }
            BlockKind::Normal | BlockKind::ListItem { .. } => {
                let (mut mn, mut mx): (f32, f32) = (0.0, 0.0);
                for child in &b.children {
                    let (a, b) = match child {
                        BoxChild::Block(cb) => self.measure_block(cb, cw),
                        BoxChild::Inline(ic) => self.measure_inline(ic, s),
                    };
                    mn = mn.max(a);
                    mx = mx.max(b);
                }
                (mn, mx)
            }
        };
        if let Some(max) = self.resolve(s.max_width, cw) {
            max_w = max_w.min(max);
            min_w = min_w.min(max);
        }
        if let Some(min) = self.resolve(s.min_width, cw) {
            max_w = max_w.max(min);
            min_w = min_w.max(min);
        }
        (min_w + extra, max_w.max(min_w) + extra)
    }

    // ------------------------------------------------------------------------
    // Tables

    fn column_count(t: &TableBox) -> usize {
        t.rows
            .iter()
            .map(|r| r.cells.iter().map(|c| c.colspan).sum::<usize>())
            .max()
            .unwrap_or(0)
    }

    fn column_bounds(&mut self, t: &TableBox, s: &ComputedStyle, cw: f32) -> (Vec<f32>, Vec<f32>) {
        let ncols = Self::column_count(t);
        let spacing = s.border_spacing * self.zoom();
        let mut mins = vec![0.0f32; ncols];
        let mut maxs = vec![0.0f32; ncols];
        let mut spanning = Vec::new();
        for row in &t.rows {
            let mut col = 0;
            for cell in &row.cells {
                if col >= ncols {
                    break;
                }
                let span = cell.colspan.min(ncols - col);
                let (mn, mx) = self.measure_block(&cell.block, cw);
                if span == 1 {
                    mins[col] = mins[col].max(mn);
                    maxs[col] = maxs[col].max(mx);
                } else {
                    spanning.push((col, span, mn, mx));
                }
                col += span;
            }
        }
        for (col, span, mn, mx) in spanning {
            let gaps = spacing * (span - 1) as f32;
            for (bounds, needed) in [(&mut mins, mn), (&mut maxs, mx)] {
                let have: f32 = bounds[col..col + span].iter().sum::<f32>() + gaps;
                if needed > have {
                    let extra = (needed - have) / span as f32;
                    for b in &mut bounds[col..col + span] {
                        *b += extra;
                    }
                }
            }
        }
        for (mn, mx) in mins.iter().zip(maxs.iter_mut()) {
            *mx = mx.max(*mn);
        }
        (mins, maxs)
    }

    fn table_min_width(&mut self, t: &TableBox, s: &ComputedStyle, cw: f32) -> f32 {
        let (mins, _) = self.column_bounds(t, s, cw);
        mins.iter().sum::<f32>() + s.border_spacing * self.zoom() * (mins.len() + 1) as f32
    }

    /// Returns the column widths and the table's content width.
    /// `explicit` is the specified content width; otherwise the table shrinks to fit `avail`.
    fn table_columns(
        &mut self,
        t: &TableBox,
        s: &ComputedStyle,
        avail: f32,
        explicit: Option<f32>,
    ) -> (Vec<f32>, f32) {
        let ncols = Self::column_count(t);
        let spacing = s.border_spacing * self.zoom();
        if ncols == 0 {
            return (Vec::new(), explicit.unwrap_or(0.0));
        }
        let cw = explicit.unwrap_or(avail);
        let (mins, maxs) = self.column_bounds(t, s, if cw.is_finite() { cw } else { 0.0 });
        let total_spacing = spacing * (ncols + 1) as f32;
        let sum_min: f32 = mins.iter().sum::<f32>() + total_spacing;
        let sum_max: f32 = maxs.iter().sum::<f32>() + total_spacing;
        let target = match explicit {
            Some(w) => w.max(sum_min),
            None => avail.min(sum_max).max(sum_min),
        };
        let widths = if target >= sum_max {
            let extra = target - sum_max;
            let total_max: f32 = maxs.iter().sum();
            maxs.iter()
                .map(|&m| {
                    if total_max > 0.0 {
                        m + extra * m / total_max
                    } else {
                        m + extra / ncols as f32
                    }
                })
                .collect()
        } else {
            let t_ = (target - sum_min) / (sum_max - sum_min).max(f32::EPSILON);
            mins.iter()
                .zip(&maxs)
                .map(|(&mn, &mx)| mn + (mx - mn) * t_)
                .collect()
        };
        (widths, target)
    }

    fn layout_table(
        &mut self,
        t: &TableBox,
        cols: &[f32],
        x: f32,
        y: f32,
        width: f32,
        s: &ComputedStyle,
    ) -> f32 {
        let spacing = s.border_spacing * self.zoom();
        let ncols = cols.len();
        let mut cursor = y;
        for caption in &t.captions {
            self.next_separator = "\n";
            let geo = self.layout_block(caption, x, cursor, width, None, true);
            cursor = geo.rect.bottom();
        }
        cursor += spacing;
        for row in &t.rows {
            let row_bg = (row.style.background.a() > 0 && row.style.visible)
                .then(|| self.push(Item::Placeholder));
            let row_top = cursor;
            let mut cells = Vec::new();
            let mut col = 0;
            let mut cx = x + spacing;
            for (ci, cell) in row.cells.iter().enumerate() {
                if col >= ncols {
                    break;
                }
                let span = cell.colspan.min(ncols - col);
                let w = cols[col..col + span].iter().sum::<f32>() + spacing * (span - 1) as f32;
                self.next_separator = if ci == 0 { "\n" } else { "\t" };
                let geo = self.layout_block(&cell.block, cx, row_top, w, Some(w), true);
                cells.push((geo, &cell.block));
                cx += w + spacing;
                col += span;
            }
            let row_h = cells
                .iter()
                .map(|(g, _)| g.rect.height())
                .fold(0.0, f32::max);
            for (mut geo, block) in cells {
                let h = geo.rect.height();
                if h < row_h {
                    let dy = match block.style.vertical_align {
                        VerticalAlign::Middle => ((row_h - h) / 2.0).round(),
                        VerticalAlign::Bottom => row_h - h,
                        _ => 0.0,
                    };
                    if dy > 0.0 {
                        self.translate_content(geo.content_items, geo.content_texts, dy);
                    }
                    geo.rect.set_height(row_h);
                    self.decorate(&block.style, &geo);
                }
            }
            if let Some(slot) = row_bg {
                let rect = Rect::from_min_max(
                    pos2(x + spacing, row_top),
                    pos2(x + width - spacing, row_top + row_h),
                );
                self.out.items[slot] =
                    Item::Shape(Shape::rect_filled(rect, 0.0, row.style.background));
            }
            cursor = row_top + row_h + spacing;
        }
        cursor - y
    }

    fn translate_content(&mut self, items_from: usize, texts_from: usize, dy: f32) {
        let delta = vec2(0.0, dy);
        for item in &mut self.out.items[items_from..] {
            match item {
                Item::Shape(shape) => shape.translate(delta),
                Item::Image { rect, .. } => *rect = rect.translate(delta),
                Item::Text(_) | Item::Placeholder => {}
            }
        }
        for text in &mut self.out.texts[texts_from..] {
            text.origin += delta;
        }
    }

    // ------------------------------------------------------------------------
    // Lists and images

    fn place_marker(
        &mut self,
        marker: &str,
        s: &ComputedStyle,
        cx: f32,
        cy: f32,
        first_text: usize,
    ) {
        let z = self.zoom();
        let font_id = self.font_id(s);
        let size = font_id.size;
        // Baseline of the first line of the list item, if it has text.
        let baseline = self.out.texts.get(first_text).and_then(|t| {
            let row = t.galley.rows.first()?;
            let glyph = row.glyphs.first()?;
            Some(t.origin.y + t.glyph_offset.y + row.pos.y + glyph.pos.y)
        });
        let baseline =
            baseline.unwrap_or_else(|| cy + self.p.ctx.fonts_mut(|f| f.row_height(&font_id)) * 0.8);
        let color = s.color;
        let gap = 0.5 * size;
        match marker {
            "•" | "◦" | "▪" => {
                let r = (size * 0.18).max(1.5);
                let center = pos2(cx - gap - r, baseline - size * 0.3);
                let shape = match marker {
                    "•" => Shape::circle_filled(center, r, color),
                    "◦" => Shape::circle_stroke(
                        center,
                        r - 0.5,
                        Stroke::new(1.0_f32.max(z * 0.8), color),
                    ),
                    _ => Shape::rect_filled(
                        Rect::from_center_size(center, Vec2::splat(r * 1.8)),
                        0.0,
                        color,
                    ),
                };
                self.push(Item::Shape(shape));
            }
            text => {
                let galley = self.layout_job(LayoutJob::simple_singleline(
                    text.to_owned(),
                    font_id,
                    color,
                ));
                let glyph_baseline = galley
                    .rows
                    .first()
                    .and_then(|r| r.glyphs.first().map(|g| r.pos.y + g.pos.y))
                    .unwrap_or(0.0);
                let pos = pos2(cx - gap * 0.6 - galley.size().x, baseline - glyph_baseline);
                self.push(Item::Shape(Shape::galley(pos, galley, color)));
            }
        }
    }

    fn resolve_uri(&self, src: &str) -> String {
        if src.starts_with("data:") {
            return register_data_uri(self.p.ctx, src).unwrap_or_default();
        }
        if src.contains("://") || src.starts_with("bytes:") {
            return src.to_owned();
        }
        match self.p.base_uri {
            Some(base) if src.starts_with('/') => {
                // Keep scheme and authority of the base.
                let after_scheme = base.find("://").map_or(0, |i| i + 3);
                let authority_end = base[after_scheme..]
                    .find('/')
                    .map_or(base.len(), |i| after_scheme + i);
                format!("{}{}", &base[..authority_end], src)
            }
            Some(base) => {
                let dir = match base.rfind('/') {
                    Some(i) if i + 1 > base.find("://").map_or(0, |s| s + 3) => &base[..=i],
                    _ => base,
                };
                let dir = if dir.ends_with('/') {
                    dir.to_owned()
                } else {
                    format!("{dir}/")
                };
                format!("{dir}{}", src.trim_start_matches("./"))
            }
            None => src.to_owned(),
        }
    }

    fn image_layout(&mut self, img: &ImageBox, avail_w: f32) -> ImageLayout {
        let z = self.zoom();
        let alt = || {
            if img.alt.trim().is_empty() {
                ImageLayout::Nothing
            } else {
                ImageLayout::Alt(img.alt.clone())
            }
        };
        if img.src.is_empty() {
            return alt();
        }
        let uri = self.resolve_uri(&img.src);
        if uri.is_empty() {
            return alt();
        }
        let intrinsic =
            match self
                .p
                .ctx
                .try_load_texture(&uri, TextureOptions::LINEAR, SizeHint::default())
            {
                Ok(TexturePoll::Ready { texture }) => Some(texture.size),
                Ok(TexturePoll::Pending { size }) => {
                    self.out.pending_images = true;
                    size
                }
                Err(_) => return alt(),
            };
        let cw = if avail_w.is_finite() { avail_w } else { 0.0 };
        let w = self.resolve(img.style.width, cw);
        let h = self.resolve_height(img.style.height);
        let mut size = match (w, h, intrinsic) {
            (Some(w), Some(h), _) => vec2(w, h),
            (Some(w), None, Some(i)) if i.x > 0.0 => vec2(w, w * i.y / i.x),
            (None, Some(h), Some(i)) if i.y > 0.0 => vec2(h * i.x / i.y, h),
            (None, None, Some(i)) => i * z,
            (Some(w), None, _) => vec2(w, w),
            (None, Some(h), _) => vec2(h, h),
            // Still loading and no size given: reserve a small box.
            (None, None, None) => Vec2::splat(16.0 * z),
        };
        if let Some(max_w) = self.resolve(img.style.max_width, cw) {
            if size.x > max_w && size.x > 0.0 {
                size *= max_w / size.x;
            }
        }
        // Like `max-width: 100%`: never overflow the container.
        if avail_w.is_finite() && size.x > avail_w && size.x > 0.0 && avail_w > 0.0 {
            size *= avail_w / size.x;
        }
        ImageLayout::Image { uri, size }
    }
}

/// Paints one border side; dashed/dotted borders are drawn as segments.
fn side_shapes(
    out: &mut Vec<Shape>,
    side: Rect,
    style: BorderStyle,
    color: Color32,
    horizontal: bool,
) {
    match style {
        BorderStyle::Dashed | BorderStyle::Dotted => {
            let thickness = if horizontal {
                side.height()
            } else {
                side.width()
            };
            let (dash, gap) = if style == BorderStyle::Dotted {
                (thickness, thickness)
            } else {
                (thickness * 3.0, thickness * 2.0)
            };
            let length = if horizontal {
                side.width()
            } else {
                side.height()
            };
            let mut t = 0.0;
            while t < length {
                let end = (t + dash).min(length);
                let r = if horizontal {
                    Rect::from_min_max(
                        pos2(side.left() + t, side.top()),
                        pos2(side.left() + end, side.bottom()),
                    )
                } else {
                    Rect::from_min_max(
                        pos2(side.left(), side.top() + t),
                        pos2(side.right(), side.top() + end),
                    )
                };
                out.push(Shape::rect_filled(r, 0.0, color));
                t += dash + gap;
            }
        }
        _ => out.push(Shape::rect_filled(side, 0.0, color)),
    }
}

/// Backgrounds and lines (underline etc.) for decorated runs of glyphs.
fn decoration_shapes(
    galley: &Galley,
    decos: &[SectionDeco],
    offset: Vec2,
    pixels_per_point: f32,
    backgrounds: &mut Vec<Shape>,
    lines: &mut Vec<Shape>,
) {
    let crisp = |y: f32, width: f32| {
        let px = (width * pixels_per_point).round().max(1.0);
        let y = (y * pixels_per_point).floor();
        (if px as i32 % 2 == 1 { y + 0.5 } else { y }) / pixels_per_point
    };
    // Glyphs map 1:1 to characters (excluding row-ending newlines).
    let job = &galley.job;
    let mut char_sections = Vec::with_capacity(job.text.len());
    for (index, section) in job.sections.iter().enumerate() {
        let text = &job.text[section.byte_range.start.0..section.byte_range.end.0];
        char_sections.extend(std::iter::repeat_n(index, text.chars().count()));
    }
    let mut row_start = 0;
    for row in &galley.rows {
        let glyphs = &row.glyphs;
        let section_of = |g: usize| {
            char_sections
                .get(row_start + g)
                .copied()
                .unwrap_or(usize::MAX)
        };
        let mut i = 0;
        while i < glyphs.len() {
            let section = section_of(i);
            let mut j = i + 1;
            while j < glyphs.len() && section_of(j) == section {
                j += 1;
            }
            let run = i..j;
            i = j;
            let Some(deco) = decos.get(section).filter(|d| !d.is_none()) else {
                continue;
            };
            // Trailing spaces at the end of a row are not decorated.
            let mut end = run.end;
            if end == glyphs.len() {
                while end > run.start && glyphs[end - 1].chr.is_whitespace() {
                    end -= 1;
                }
            }
            if end == run.start {
                continue;
            }
            let first = &glyphs[run.start];
            let last = &glyphs[end - 1];
            let x0 = offset.x + row.pos.x + first.pos.x;
            let x1 = offset.x + row.pos.x + last.pos.x + last.advance_width;
            let baseline = offset.y + row.pos.y + first.pos.y;
            let size = galley.job.sections[section].format.font_id.size;
            if deco.background != Color32::TRANSPARENT {
                let top = baseline - first.font_ascent;
                let rect = Rect::from_min_max(
                    pos2(x0 - 1.0, top),
                    pos2(x1 + 1.0, top + first.font_height),
                );
                backgrounds.push(Shape::rect_filled(rect, 0.0, deco.background));
            }
            let mut hline = |stroke: Stroke, y: f32| {
                lines.push(Shape::hline(x0..=x1, crisp(y, stroke.width), stroke));
            };
            if let Some(stroke) = deco.underline {
                hline(stroke, baseline + (size * 0.1).max(1.0));
            }
            if let Some(stroke) = deco.overline {
                hline(stroke, baseline - first.font_ascent);
            }
            if let Some(stroke) = deco.line_through {
                hline(stroke, baseline - size * 0.28);
            }
        }
        row_start += row.char_count_including_newline().0;
    }
}

/// Width of the widest unbreakable run of glyphs in a no-wrap galley.
fn longest_word(galley: &Galley) -> f32 {
    let mut longest: f32 = 0.0;
    for row in &galley.rows {
        let mut word_start: Option<f32> = None;
        for glyph in &row.glyphs {
            let breakable = glyph.chr.is_whitespace() && glyph.chr != '\u{a0}';
            if breakable {
                word_start = None;
            } else {
                let start = *word_start.get_or_insert(glyph.pos.x);
                longest = longest.max(glyph.max_x() - start);
            }
        }
    }
    longest
}

/// Registers the bytes of a `data:` URI with egui's loaders and returns the URI to load.
fn register_data_uri(ctx: &Context, src: &str) -> Option<String> {
    let (meta, data) = src.strip_prefix("data:")?.split_once(',')?;
    let is_base64 = meta.ends_with(";base64");
    let mime = meta.split(';').next().unwrap_or("");
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        _ => "bin",
    };
    let bytes = if is_base64 {
        decode_base64(data)?
    } else {
        percent_decode(data)
    };
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut hasher);
    let uri = format!("bytes://egui_html_view/{:016x}.{ext}", hasher.finish());
    ctx.include_bytes(uri.clone(), bytes);
    Some(uri)
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        assert_eq!(decode_base64("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(decode_base64("YQ").unwrap(), b"a");
    }

    #[test]
    fn percent() {
        assert_eq!(percent_decode("%3Csvg%3E"), b"<svg>");
        assert_eq!(percent_decode("a%2"), b"a%2");
    }

    #[test]
    fn margins() {
        assert_eq!(collapse_margins(10.0, 20.0), 20.0);
        assert_eq!(collapse_margins(-5.0, 20.0), 15.0);
        assert_eq!(collapse_margins(-5.0, -20.0), -20.0);
    }
}

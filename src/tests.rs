//! Headless tests driving the widget through `egui::Context::run_ui`.

use std::collections::HashMap;

use egui::{
    epaint::{ClippedPrimitive, Primitive},
    pos2, vec2, Color32, ColorImage, Context, Event, FullOutput, Modifiers, OutputCommand,
    PointerButton, Pos2, RawInput, Rect, TextureId, Vec2, Visuals,
};

use crate::{HtmlView, ScrollMode};

pub(crate) struct Harness {
    pub ctx: Context,
    pub size: Vec2,
    pub pixels_per_point: f32,
    time: f64,
    textures: HashMap<TextureId, ColorImage>,
    pub last_output: Option<FullOutput>,
}

pub(crate) struct FrameResult {
    pub clicked_link: Option<String>,
    pub hovered_link: Option<String>,
    pub content_size: Vec2,
    pub rect: Rect,
    pub copied: Option<String>,
}

impl Harness {
    pub fn new(size: Vec2) -> Self {
        let ctx = Context::default();
        ctx.set_visuals(Visuals::light());
        Self {
            ctx,
            size,
            pixels_per_point: 1.0,
            time: 0.0,
            textures: HashMap::new(),
            last_output: None,
        }
    }

    pub fn frame(&mut self, view: &mut HtmlView, events: Vec<Event>) -> FrameResult {
        self.frame_with(events, |ui| view.show(ui))
    }

    pub fn frame_with(
        &mut self,
        events: Vec<Event>,
        mut f: impl FnMut(&mut egui::Ui) -> crate::HtmlViewResponse,
    ) -> FrameResult {
        self.time += 1.0 / 60.0;
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        input
            .viewports
            .entry(input.viewport_id)
            .or_default()
            .native_pixels_per_point = Some(self.pixels_per_point);
        let mut result = None;
        let mut output = self.ctx.run_ui(input, |ui| {
            let r = f(ui);
            result = Some(FrameResult {
                clicked_link: r.clicked_link,
                hovered_link: r.hovered_link,
                content_size: r.content_size,
                rect: r.response.rect,
                copied: None,
            });
        });
        let mut result = result.expect("ui ran");
        result.copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|c| match c {
                OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            });
        for (id, delta) in output
            .textures_delta
            .set
            .iter()
            .flat_map(|(id, deltas)| deltas.iter().map(move |d| (id, d)))
        {
            let egui::ImageData::Color(image) = &delta.image;
            match delta.pos {
                None => {
                    self.textures.insert(*id, (**image).clone());
                }
                Some([x0, y0]) => {
                    if let Some(tex) = self.textures.get_mut(id) {
                        for y in 0..image.size[1] {
                            for x in 0..image.size[0] {
                                tex.pixels[(y0 + y) * tex.size[0] + x0 + x] =
                                    image.pixels[y * image.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
        output.textures_delta.clear();
        self.last_output = Some(output);
        result
    }

    /// Runs a few frames so that layout and scroll state settle.
    pub fn settle(&mut self, view: &mut HtmlView) -> FrameResult {
        self.frame(view, vec![]);
        self.frame(view, vec![]);
        self.frame(view, vec![])
    }

    pub fn click(&mut self, view: &mut HtmlView, pos: Pos2) -> FrameResult {
        self.frame(view, vec![Event::PointerMoved(pos)]);
        self.frame(view, vec![button(pos, true, Modifiers::NONE)]);
        self.frame(view, vec![button(pos, false, Modifiers::NONE)])
    }

    pub fn drag(&mut self, view: &mut HtmlView, from: Pos2, to: Pos2) {
        self.frame(view, vec![Event::PointerMoved(from)]);
        self.frame(view, vec![button(from, true, Modifiers::NONE)]);
        for i in 1..=4 {
            let p = from + (to - from) * (i as f32 / 4.0);
            self.frame(view, vec![Event::PointerMoved(p)]);
        }
        self.frame(view, vec![button(to, false, Modifiers::NONE)]);
    }

    /// Software-rasterizes the last frame into an RGBA image.
    pub fn render(&self) -> ColorImage {
        let output = self.last_output.as_ref().expect("rendered a frame");
        let ppp = self.pixels_per_point;
        let primitives = self.ctx.tessellate(output.shapes.clone(), ppp);
        let w = (self.size.x * ppp) as usize;
        let h = (self.size.y * ppp) as usize;
        let bg = self.ctx.global_style().visuals.panel_fill;
        let mut img = ColorImage::new([w, h], vec![bg; w * h]);
        for ClippedPrimitive {
            clip_rect,
            primitive,
        } in primitives
        {
            let Primitive::Mesh(mesh) = primitive else {
                continue;
            };
            let Some(tex) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = Rect::from_min_max(
                (clip_rect.min.to_vec2() * ppp).to_pos2(),
                (clip_rect.max.to_vec2() * ppp).to_pos2(),
            );
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [0, 1, 2].map(|k| mesh.vertices[tri[k] as usize]);
                raster_triangle(&mut img, tex, v, ppp, clip);
            }
        }
        img
    }
}

pub(crate) fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn raster_triangle(
    img: &mut ColorImage,
    tex: &ColorImage,
    v: [egui::epaint::Vertex; 3],
    ppp: f32,
    clip: Rect,
) {
    let p: [Pos2; 3] = v.map(|v| (v.pos.to_vec2() * ppp).to_pos2());
    let area = (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y);
    if area.abs() < 1e-8 {
        return;
    }
    let min_x = p
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.x)
        .max(0.0)
        .floor() as i32;
    let max_x = p
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.x)
        .min(img.size[0] as f32)
        .ceil() as i32;
    let min_y = p
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .max(clip.min.y)
        .max(0.0)
        .floor() as i32;
    let max_y = p
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.max.y)
        .min(img.size[1] as f32)
        .ceil() as i32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let c = pos2(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((p[1].x - c.x) * (p[2].y - c.y) - (p[2].x - c.x) * (p[1].y - c.y)) / area;
            let w1 = ((p[2].x - c.x) * (p[0].y - c.y) - (p[0].x - c.x) * (p[2].y - c.y)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                continue;
            }
            let uv = v[0].uv.to_vec2() * w0 + v[1].uv.to_vec2() * w1 + v[2].uv.to_vec2() * w2;
            let tx = ((uv.x * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
            let ty = ((uv.y * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
            let t = tex.pixels[ty * tex.size[0] + tx];
            let mut src = [0.0f32; 4];
            for k in 0..4 {
                let vc = v[0].color[k] as f32 * w0
                    + v[1].color[k] as f32 * w1
                    + v[2].color[k] as f32 * w2;
                src[k] = vc / 255.0 * (t[k] as f32 / 255.0);
            }
            let dst = &mut img.pixels[y as usize * img.size[0] + x as usize];
            let a = src[3];
            let out: [u8; 4] = [0, 1, 2, 3].map(|k| {
                ((src[k] + dst[k] as f32 / 255.0 * (1.0 - a)) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8
            });
            *dst = Color32::from_rgba_premultiplied(out[0], out[1], out[2], out[3]);
        }
    }
}

/// Minimal PNG encoder (stored deflate blocks) for debug screenshots.
pub(crate) fn write_png(path: &std::path::Path, img: &ColorImage) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let [w, h] = img.size;
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let c = img.pixels[y * w + x];
            raw.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
    }
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(u8::from(last));
        z.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        png.extend_from_slice(&c);
        png.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, png)
}

// ----------------------------------------------------------------------------

fn texts(view: &HtmlView) -> &[crate::layout::TextBlock] {
    &view.layout.as_ref().expect("laid out").layout.texts
}

fn all_text(view: &mut HtmlView) -> String {
    view.select_all();
    view.selected_text().unwrap_or_default()
}

#[test]
fn renders_text_with_block_separators() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<html><head><title> My  page </title><style>.x{display:none}</style>\
         <script>var a = 1;</script></head>\
         <body><h1>Title</h1><p>Hello <b>bold</b>   world\n and   more</p>\
         <div class=x>hidden</div><ul><li>One</li><li>Two</li></ul></body></html>",
    );
    h.settle(&mut view);
    assert_eq!(view.title(), Some("My page"));
    assert_eq!(
        all_text(&mut view),
        "Title\nHello bold world and more\nOne\nTwo"
    );
}

#[test]
fn preformatted_text_keeps_whitespace() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new("<pre>  a  b\n\tc</pre><p>x<br>y<br></p>");
    h.settle(&mut view);
    assert_eq!(all_text(&mut view), "  a  b\n        c\nx\ny");
}

#[test]
fn css_cascade_specificity_and_inline_styles() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<style>:root{--accent:#00ff00} p{color:red} .a{color:blue} #b{color:var(--accent)}\
         p.imp{color:black !important}</style>\
         <p>red</p><p class=a>blue</p><p class=a id=b>green</p>\
         <p class=imp style='color:white'>black</p><p style='color: rgb(1,2,3)'>inline</p>",
    );
    h.settle(&mut view);
    let colors: Vec<Color32> = texts(&view)
        .iter()
        .map(|t| t.galley.job.sections[0].format.color)
        .collect();
    assert_eq!(
        colors,
        vec![
            Color32::RED,
            Color32::BLUE,
            Color32::from_rgb(0, 255, 0),
            Color32::BLACK,
            Color32::from_rgb(1, 2, 3)
        ]
    );
}

#[test]
fn user_stylesheet_overrides_document() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view =
        HtmlView::new("<style>p{color:red}</style><p>x</p>").with_css("p { color: #123456 }");
    h.settle(&mut view);
    assert_eq!(
        texts(&view)[0].galley.job.sections[0].format.color,
        Color32::from_rgb(0x12, 0x34, 0x56)
    );
}

#[test]
fn text_formats() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<p><b>b</b><i>i</i><code>c</code><u>u</u><s>s</s>\
         <span style='font-size:20px;letter-spacing:2px;background:yellow'>z</span></p>",
    );
    h.settle(&mut view);
    let block = &texts(&view)[0];
    let s = &block.galley.job.sections;
    assert!(block.bold_overlay.is_some(), "synthetic bold overlay");
    assert!(s[1].format.italics);
    assert_eq!(s[2].format.font_id.family, egui::FontFamily::Monospace);
    assert_eq!(s[5].format.font_id.size, 20.0);
    assert_eq!(s[5].format.extra_letter_spacing, 2.0);
    let shapes: Vec<&egui::Shape> = view
        .layout
        .as_ref()
        .unwrap()
        .layout
        .items
        .iter()
        .filter_map(|i| match i {
            crate::layout::Item::Shape(s) => Some(s),
            _ => None,
        })
        .collect();
    // Underline + line-through, and the yellow inline background (plus the `code` background).
    let lines = shapes
        .iter()
        .filter(|s| matches!(s, egui::Shape::LineSegment { .. }))
        .count();
    assert_eq!(lines, 2);
    assert!(shapes
        .iter()
        .any(|s| matches!(s, egui::Shape::Rect(r) if r.fill == Color32::YELLOW)));
}

#[test]
fn margins_collapse() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<body style='margin:0'><div><p style='margin:20px 0'>a</p></div>\
         <p style='margin:30px 0'>b</p><p></p><p style='margin:10px 0'>c</p></body>",
    );
    h.settle(&mut view);
    let t = texts(&view);
    let a = t[0].rect();
    let b = t[1].rect();
    let c = t[2].rect();
    assert_eq!(a.top(), 20.0, "parent/first-child margins collapse");
    assert_eq!(b.top() - a.bottom(), 30.0, "sibling margins collapse");
    assert_eq!(
        c.top() - b.bottom(),
        30.0,
        "empty block margins collapse through"
    );
}

#[test]
fn box_model_width_padding_and_auto_margins() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<body style='margin:0'><div style='width:200px;margin:0 auto;padding:10px;border:5px solid red'>x</div>\
         <div style='max-width:100px;box-sizing:border-box;padding:0 10px'>y</div></body>",
    );
    h.settle(&mut view);
    let t = texts(&view);
    // (600 - 200 - 2*10 - 2*5) / 2 = 185, plus border and padding.
    assert_eq!(t[0].origin.x, 185.0 + 15.0);
    assert_eq!(t[1].origin.x, 10.0);
}

#[test]
fn tables_lay_out_cells_side_by_side() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<table border=1><tr><th>Name</th><th>Value</th></tr>\
         <tr><td>alpha</td><td>1</td></tr><tr><td colspan=2>wide cell</td></tr></table><p>after</p>",
    );
    h.settle(&mut view);
    let t = texts(&view);
    assert_eq!(t.len(), 6);
    assert_eq!(t[0].rect().center().y, t[1].rect().center().y);
    assert!(t[1].rect().left() > t[0].rect().right());
    assert!(t[2].rect().top() > t[0].rect().bottom());
    assert!(t[5].rect().top() > t[4].rect().bottom());
    assert_eq!(
        all_text(&mut view),
        "Name\tValue\nalpha\t1\nwide cell\nafter"
    );
}

#[test]
fn list_markers_are_painted_but_not_copied() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view =
        HtmlView::new("<ol start=3><li>three</li><li>four</li></ol><ul><li>dot</li></ul>");
    h.settle(&mut view);
    let layout = &view.layout.as_ref().unwrap().layout;
    let markers: Vec<String> = layout
        .items
        .iter()
        .filter_map(|i| match i {
            crate::layout::Item::Shape(egui::Shape::Text(t)) => Some(t.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(markers, vec!["3.", "4."]);
    assert!(layout
        .items
        .iter()
        .any(|i| matches!(i, crate::layout::Item::Shape(egui::Shape::Circle(_)))));
    assert_eq!(all_text(&mut view), "three\nfour\ndot");
}

#[test]
fn drag_selects_and_ctrl_c_copies() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new("<p>first paragraph</p><p>second paragraph</p>");
    h.settle(&mut view);
    let (a, b) = {
        let t = texts(&view);
        (t[0].rect(), t[1].rect())
    };
    // From the start of "first" to the middle of "second".
    let from = pos2(a.left() + 1.0, a.center().y);
    let to = pos2(b.left() + b.width() * 0.25, b.center().y);
    h.drag(&mut view, from, to);
    let selected = view.selected_text().expect("selection");
    assert!(selected.starts_with("first paragraph\nsec"), "{selected:?}");

    let r = h.frame(&mut view, vec![Event::Copy]);
    assert_eq!(r.copied.as_deref(), Some(selected.as_str()));

    // A click elsewhere clears the selection.
    h.click(&mut view, pos2(300.0, 350.0));
    assert_eq!(view.selected_text(), None);
}

#[test]
fn double_click_selects_word_and_ctrl_a_selects_all() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new("<p>alpha beta gamma</p><p>delta</p>");
    h.settle(&mut view);
    let galley_pos = {
        let t = &texts(&view)[0];
        // Middle of "beta".
        let x = t
            .galley
            .pos_from_cursor(egui::text::CCursor::new(egui::text::CharIndex(8)))
            .min
            .x;
        t.origin + vec2(x, t.galley.size().y / 2.0)
    };
    h.frame(&mut view, vec![Event::PointerMoved(galley_pos)]);
    h.frame(&mut view, vec![button(galley_pos, true, Modifiers::NONE)]);
    h.frame(&mut view, vec![button(galley_pos, false, Modifiers::NONE)]);
    h.frame(&mut view, vec![button(galley_pos, true, Modifiers::NONE)]);
    h.frame(&mut view, vec![button(galley_pos, false, Modifiers::NONE)]);
    assert_eq!(view.selected_text().as_deref(), Some("beta"));

    h.frame(
        &mut view,
        vec![Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }],
    );
    assert_eq!(
        view.selected_text().as_deref(),
        Some("alpha beta gamma\ndelta")
    );
}

#[test]
fn links_are_reported_and_fragments_scroll() {
    let mut h = Harness::new(vec2(400.0, 300.0));
    let filler = "<p>filler</p>".repeat(40);
    let mut view = HtmlView::new(&format!(
        "<p><a href='#target'>jump</a> <a href='https://example.com/'>external</a></p>{filler}\
         <h2 id=target>Target</h2>{filler}"
    ));
    h.settle(&mut view);
    let (jump, external) = {
        let t = &texts(&view)[0];
        let y = t.galley.size().y / 2.0;
        let x_of = |i: usize| {
            t.galley
                .pos_from_cursor(egui::text::CCursor::new(egui::text::CharIndex(i)))
                .min
                .x
        };
        (t.origin + vec2(x_of(1), y), t.origin + vec2(x_of(7), y))
    };
    let r = h.frame(&mut view, vec![Event::PointerMoved(external)]);
    assert_eq!(r.hovered_link.as_deref(), Some("https://example.com/"));
    let r = h.click(&mut view, external);
    assert_eq!(r.clicked_link.as_deref(), Some("https://example.com/"));

    let r = h.click(&mut view, jump);
    assert_eq!(r.clicked_link.as_deref(), Some("#target"));
    h.settle(&mut view);
    let target_y = view.layout.as_ref().unwrap().layout.anchors["target"];
    assert!(target_y > 300.0);
    assert_eq!(view.scroll_offset().y, target_y);
}

#[test]
fn zoom_scales_layout_and_ctrl_wheel_zooms() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new("<p style='font-size:20px'>zoom me</p>");
    let r1 = h.settle(&mut view);
    let size1 = texts(&view)[0].galley.job.sections[0].format.font_id.size;
    view.set_zoom(2.0);
    let r2 = h.settle(&mut view);
    let size2 = texts(&view)[0].galley.job.sections[0].format.font_id.size;
    assert_eq!((size1, size2), (20.0, 40.0));
    assert!(r2.content_size.y >= r1.content_size.y);

    // Ctrl + mouse wheel (egui turns it into a zoom delta).
    view.set_zoom(1.0);
    h.settle(&mut view);
    h.frame(&mut view, vec![Event::PointerMoved(pos2(100.0, 100.0))]);
    h.frame(&mut view, vec![Event::Zoom(1.5)]);
    assert!((view.zoom() - 1.5).abs() < 1e-4, "zoom = {}", view.zoom());
    view.set_zoom(100.0);
    assert_eq!(view.zoom(), 5.0, "clamped to the zoom range");
}

#[test]
fn scroll_modes() {
    let mut h = Harness::new(vec2(300.0, 200.0));
    let long_line = "x".repeat(200);
    let html = format!("<pre>{long_line}</pre>{}", "<p>line</p>".repeat(30));

    let mut view = HtmlView::new(&html).scroll_mode(ScrollMode::Both);
    let r = h.settle(&mut view);
    assert!(r.content_size.x > 300.0, "wide pre overflows horizontally");
    assert!(r.content_size.y > 200.0);
    assert_eq!(r.rect.height(), r.content_size.y.max(200.0));

    let mut view = HtmlView::new(&html).scroll_mode(ScrollMode::None);
    let r = h.settle(&mut view);
    assert_eq!(
        r.rect.height(),
        r.content_size.y,
        "no scroll area: takes the full content height"
    );

    // Keyboard scrolling when focused.
    let mut view = HtmlView::new(&html);
    h.settle(&mut view);
    h.click(&mut view, pos2(150.0, 100.0));
    h.frame(
        &mut view,
        vec![Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    h.settle(&mut view);
    assert!(
        view.scroll_offset().y > 100.0,
        "offset = {:?}",
        view.scroll_offset()
    );
}

#[test]
fn dark_mode_uses_visuals_and_media_queries() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    h.ctx.set_visuals(Visuals::dark());
    let mut view = HtmlView::new(
        "<style>@media (prefers-color-scheme: dark) { .m { color: rgb(9,9,9) } }\
         @media (max-width: 100px) { .w { color: red } }</style>\
         <p>plain</p><p class=m>media</p><p class=w>wide</p>",
    );
    h.settle(&mut view);
    let t = texts(&view);
    let color = |i: usize| t[i].galley.job.sections[0].format.color;
    assert_eq!(color(0), Visuals::dark().text_color());
    assert_eq!(color(1), Color32::from_rgb(9, 9, 9));
    assert_eq!(color(2), Visuals::dark().text_color());
}

#[test]
fn images_without_loaders_show_alt_text() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<p>before <img src='data:image/png;base64,iVBORw0KGgo=' alt='[picture]'> after</p>",
    );
    h.settle(&mut view);
    assert_eq!(all_text(&mut view), "before [picture] after");
}

#[test]
fn nowrap_and_wrapping() {
    let mut h = Harness::new(vec2(200.0, 400.0));
    let words = "word ".repeat(20);
    let mut view = HtmlView::new(&format!(
        "<p>{words}</p><p style='white-space:nowrap'>{words}</p>"
    ));
    h.settle(&mut view);
    let t = texts(&view);
    assert!(t[0].galley.rows.len() > 3);
    assert_eq!(t[1].galley.rows.len(), 1);
}

#[test]
fn text_align_center_and_right() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new(
        "<body style='margin:0'><p style='text-align:center'>c</p><p align=right>r</p></body>",
    );
    h.settle(&mut view);
    let t = texts(&view);
    assert!((t[0].rect().center().x - 300.0).abs() < 2.0);
    assert!((t[1].rect().right() - 600.0).abs() < 1.0);
}

/// Writes screenshots of a demo page to `$EGUI_HTML_VIEW_SCREENSHOT_DIR` (if set).
#[test]
fn screenshots() {
    let Ok(dir) = std::env::var("EGUI_HTML_VIEW_SCREENSHOT_DIR") else {
        return;
    };
    let html = include_str!("../examples/demo.html");
    for (name, dark, zoom, size, ppp) in [
        ("light", false, 1.0, vec2(820.0, 1300.0), 1.0),
        ("dark", true, 1.0, vec2(820.0, 1300.0), 1.0),
        ("zoomed", false, 1.5, vec2(820.0, 1300.0), 1.0),
        ("detail", false, 1.0, vec2(560.0, 330.0), 2.0),
    ] {
        let mut h = Harness::new(size);
        h.pixels_per_point = ppp;
        h.ctx.set_visuals(if dark {
            Visuals::dark()
        } else {
            Visuals::light()
        });
        let mut view = HtmlView::new(html).with_zoom(zoom);
        h.settle(&mut view);
        // Select some text to show the highlight.
        let (from, to) = {
            let t = texts(&view);
            let a = t[2].rect();
            let b = t[3].rect();
            (
                pos2(a.left() + a.width() * 0.3, a.top() + 4.0),
                pos2(b.left() + 80.0, b.top() + 4.0),
            )
        };
        h.drag(&mut view, from, to);
        h.settle(&mut view);
        let img = h.render();
        write_png(
            &std::path::Path::new(&dir).join(format!("{name}.png")),
            &img,
        )
        .unwrap();
    }
}

struct TestImageLoader;

impl egui::load::ImageLoader for TestImageLoader {
    fn id(&self) -> &str {
        "egui_html_view::tests::TestImageLoader"
    }

    /// `test://WxH` → a red image of that size.
    fn load(&self, _ctx: &Context, uri: &str, _: egui::SizeHint) -> egui::load::ImageLoadResult {
        let Some((w, h)) = uri
            .strip_prefix("test://")
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse::<usize>().ok()?, h.parse::<usize>().ok()?)))
        else {
            return Err(egui::load::LoadError::NotSupported);
        };
        Ok(egui::load::ImagePoll::Ready {
            image: std::sync::Arc::new(ColorImage::new([w, h], vec![Color32::RED; w * h])),
        })
    }

    fn forget(&self, _: &str) {}

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        0
    }
}

#[test]
fn images_are_sized_scaled_and_clickable() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    h.ctx.add_image_loader(std::sync::Arc::new(TestImageLoader));
    let mut view = HtmlView::new(
        "<body style='margin:0'><p><img src='test://100x50'> <img src='test://40x40' width=20></p>\
         <p><a href='/big'><img src='test://1000x100'></a></p></body>",
    );
    h.settle(&mut view);
    let rects: Vec<Rect> = view
        .layout
        .as_ref()
        .unwrap()
        .layout
        .items
        .iter()
        .filter_map(|i| match i {
            crate::layout::Item::Image { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), 3);
    assert_eq!(rects[0].size(), vec2(100.0, 50.0));
    assert_eq!(rects[1].size(), vec2(20.0, 20.0));
    assert_eq!(rects[0].top(), 16.0);
    assert!(
        rects[1].left() > rects[0].right(),
        "inline images flow side by side"
    );
    assert_eq!(
        rects[2].size(),
        vec2(600.0, 60.0),
        "scaled down to the container width"
    );

    let r = h.click(&mut view, rects[2].center());
    assert_eq!(r.clicked_link.as_deref(), Some("/big"));
}

#[test]
fn layout_is_cached_between_frames() {
    let mut h = Harness::new(vec2(600.0, 400.0));
    let mut view = HtmlView::new("<p>cached paragraph</p>");
    h.settle(&mut view);
    let first = texts(&view)[0].galley.clone();
    h.frame(&mut view, vec![Event::PointerMoved(pos2(10.0, 10.0))]);
    assert!(std::sync::Arc::ptr_eq(&first, &texts(&view)[0].galley));

    h.size = vec2(300.0, 400.0);
    h.settle(&mut view);
    assert!(
        !std::sync::Arc::ptr_eq(&first, &texts(&view)[0].galley),
        "relaid out on resize"
    );
}

#[test]
fn without_scroll_area_anchors_scroll_the_parent_and_repaints_stop() {
    let mut h = Harness::new(vec2(400.0, 300.0));
    let filler = "<p>filler</p>".repeat(40);
    let mut view = HtmlView::new(&format!(
        "<p><a href='#t'>jump</a></p>{filler}<h2 id=t>Target</h2>{filler}"
    ))
    .scroll_mode(ScrollMode::None);

    // `set_html` leaves a pending "scroll to top"; it must not keep the view repainting.
    let mut alone = HtmlView::new(&filler).scroll_mode(ScrollMode::None);
    for _ in 0..3 {
        h.frame(&mut alone, vec![]);
    }
    let repaint_delay = h
        .last_output
        .as_ref()
        .unwrap()
        .viewport_output
        .values()
        .next()
        .unwrap()
        .repaint_delay;
    assert!(
        repaint_delay > std::time::Duration::from_secs(1),
        "{repaint_delay:?}"
    );

    let frame = |h: &mut Harness, view: &mut HtmlView, events: Vec<Event>| {
        h.frame_with(events, |ui| {
            egui::ScrollArea::vertical()
                .show(ui, |ui| view.show(ui))
                .inner
        })
    };
    for _ in 0..3 {
        frame(&mut h, &mut view, vec![]);
    }
    let link = {
        let t = &texts(&view)[0];
        t.origin + vec2(5.0, t.galley.size().y / 2.0)
    };
    frame(&mut h, &mut view, vec![Event::PointerMoved(link)]);
    frame(&mut h, &mut view, vec![button(link, true, Modifiers::NONE)]);
    let r = frame(
        &mut h,
        &mut view,
        vec![button(link, false, Modifiers::NONE)],
    );
    assert_eq!(r.clicked_link.as_deref(), Some("#t"));
    let mut r = frame(&mut h, &mut view, vec![]);
    for _ in 0..30 {
        r = frame(&mut h, &mut view, vec![]);
    }
    let target_y = view.layout.as_ref().unwrap().layout.anchors["t"];
    assert!(
        (r.rect.top() + target_y).abs() < 8.0,
        "rect top {} target {target_y}",
        r.rect.top()
    );
}

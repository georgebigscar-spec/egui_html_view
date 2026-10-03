# egui_html_view

[![CI](https://github.com/georgebigscar-spec/egui_html_view/actions/workflows/ci.yml/badge.svg)](https://github.com/georgebigscar-spec/egui_html_view/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A widget for [egui](https://github.com/emilk/egui) 0.36 that renders HTML + CSS:
optional scroll bars, zoom, styles, and text selection and copying.

![Demo app (eframe, dark theme): some text is selected and a tooltip shows a link's URL](https://raw.githubusercontent.com/georgebigscar-spec/egui_html_view/main/docs/screenshot.png)

## Why this is not a "real" webview

A native webview (WebView2, WKWebView, WebKitGTK, for example through `wry`) is a separate
OS window on top of the application window. egui cannot clip it to a scroll area, scale it
together with the rest of the UI, or draw its own widgets (menus, tooltips) over it. So
`HtmlView` parses HTML itself (`html5ever` via `scraper`), applies CSS and draws the result
with egui primitives. It behaves like any other widget: it works inside panels and
`ScrollArea` and follows the egui theme.

The price of this approach is that it is not a browser: JavaScript, forms, floats,
flex/grid and positioning are not supported (see [Limitations](#limitations)). If you need
a full browser, use `wry`.

## Features

- **HTML**: headings, paragraphs, `div`/sections, lists (`ul`/`ol`, nested, `start`, `type`,
  `reversed`, `li value`), tables (`colspan`, `border`, `cellpadding`, `cellspacing`,
  `thead`/`tbody`, `caption`), `pre`, `blockquote`, `hr`, `br`, `img`, links, `sub`/`sup`,
  `mark`, `code`/`kbd`, `b`/`i`/`u`/`s`, `small`/`big`, task-list checkboxes (`☐`/`☑`),
  legacy attributes (`bgcolor`, `align`, `width`, `<font>` and others).
- **CSS**: `<style>`, the `style` attribute, your own style sheets via `with_css`/`add_css`;
  the cascade with specificity, `!important`, inheritance, `inherit`/`initial`/`unset`,
  variables `var(--x, fallback)`, `@media` (window width and `prefers-color-scheme`).
  Selectors: everything `scraper` supports, including descendants, `>`, `+`, `~`,
  attributes, `:not()`, `:nth-child()` (`:hover` and pseudo-elements are skipped).
- **Properties**: `color`, `background(-color)`, `font(-size|-weight|-style|-family)`,
  `line-height`, `letter-spacing`, `text-align` (including `justify`), `text-indent`,
  `text-transform`, `text-decoration` (underline/overline/line-through, color), `white-space`,
  `tab-size`, `vertical-align`, `margin` (with collapsing and `auto` centering),
  `padding`, `border` (solid/dashed/dotted, per side, `border-radius`),
  `width`/`min-width`/`max-width`, `height`/`min-height`, `box-sizing`, `display`,
  `visibility`, `list-style`, `border-collapse`/`border-spacing`. Units: `px`, `em`,
  `rem`, `%`, `pt`, `vw`, `vh` and more; colors: names, `#rgb[a]`, `#rrggbb[aa]`,
  `rgb[a]()`, `hsl[a]()`.
- **Scroll bars** (optional): `ScrollMode::{Both, Vertical, None}` and
  `ScrollBarVisibility::{VisibleWhenNeeded, AlwaysVisible, AlwaysHidden}`. The mouse wheel and
  keyboard (arrows, PageUp/PageDown, Space, Home/End) scroll even when the bars are hidden.
- **Zoom**: Ctrl + wheel and pinch (the point under the cursor stays in place),
  Ctrl `+`/`-`/`0`, and `set_zoom`/`zoom_in`/`zoom_out`/`reset_zoom`. When zoomed, text is
  laid out again, as in a browser, rather than stretched like a picture.
- **Selection**: dragging across several blocks (with auto-scroll at the edge),
  Shift+click, double click (word), triple click (line/paragraph), Ctrl+A, Esc.
  Ctrl+C or the context menu ("Copy", "Select all", "Copy link address") copies the text
  (table cells are separated by tabs). Programmatically: `selected_text()`, `select_all()`.
- **Links**: `HtmlViewResponse::clicked_link` / `hovered_link`; `#id` links scroll to the
  anchor on their own; optionally `open_external_links(true)` opens `http(s)`/`mailto` links.
- **Images** through egui's image loaders (`egui_extras::install_image_loaders`), including
  `data:` URIs and `base_uri` for relative paths. If an image cannot be loaded, its `alt`
  text is shown.
- **Theme**: text without explicit styles takes its colors from `egui::Visuals` (the dark
  theme works out of the box); the `<body>`/`<html>` background fills the whole area, as in
  a browser.

## Usage

```toml
[dependencies]
egui = "0.36"
egui_html_view = { git = "https://github.com/georgebigscar-spec/egui_html_view" }
```

```rust
use egui_html_view::{HtmlView, ScrollMode, ScrollBarVisibility};

struct MyApp {
    view: HtmlView,
}

impl MyApp {
    fn new() -> Self {
        let view = HtmlView::new(r#"
            <h1>Hello</h1>
            <p>This is <b>HTML</b> with a <a href="#more">link</a>.</p>
            <h2 id="more">More</h2>
        "#)
        .with_css("h1 { color: #2f6fde } p { line-height: 1.5 }")
        .scroll_mode(ScrollMode::Both)                         // ScrollMode::None: no own ScrollArea
        .scroll_bar_visibility(ScrollBarVisibility::VisibleWhenNeeded)
        .zoom_range(0.5..=3.0);
        Self { view }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let response = self.view.show(ui);
        if let Some(href) = response.clicked_link {
            println!("link clicked: {href}");
        }
    }
}
```

`HtmlView` holds the DOM, style and layout caches, the zoom level and the selection, so keep
it in your application state and call `show` every frame. Layout is recomputed only when the
width, zoom, theme, document or styles change.

### Useful settings

| Method | What it does |
| --- | --- |
| `set_html(&str)` | replace the document (resets selection and scrolling) |
| `with_css` / `add_css` / `clear_css` | your own styles on top of the document's |
| `scroll_mode`, `scroll_bar_visibility`, `auto_shrink` | scrolling and scroll bars |
| `with_zoom`, `zoom_range`, `zoomable(false)` | zoom |
| `selectable(false)` | disable selection |
| `base_font_size(16.0)` | default font size in CSS px |
| `bold_mode(BoldMode::...)` | how bold text is drawn (see below) |
| `font_family("Roboto", FontFamily::Name(..))` | map a CSS font name to an egui font |
| `base_uri("file:///dir/")` | base for relative image `src` |
| `scroll_to_anchor("id")`, `scroll_to_offset` | scroll programmatically |
| `title()` | text of `<title>` |

### Bold text

egui's default fonts have no bold face, so by default bold text is synthesized: glyphs are
drawn a second time with a sub-pixel offset (`BoldMode::Synthetic`). If you load your own
fonts, there are better options:

- `BoldMode::Family { proportional, monospace }`: a separate family for bold text,
  registered through `FontDefinitions` (for example, `FontFamily::Name("bold".into())`);
- `BoldMode::VariableWeight`: the `wght` axis of a variable font.

### Zoom shortcuts

egui handles Ctrl `+`/`-`/`0` itself (it scales the whole UI). To let these keys reach the
widget, turn that off:

```rust
ctx.options_mut(|o| o.zoom_with_keyboard = false);
```

Ctrl + wheel and pinch work without this setting.

### Images

```toml
egui_extras = { version = "0.36", features = ["all_loaders"] }
image = { version = "0.25", features = ["png", "jpeg"] }
```

```rust
egui_extras::install_image_loaders(&ctx);
```

## Limitations

- No JavaScript, forms (except checkboxes/radio buttons rendered as symbols), `float`,
  `position`, flexbox/grid (such containers are laid out as normal blocks), `rowspan`,
  CSS transforms, animations, background images and gradients, `::before`/`::after`, `:hover`.
- Inline elements get backgrounds and decorations, but not padding or borders.
- An image inside text is placed on its own line (several images in a row line up and wrap).
- `calc()`, `min()`, `max()`, `clamp()` are supported roughly: the first argument that can
  be parsed is used.
- Subscript (`<sub>`) text is made smaller but barely drops below the baseline.

## Example and tests

```sh
cargo run --example demo                 # built-in demo page
cargo run --example demo -- page.html    # your own file
cargo test
```

The demo has a zoom bar, a scroll bar switch, themes and an HTML editor that re-renders as
you type.

The tests drive the widget in a headless `egui::Context`: mouse, keyboard, clipboard, layout,
the CSS cascade, images. You can save screenshots of the demo page drawn by a software
rasterizer:

```sh
EGUI_HTML_VIEW_SCREENSHOT_DIR=/tmp/shots cargo test screenshots
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.

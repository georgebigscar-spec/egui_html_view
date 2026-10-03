//! Computed styles and the cascade.

use std::{collections::HashMap, sync::Arc};

use egui::Color32;
use scraper::{ElementRef, Selector};

use crate::css::{self, Declaration, Length, MediaEnv, Rule, SelectorKey};

/// Colors taken from the egui [`egui::Visuals`] that the default style sheet refers to,
/// so that unstyled HTML matches the surrounding UI (including dark mode).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub text: Color32,
    pub link: Color32,
    pub code_bg: Color32,
    pub border: Color32,
}

impl Theme {
    pub fn from_visuals(visuals: &egui::Visuals) -> Self {
        Self {
            dark: visuals.dark_mode,
            text: visuals.text_color(),
            link: visuals.hyperlink_color,
            code_bg: visuals.code_bg_color,
            border: visuals.widgets.noninteractive.bg_stroke.color,
        }
    }

    fn color(&self, keyword: &str) -> Option<Color32> {
        Some(match keyword {
            "-egui-text" => self.text,
            "-egui-link" => self.link,
            "-egui-code-bg" => self.code_bg,
            "-egui-border" => self.border,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Inline,
    Block,
    ListItem,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
    TableCaption,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
}

impl WhiteSpace {
    pub fn collapses_spaces(self) -> bool {
        matches!(self, Self::Normal | Self::NoWrap | Self::PreLine)
    }

    pub fn wraps(self) -> bool {
        !matches!(self, Self::NoWrap | Self::Pre)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalAlign {
    Baseline,
    Sub,
    Super,
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    LowerGreek,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    /// Unitless multiplier, inherited as such.
    Factor(f32),
    /// Absolute line height in CSS px.
    Px(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TextDecoration {
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
    pub color: Option<Color32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Solid,
    Dashed,
    Dotted,
    Double,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub style: BorderStyle,
    /// `None` means `currentcolor`.
    pub color: Option<Color32>,
}

impl Border {
    pub const NONE: Self = Self {
        width: 3.0,
        style: BorderStyle::None,
        color: None,
    };

    /// Width in CSS px, taking `border-style: none` into account.
    pub fn used_width(&self) -> f32 {
        if self.style == BorderStyle::None {
            0.0
        } else {
            self.width.max(0.0)
        }
    }
}

/// Fully computed style of an element. Lengths are in CSS px (unzoomed).
#[derive(Clone, Debug)]
pub struct ComputedStyle {
    // --- inherited ---
    pub color: Color32,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    /// Lower-cased family names, in order of preference.
    pub font_family: Arc<[String]>,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub white_space: WhiteSpace,
    pub letter_spacing: f32,
    pub text_transform: TextTransform,
    pub text_indent: Length,
    pub list_style: ListStyle,
    pub visible: bool,
    pub border_collapse: bool,
    pub border_spacing: f32,
    pub tab_size: usize,
    /// CSS custom properties (`--name: value`).
    pub vars: Arc<HashMap<String, String>>,

    // --- not inherited ---
    /// Propagates to descendants (as in browsers).
    pub decoration: TextDecoration,
    pub display: Display,
    pub background: Color32,
    pub vertical_align: VerticalAlign,
    /// top, right, bottom, left
    pub margin: [Length; 4],
    pub padding: [Length; 4],
    pub border: [Border; 4],
    pub border_radius: f32,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    /// `Auto` means `none`.
    pub max_width: Length,
    pub min_height: Length,
    pub border_box: bool,
    pub opacity: f32,
}

impl ComputedStyle {
    pub fn initial(theme: &Theme, base_font_size: f32) -> Self {
        Self {
            color: theme.text,
            font_size: base_font_size,
            font_weight: 400,
            italic: false,
            font_family: Arc::from(vec!["sans-serif".to_owned()]),
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            letter_spacing: 0.0,
            text_transform: TextTransform::None,
            text_indent: Length::ZERO,
            list_style: ListStyle::Disc,
            visible: true,
            border_collapse: false,
            border_spacing: 0.0,
            tab_size: 8,
            vars: Arc::default(),
            decoration: TextDecoration::default(),
            display: Display::Inline,
            background: Color32::TRANSPARENT,
            vertical_align: VerticalAlign::Baseline,
            margin: [Length::ZERO; 4],
            padding: [Length::ZERO; 4],
            border: [Border::NONE; 4],
            border_radius: 0.0,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            border_box: false,
            opacity: 1.0,
        }
    }

    /// Style for a child: inherited properties are copied, the rest reset to initial values.
    pub fn inherit(parent: &Self, initial: &Self) -> Self {
        Self {
            color: parent.color,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            italic: parent.italic,
            font_family: parent.font_family.clone(),
            line_height: parent.line_height,
            text_align: parent.text_align,
            white_space: parent.white_space,
            letter_spacing: parent.letter_spacing,
            text_transform: parent.text_transform,
            text_indent: parent.text_indent,
            list_style: parent.list_style,
            visible: parent.visible,
            border_collapse: parent.border_collapse,
            border_spacing: parent.border_spacing,
            tab_size: parent.tab_size,
            vars: parent.vars.clone(),
            decoration: parent.decoration,
            ..initial.clone()
        }
    }

    pub fn is_bold(&self) -> bool {
        self.font_weight >= 600
    }

    pub fn is_monospace(&self) -> bool {
        self.font_family.iter().any(|f| {
            matches!(
                f.as_str(),
                "monospace"
                    | "ui-monospace"
                    | "courier"
                    | "courier new"
                    | "consolas"
                    | "menlo"
                    | "monaco"
                    | "lucida console"
                    | "sfmono-regular"
                    | "sf mono"
                    | "source code pro"
                    | "fira code"
                    | "fira mono"
                    | "jetbrains mono"
                    | "dejavu sans mono"
                    | "liberation mono"
                    | "hack"
                    | "ubuntu mono"
                    | "roboto mono"
                    | "cascadia code"
                    | "cascadia mono"
            )
        })
    }

    /// Resolved line height in CSS px, `None` for `normal`.
    pub fn line_height_px(&self) -> Option<f32> {
        match self.line_height {
            LineHeight::Normal => None,
            LineHeight::Factor(f) => Some(f * self.font_size),
            LineHeight::Px(px) => Some(px),
        }
    }

    pub fn border_color(&self, side: usize) -> Color32 {
        self.border[side].color.unwrap_or(self.color)
    }
}

// ----------------------------------------------------------------------------
// Default (user agent) style sheet

pub const UA_STYLESHEET: &str = r#"
html, address, blockquote, body, center, dd, details, dialog, dir, div, dl, dt, fieldset,
figcaption, figure, footer, form, h1, h2, h3, h4, h5, h6, header, hgroup, hr, legend, listing,
main, menu, nav, ol, p, plaintext, pre, search, section, summary, ul, xmp, article, aside,
optgroup, textarea { display: block }
head, script, style, title, meta, link, base, template, noscript, datalist, select, option, param,
source, track, area, map, iframe, object, embed, audio, video, canvas, svg, math, noframes, frame,
frameset, rp, input, dialog:not([open]) { display: none }
input[type=checkbox], input[type=radio] { display: inline }
[hidden] { display: none }
li { display: list-item }
table { display: table; border-spacing: 2px; border-collapse: separate; text-indent: 0 }
caption { display: table-caption; text-align: center }
thead, tbody, tfoot { display: table-row-group; vertical-align: middle }
tr { display: table-row; vertical-align: inherit }
td, th { display: table-cell; padding: 1px; vertical-align: inherit }
th { font-weight: bold; text-align: center }
colgroup, col { display: none }
body { margin: 8px }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold }
h4 { margin: 1.33em 0; font-weight: bold }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold }
p, dl { margin: 1em 0 }
blockquote, figure { margin: 1em 40px }
dd { margin-left: 40px }
dt { font-weight: bold }
ul, ol, menu, dir { margin: 1em 0; padding-left: 40px }
ul, menu, dir { list-style-type: disc }
ol { list-style-type: decimal }
ul ul, ol ul, menu ul, ul menu { list-style-type: circle }
ul ul ul, ul ol ul, ol ul ul, ol ol ul { list-style-type: square }
li ul, li ol, ul ul, ol ol, ul ol, ol ul { margin-top: 0; margin-bottom: 0 }
b, strong { font-weight: bolder }
i, em, cite, var, dfn, address { font-style: italic }
code, kbd, samp, tt, pre, listing, xmp, plaintext, textarea { font-family: monospace; font-size: 0.9em }
pre code, pre kbd, pre samp, pre tt { font-size: 1em }
pre, listing, xmp, plaintext { white-space: pre; margin: 1em 0 }
pre { background-color: -egui-code-bg; padding: 0.5em; border-radius: 3px }
:not(pre) > code, kbd { background-color: -egui-code-bg }
a[href] { color: -egui-link; text-decoration: underline }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
small { font-size: smaller }
big { font-size: larger }
sub { vertical-align: sub; font-size: smaller }
sup { vertical-align: super; font-size: smaller }
mark { background-color: yellow; color: black }
hr { border: none; border-top: 1px solid -egui-border; margin: 0.5em 0 }
center { text-align: center }
nobr { white-space: nowrap }
fieldset { margin: 0 2px; padding: 0.35em 0.75em 0.625em; border: 2px solid -egui-border }
legend { padding: 0 2px }
textarea { white-space: pre-wrap; border: 1px solid -egui-border; padding: 2px }
summary { font-weight: bold }
"#;

// ----------------------------------------------------------------------------
// Style engine

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    UserAgent = 0,
    Author = 1,
    Inline = 2,
}

struct CompiledSelector {
    selector: Selector,
    specificity: u32,
    rule: usize,
}

struct CompiledRule {
    origin: Origin,
    order: u32,
    declarations: Vec<Declaration>,
    /// Indices into [`StyleEngine::media_conditions`].
    media: Vec<usize>,
}

/// Compiled style sheets with rules bucketed by the key of their right-most selector.
pub struct StyleEngine {
    rules: Vec<CompiledRule>,
    selectors: Vec<CompiledSelector>,
    index: HashMap<SelectorKey, Vec<usize>>,
    media_conditions: Vec<String>,
}

impl StyleEngine {
    /// `author_sheets` are applied in order after the default style sheet.
    pub fn new(author_sheets: &[Vec<Rule>]) -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            selectors: Vec::new(),
            index: HashMap::new(),
            media_conditions: Vec::new(),
        };
        engine.add_rules(&css::parse_stylesheet(UA_STYLESHEET), Origin::UserAgent);
        for sheet in author_sheets {
            engine.add_rules(sheet, Origin::Author);
        }
        engine
    }

    fn add_rules(&mut self, rules: &[Rule], origin: Origin) {
        for rule in rules {
            let media = rule
                .media
                .iter()
                .map(|m| {
                    self.media_conditions
                        .iter()
                        .position(|c| c == m)
                        .unwrap_or_else(|| {
                            self.media_conditions.push(m.clone());
                            self.media_conditions.len() - 1
                        })
                })
                .collect();
            let rule_index = self.rules.len();
            self.rules.push(CompiledRule {
                origin,
                // Order 0 is reserved for presentational hints.
                order: rule_index as u32 + 1,
                declarations: rule.declarations.clone(),
                media,
            });
            for selector_text in &rule.selectors {
                // Unsupported selectors (e.g. `:hover`, `::before`) fail to parse and are skipped.
                let Ok(selector) = Selector::parse(selector_text) else {
                    continue;
                };
                let index = self.selectors.len();
                self.selectors.push(CompiledSelector {
                    selector,
                    specificity: css::specificity(selector_text),
                    rule: rule_index,
                });
                self.index
                    .entry(css::selector_key(selector_text))
                    .or_default()
                    .push(index);
            }
        }
    }

    /// Which media conditions match `env`. The cascade must be redone when this changes.
    pub fn media_state(&self, env: &MediaEnv) -> Vec<bool> {
        self.media_conditions
            .iter()
            .map(|c| css::media_matches(c, env))
            .collect()
    }

    /// Computes the style of `element`.
    pub fn compute(
        &self,
        element: ElementRef<'_>,
        parent: &ComputedStyle,
        ctx: &CascadeContext<'_>,
    ) -> ComputedStyle {
        let el = element.value();
        let tag = el.name();

        let mut candidates: Vec<usize> = Vec::new();
        let mut add_bucket = |key: SelectorKey| {
            if let Some(list) = self.index.get(&key) {
                candidates.extend_from_slice(list);
            }
        };
        add_bucket(SelectorKey::Universal);
        add_bucket(SelectorKey::Tag(tag.to_owned()));
        if let Some(id) = el.id() {
            add_bucket(SelectorKey::Id(id.to_owned()));
        }
        for class in el.classes() {
            add_bucket(SelectorKey::Class(class.to_owned()));
        }

        // (important, origin, specificity, order, declaration)
        let mut matched: Vec<(bool, Origin, u32, u32, &Declaration)> = Vec::new();
        for &sel_index in &candidates {
            let sel = &self.selectors[sel_index];
            let rule = &self.rules[sel.rule];
            if !rule.media.iter().all(|&m| ctx.media[m]) {
                continue;
            }
            if sel.selector.matches(&element) {
                for decl in &rule.declarations {
                    matched.push((
                        decl.important,
                        rule.origin,
                        sel.specificity,
                        rule.order,
                        decl,
                    ));
                }
            }
        }

        let hints = presentational_hints(element);
        for decl in &hints {
            matched.push((false, Origin::Author, 0, 0, decl));
        }

        let inline = el
            .attr("style")
            .map(css::parse_declarations)
            .unwrap_or_default();
        for decl in &inline {
            matched.push((decl.important, Origin::Inline, u32::MAX, u32::MAX, decl));
        }

        // Stable sort keeps declaration order within a rule.
        matched.sort_by_key(|&(important, origin, specificity, order, _)| {
            (important, origin, specificity, order)
        });

        let mut style = ComputedStyle::inherit(parent, ctx.initial);
        let mut applier = Applier {
            parent,
            ctx,
            decoration_set: false,
        };

        // 1. Custom properties.
        let custom: Vec<_> = matched
            .iter()
            .filter(|m| m.4.name.starts_with("--"))
            .collect();
        if !custom.is_empty() {
            let vars = Arc::make_mut(&mut style.vars);
            for m in custom {
                vars.insert(m.4.name.clone(), m.4.value.clone());
            }
        }

        // 2. Font size, which `em` units in the other properties depend on.
        for m in &matched {
            let decl = m.4;
            if decl.name == "font-size" || decl.name == "font" {
                let value = substitute_vars(&decl.value, &style.vars);
                applier.apply_font_size(&mut style, &decl.name, &value);
            }
        }

        // 3. Everything else.
        for m in &matched {
            let decl = m.4;
            if decl.name.starts_with("--") || decl.name == "font-size" {
                continue;
            }
            let value = substitute_vars(&decl.value, &style.vars);
            applier.apply(&mut style, &decl.name, value.trim());
        }

        if applier.decoration_set && style.decoration.color.is_none() {
            style.decoration.color = Some(style.color);
        }
        // Tables with collapsed borders have no spacing between cells.
        if style.border_collapse && style.display == Display::Table {
            style.border_spacing = 0.0;
        }
        style
    }
}

pub struct CascadeContext<'a> {
    pub theme: &'a Theme,
    pub initial: &'a ComputedStyle,
    pub media: &'a [bool],
    pub root_font_size: f32,
    pub base_font_size: f32,
}

fn substitute_vars(value: &str, vars: &HashMap<String, String>) -> String {
    if !value.contains("var(") {
        return value.to_owned();
    }
    let mut out = value.to_owned();
    for _ in 0..8 {
        let Some(start) = out.find("var(") else { break };
        let args_start = start + 4;
        let mut depth = 1;
        let mut end = None;
        for (i, c) in out[args_start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(args_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let args = &out[args_start..end];
        let (name, fallback) = match args.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (args.trim(), None),
        };
        let replacement = vars
            .get(name)
            .map(String::as_str)
            .or(fallback)
            .unwrap_or("")
            .to_owned();
        out.replace_range(start..=end, &replacement);
    }
    out
}

/// Maps legacy HTML attributes (`bgcolor`, `align`, `width`, ...) to declarations.
fn presentational_hints(element: ElementRef<'_>) -> Vec<Declaration> {
    let el = element.value();
    let tag = el.name();
    let mut out = Vec::new();
    let mut push = |name: &str, value: String| {
        out.push(Declaration {
            name: name.to_owned(),
            value,
            important: false,
        });
    };
    let dimension = |v: &str| -> String {
        let v = v.trim();
        if v.ends_with('%') {
            v.to_owned()
        } else {
            format!("{}px", v.trim_end_matches("px"))
        }
    };

    if let Some(c) = el.attr("bgcolor") {
        push("background-color", c.to_owned());
    }
    if let Some(a) = el.attr("align") {
        let a = a.to_ascii_lowercase();
        match tag {
            "table" if a == "center" => {
                push("margin-left", "auto".into());
                push("margin-right", "auto".into());
            }
            "img" | "table" => {}
            _ => push("text-align", a),
        }
    }
    if let Some(v) = el.attr("valign") {
        push("vertical-align", v.to_ascii_lowercase());
    }
    if matches!(
        tag,
        "img" | "table" | "td" | "th" | "col" | "hr" | "canvas" | "video"
    ) {
        if let Some(w) = el.attr("width") {
            push("width", dimension(w));
        }
        if let Some(h) = el.attr("height") {
            push("height", dimension(h));
        }
    }
    if tag == "td" || tag == "th" {
        if el.attr("nowrap").is_some() {
            push("white-space", "nowrap".into());
        }
        // Table-level attributes that affect cells.
        if let Some(table) = element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|a| a.value().name() == "table")
        {
            let t = table.value();
            if let Some(b) = t.attr("border") {
                if b.trim() != "0" {
                    push("border", "1px solid gray".into());
                }
            }
            if let Some(p) = t.attr("cellpadding") {
                push("padding", dimension(p));
            }
        }
    }
    if tag == "table" {
        if let Some(b) = t_attr_border(el.attr("border")) {
            push("border", format!("{b}px outset gray"));
        }
        if let Some(s) = el.attr("cellspacing") {
            push("border-spacing", dimension(s));
        }
    }
    if tag == "font" {
        if let Some(c) = el.attr("color") {
            push("color", c.to_owned());
        }
        if let Some(face) = el.attr("face") {
            push("font-family", face.to_owned());
        }
        if let Some(size) = el.attr("size") {
            let size = size.trim();
            let n = if let Some(rel) = size.strip_prefix('+') {
                3 + rel.parse::<i32>().unwrap_or(0)
            } else if let Some(rel) = size.strip_prefix('-') {
                3 - rel.parse::<i32>().unwrap_or(0)
            } else {
                size.parse::<i32>().unwrap_or(3)
            };
            let keyword = match n.clamp(1, 7) {
                1 => "x-small",
                2 => "small",
                3 => "medium",
                4 => "large",
                5 => "x-large",
                6 => "xx-large",
                _ => "xxx-large",
            };
            push("font-size", keyword.into());
        }
    }
    if tag == "body" {
        if let Some(c) = el.attr("text") {
            push("color", c.to_owned());
        }
    }
    if tag == "hr" {
        if let Some(c) = el.attr("color") {
            push("border-top-color", c.to_owned());
        }
    }
    if matches!(tag, "ol" | "ul") {
        if let Some(t) = el.attr("type") {
            let v = match t {
                "1" => "decimal",
                "a" => "lower-alpha",
                "A" => "upper-alpha",
                "i" => "lower-roman",
                "I" => "upper-roman",
                other => other,
            };
            push("list-style-type", v.to_owned());
        }
    }
    out
}

fn t_attr_border(attr: Option<&str>) -> Option<u32> {
    let v = attr?.trim();
    if v.is_empty() {
        return Some(1);
    }
    v.parse::<u32>().ok().filter(|&b| b > 0)
}

struct Applier<'a> {
    parent: &'a ComputedStyle,
    ctx: &'a CascadeContext<'a>,
    decoration_set: bool,
}

impl Applier<'_> {
    fn length(&self, style: &ComputedStyle, value: &str) -> Option<Length> {
        css::parse_length(value, style.font_size, self.ctx.root_font_size)
    }

    fn color(&self, style: &ComputedStyle, value: &str) -> Option<Color32> {
        let lower = value.trim().to_ascii_lowercase();
        if let Some(c) = self.ctx.theme.color(&lower) {
            return Some(c);
        }
        css::parse_color(value, style.color)
    }

    fn apply_font_size(&mut self, style: &mut ComputedStyle, name: &str, value: &str) {
        if name == "font" {
            if let Some((_, size, _)) = parse_font_shorthand(value) {
                self.set_font_size(style, size);
            }
        } else {
            self.set_font_size(style, value);
        }
    }

    fn set_font_size(&self, style: &mut ComputedStyle, value: &str) {
        let parent = self.parent.font_size;
        let base = self.ctx.base_font_size;
        let lower = value.trim().to_ascii_lowercase();
        let keyword = match lower.as_str() {
            "xx-small" => Some(9.0 / 16.0),
            "x-small" => Some(10.0 / 16.0),
            "small" => Some(13.0 / 16.0),
            "medium" => Some(1.0),
            "large" => Some(18.0 / 16.0),
            "x-large" => Some(24.0 / 16.0),
            "xx-large" => Some(2.0),
            "xxx-large" => Some(3.0),
            _ => None,
        };
        let size = if let Some(k) = keyword {
            base * k
        } else if lower == "smaller" {
            parent / 1.2
        } else if lower == "larger" {
            parent * 1.2
        } else if lower == "inherit" || lower == "unset" {
            parent
        } else if lower == "initial" {
            base
        } else {
            match css::parse_length(&lower, parent, self.ctx.root_font_size) {
                Some(Length::Px(px)) => px,
                Some(Length::Percent(p)) => parent * p / 100.0,
                _ => return,
            }
        };
        style.font_size = size.clamp(1.0, 1000.0);
    }

    fn apply(&mut self, style: &mut ComputedStyle, name: &str, value: &str) {
        let lower = value.to_ascii_lowercase();
        match lower.as_str() {
            "inherit" => return copy_property(style, self.parent, name),
            "initial" => return copy_property(style, self.ctx.initial, name),
            "unset" | "revert" | "revert-layer" => {
                let src = if is_inherited(name) {
                    self.parent
                } else {
                    self.ctx.initial
                };
                return copy_property(style, src, name);
            }
            _ => {}
        }
        let v = lower.as_str();

        match name {
            "color" => {
                if let Some(c) = self.color(style, value) {
                    style.color = c;
                }
            }
            "background-color" => {
                if let Some(c) = self.color(style, value) {
                    style.background = c;
                }
            }
            "background" => {
                // Pick the color out of the shorthand; images and gradients are not supported.
                let mut color = None;
                for token in split_whitespace_top_level(value) {
                    if let Some(c) = self.color(style, token) {
                        color = Some(c);
                    }
                }
                style.background = color.unwrap_or(Color32::TRANSPARENT);
            }
            "font-weight" => {
                if let Some(w) = self.font_weight(v) {
                    style.font_weight = w;
                }
            }
            "font-style" => style.italic = v.starts_with("italic") || v.starts_with("oblique"),
            "font-family" => style.font_family = parse_font_family(value),
            "font" => {
                if let Some((prefix, _size, rest)) = parse_font_shorthand(value) {
                    style.italic = false;
                    style.font_weight = 400;
                    for token in prefix.split_whitespace() {
                        let t = token.to_ascii_lowercase();
                        if t == "italic" || t == "oblique" {
                            style.italic = true;
                        } else if let Some(w) = self.font_weight(&t) {
                            style.font_weight = w;
                        }
                    }
                    let (line_height, family) = match rest.trim_start().strip_prefix('/') {
                        Some(r) => {
                            let r = r.trim_start();
                            let end = r.find(char::is_whitespace).unwrap_or(r.len());
                            (Some(&r[..end]), &r[end..])
                        }
                        None => (None, rest),
                    };
                    style.line_height = LineHeight::Normal;
                    if let Some(lh) = line_height {
                        self.set_line_height(style, lh);
                    }
                    if !family.trim().is_empty() {
                        style.font_family = parse_font_family(family);
                    }
                }
            }
            "line-height" => self.set_line_height(style, v),
            "text-align" => {
                style.text_align = match v {
                    "right" | "end" | "-webkit-right" => TextAlign::Right,
                    "center" | "-webkit-center" | "-moz-center" => TextAlign::Center,
                    "justify" => TextAlign::Justify,
                    _ => TextAlign::Left,
                }
            }
            "white-space" | "white-space-collapse" | "text-wrap-mode" => {
                style.white_space = match v {
                    "nowrap" => WhiteSpace::NoWrap,
                    "pre" | "preserve" => WhiteSpace::Pre,
                    "pre-wrap" | "break-spaces" => WhiteSpace::PreWrap,
                    "pre-line" | "preserve-breaks" => WhiteSpace::PreLine,
                    "wrap" if style.white_space == WhiteSpace::NoWrap => WhiteSpace::Normal,
                    "wrap" => style.white_space,
                    _ => WhiteSpace::Normal,
                }
            }
            "letter-spacing" => {
                style.letter_spacing = match self.length(style, v) {
                    Some(Length::Px(px)) => px,
                    _ => 0.0,
                }
            }
            "text-transform" => {
                style.text_transform = match v {
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    "capitalize" => TextTransform::Capitalize,
                    _ => TextTransform::None,
                }
            }
            "text-indent" => {
                if let Some(l) = self.length(style, v) {
                    style.text_indent = l;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                let mut deco = TextDecoration::default();
                for token in split_whitespace_top_level(value) {
                    match token.to_ascii_lowercase().as_str() {
                        "underline" => deco.underline = true,
                        "overline" => deco.overline = true,
                        "line-through" => deco.line_through = true,
                        _ => {
                            if name == "text-decoration" {
                                if let Some(c) = self.color(style, token) {
                                    deco.color = Some(c);
                                }
                            }
                        }
                    }
                }
                if name == "text-decoration-line" {
                    deco.color = style.decoration.color;
                }
                style.decoration = deco;
                self.decoration_set = true;
            }
            "text-decoration-color" => {
                if let Some(c) = self.color(style, value) {
                    style.decoration.color = Some(c);
                }
            }
            "list-style-type" => {
                if let Some(ls) = parse_list_style(v) {
                    style.list_style = ls;
                }
            }
            "list-style" => {
                for token in v.split_whitespace() {
                    if let Some(ls) = parse_list_style(token) {
                        style.list_style = ls;
                    }
                }
            }
            "visibility" => style.visible = v == "visible",
            "display" => {
                style.display = match v.split_whitespace().last().unwrap_or("") {
                    "none" => Display::None,
                    "block" | "flex" | "grid" | "flow-root" | "flow" | "table-column-group"
                    | "-webkit-box" | "ruby" => Display::Block,
                    "list-item" => Display::ListItem,
                    "table" | "inline-table" => Display::Table,
                    "table-row-group" | "table-header-group" | "table-footer-group" => {
                        Display::TableRowGroup
                    }
                    "table-row" => Display::TableRow,
                    "table-cell" => Display::TableCell,
                    "table-caption" => Display::TableCaption,
                    "table-column" => Display::None,
                    // inline, inline-block, inline-flex, contents, ...
                    _ => Display::Inline,
                };
                if v.starts_with("inline") && v.contains("flex") || v == "inline-grid" {
                    style.display = Display::Inline;
                }
            }
            "vertical-align" => {
                style.vertical_align = match v {
                    "sub" => VerticalAlign::Sub,
                    "super" => VerticalAlign::Super,
                    "top" | "text-top" => VerticalAlign::Top,
                    "middle" | "center" => VerticalAlign::Middle,
                    "bottom" | "text-bottom" => VerticalAlign::Bottom,
                    _ => VerticalAlign::Baseline,
                }
            }
            "margin" | "padding" => {
                let values: Vec<Length> = v
                    .split_whitespace()
                    .filter_map(|t| self.length(style, t))
                    .collect();
                if let Some(sides) = expand_sides(&values) {
                    if name == "margin" {
                        style.margin = sides;
                    } else {
                        style.padding = sides.map(|l| if l.is_auto() { Length::ZERO } else { l });
                    }
                }
            }
            "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "margin-block-start"
            | "margin-block-end"
            | "margin-inline-start"
            | "margin-inline-end" => {
                if let Some(l) = self.length(style, v) {
                    style.margin[side_index(name)] = l;
                }
            }
            "margin-block" | "margin-inline" | "padding-block" | "padding-inline" => {
                let values: Vec<Length> = v
                    .split_whitespace()
                    .filter_map(|t| self.length(style, t))
                    .collect();
                if let Some(&first) = values.first() {
                    let second = values.get(1).copied().unwrap_or(first);
                    let sides = if name.ends_with("block") {
                        [0, 2]
                    } else {
                        [3, 1]
                    };
                    let target = if name.starts_with("margin") {
                        &mut style.margin
                    } else {
                        &mut style.padding
                    };
                    target[sides[0]] = first;
                    target[sides[1]] = second;
                }
            }
            "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "padding-block-start"
            | "padding-block-end"
            | "padding-inline-start"
            | "padding-inline-end" => {
                if let Some(l) = self.length(style, v) {
                    if !l.is_auto() {
                        style.padding[side_index(name)] = l;
                    }
                }
            }
            "border"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "border-block"
            | "border-inline"
            | "border-block-start"
            | "border-block-end"
            | "border-inline-start"
            | "border-inline-end" => {
                let border = self.parse_border(style, value);
                let sides: &[usize] = match name {
                    "border" => &[0, 1, 2, 3],
                    "border-block" => &[0, 2],
                    "border-inline" => &[1, 3],
                    _ => &[side_index(name)],
                };
                for &s in sides {
                    style.border[s] = border;
                }
            }
            "border-width" => {
                let values: Vec<Length> = v
                    .split_whitespace()
                    .filter_map(|t| border_width_keyword(t).or_else(|| self.length(style, t)))
                    .collect();
                if let Some(sides) = expand_sides(&values) {
                    for (b, l) in style.border.iter_mut().zip(sides) {
                        if let Length::Px(px) = l {
                            b.width = px;
                        }
                    }
                }
            }
            "border-style" => {
                let values: Vec<BorderStyle> = v
                    .split_whitespace()
                    .filter_map(parse_border_style)
                    .collect();
                if let Some(sides) = expand_sides(&values) {
                    for (b, s) in style.border.iter_mut().zip(sides) {
                        b.style = s;
                    }
                }
            }
            "border-color" => {
                let values: Vec<Color32> = split_whitespace_top_level(value)
                    .into_iter()
                    .filter_map(|t| self.color(style, t))
                    .collect();
                if let Some(sides) = expand_sides(&values) {
                    for (b, c) in style.border.iter_mut().zip(sides) {
                        b.color = Some(c);
                    }
                }
            }
            "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width" => {
                if let Some(Length::Px(px)) =
                    border_width_keyword(v).or_else(|| self.length(style, v))
                {
                    style.border[side_index(name)].width = px;
                }
            }
            "border-top-style"
            | "border-right-style"
            | "border-bottom-style"
            | "border-left-style" => {
                if let Some(s) = parse_border_style(v) {
                    style.border[side_index(name)].style = s;
                }
            }
            "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color" => {
                if let Some(c) = self.color(style, value) {
                    style.border[side_index(name)].color = Some(c);
                }
            }
            "border-radius" => {
                if let Some(Length::Px(px)) = v
                    .split(['/', ' '])
                    .next()
                    .and_then(|t| self.length(style, t))
                {
                    style.border_radius = px.max(0.0);
                }
            }
            "border-collapse" => style.border_collapse = v == "collapse",
            "border-spacing" => {
                if let Some(Length::Px(px)) = v
                    .split_whitespace()
                    .next()
                    .and_then(|t| self.length(style, t))
                {
                    style.border_spacing = px.max(0.0);
                }
            }
            "tab-size" | "-moz-tab-size" => {
                if let Ok(n) = v.parse::<usize>() {
                    style.tab_size = n.min(32);
                }
            }
            "width" | "inline-size" => {
                if let Some(l) = self.length(style, v) {
                    style.width = l;
                }
            }
            "height" | "block-size" => {
                if let Some(l) = self.length(style, v) {
                    style.height = l;
                }
            }
            "min-width" | "min-inline-size" => {
                if let Some(l) = self.length(style, v) {
                    style.min_width = l;
                }
            }
            "max-width" | "max-inline-size" => {
                style.max_width = if v == "none" {
                    Length::Auto
                } else {
                    self.length(style, v).unwrap_or(style.max_width)
                };
            }
            "min-height" | "min-block-size" => {
                if let Some(l) = self.length(style, v) {
                    style.min_height = l;
                }
            }
            "box-sizing" => style.border_box = v == "border-box",
            "opacity" => {
                if let Some(o) = css::split_number(v).and_then(|(n, unit)| match unit {
                    "" => Some(n),
                    "%" => Some(n / 100.0),
                    _ => None,
                }) {
                    style.opacity = o.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn font_weight(&self, v: &str) -> Option<u16> {
        Some(match v {
            "normal" => 400,
            "bold" => 700,
            "bolder" => {
                if self.parent.font_weight < 600 {
                    700
                } else {
                    900
                }
            }
            "lighter" => {
                if self.parent.font_weight > 500 {
                    400
                } else {
                    100
                }
            }
            _ => {
                let n = v.parse::<f32>().ok()?;
                if !(1.0..=1000.0).contains(&n) {
                    return None;
                }
                n as u16
            }
        })
    }

    fn set_line_height(&self, style: &mut ComputedStyle, v: &str) {
        if v == "normal" {
            style.line_height = LineHeight::Normal;
            return;
        }
        if let Some((n, "")) = css::split_number(v) {
            style.line_height = LineHeight::Factor(n.max(0.0));
            return;
        }
        match self.length(style, v) {
            Some(Length::Px(px)) => style.line_height = LineHeight::Px(px.max(0.0)),
            Some(Length::Percent(p)) => {
                style.line_height = LineHeight::Px(style.font_size * p / 100.0);
            }
            _ => {}
        }
    }

    fn parse_border(&self, style: &ComputedStyle, value: &str) -> Border {
        let mut border = Border {
            width: 3.0,
            style: BorderStyle::None,
            color: None,
        };
        for token in split_whitespace_top_level(value) {
            let t = token.to_ascii_lowercase();
            if let Some(s) = parse_border_style(&t) {
                border.style = s;
            } else if let Some(Length::Px(px)) =
                border_width_keyword(&t).or_else(|| self.length(style, &t))
            {
                border.width = px;
            } else if let Some(c) = self.color(style, token) {
                border.color = Some(c);
            }
        }
        border
    }
}

fn parse_font_family(value: &str) -> Arc<[String]> {
    let families: Vec<String> = value
        .split(',')
        .map(|f| {
            f.trim()
                .trim_matches(['"', '\''])
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|f| !f.is_empty())
        .collect();
    Arc::from(families)
}

/// Splits `font` shorthand into (style/weight prefix, size, rest after size).
fn parse_font_shorthand(value: &str) -> Option<(&str, &str, &str)> {
    // The size is the first token that looks like a length or a size keyword.
    let mut offset = 0;
    for token in value.split_whitespace() {
        let start = offset + value[offset..].find(token)?;
        offset = start + token.len();
        let size_part = token.split('/').next().unwrap_or(token);
        let lower = size_part.to_ascii_lowercase();
        let is_keyword = matches!(
            lower.as_str(),
            "xx-small"
                | "x-small"
                | "small"
                | "medium"
                | "large"
                | "x-large"
                | "xx-large"
                | "xxx-large"
                | "smaller"
                | "larger"
        );
        let is_length = css::split_number(&lower)
            .is_some_and(|(n, unit)| !unit.is_empty() || n == 0.0)
            && !lower.chars().all(|c| c.is_ascii_digit());
        if is_keyword || is_length {
            let rest = &value[start + size_part.len()..];
            return Some((&value[..start], size_part, rest));
        }
    }
    None
}

fn parse_list_style(v: &str) -> Option<ListStyle> {
    Some(match v {
        "none" => ListStyle::None,
        "disc" => ListStyle::Disc,
        "circle" => ListStyle::Circle,
        "square" => ListStyle::Square,
        "decimal" => ListStyle::Decimal,
        "decimal-leading-zero" => ListStyle::DecimalLeadingZero,
        "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
        "lower-roman" => ListStyle::LowerRoman,
        "upper-roman" => ListStyle::UpperRoman,
        "lower-greek" => ListStyle::LowerGreek,
        _ => return None,
    })
}

fn parse_border_style(v: &str) -> Option<BorderStyle> {
    Some(match v {
        "none" | "hidden" => BorderStyle::None,
        "solid" | "groove" | "ridge" | "inset" | "outset" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        _ => return None,
    })
}

fn border_width_keyword(v: &str) -> Option<Length> {
    Some(Length::Px(match v {
        "thin" => 1.0,
        "medium" => 3.0,
        "thick" => 5.0,
        _ => return None,
    }))
}

/// Expands CSS 1–4 value shorthand into top, right, bottom, left.
fn expand_sides<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match *v {
        [a] => [a, a, a, a],
        [a, b] => [a, b, a, b],
        [a, b, c] => [a, b, c, b],
        [a, b, c, d, ..] => [a, b, c, d],
        [] => return None,
    })
}

fn side_index(name: &str) -> usize {
    if name.contains("top") || name.contains("block-start") {
        0
    } else if name.contains("right") || name.contains("inline-end") {
        1
    } else if name.contains("bottom") || name.contains("block-end") {
        2
    } else {
        3
    }
}

fn split_whitespace_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(st) = start.take() {
                out.push(&s[st..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

fn is_inherited(name: &str) -> bool {
    matches!(
        name,
        "color"
            | "font"
            | "font-size"
            | "font-weight"
            | "font-style"
            | "font-family"
            | "line-height"
            | "text-align"
            | "white-space"
            | "letter-spacing"
            | "text-transform"
            | "text-indent"
            | "list-style"
            | "list-style-type"
            | "visibility"
            | "border-collapse"
            | "border-spacing"
            | "tab-size"
    )
}

/// Copies the property `name` (or the properties of a shorthand) from `src` to `dst`.
fn copy_property(dst: &mut ComputedStyle, src: &ComputedStyle, name: &str) {
    match name {
        "color" => dst.color = src.color,
        "font" => {
            dst.font_size = src.font_size;
            dst.font_weight = src.font_weight;
            dst.italic = src.italic;
            dst.font_family = src.font_family.clone();
            dst.line_height = src.line_height;
        }
        "font-size" => dst.font_size = src.font_size,
        "font-weight" => dst.font_weight = src.font_weight,
        "font-style" => dst.italic = src.italic,
        "font-family" => dst.font_family = src.font_family.clone(),
        "line-height" => dst.line_height = src.line_height,
        "text-align" => dst.text_align = src.text_align,
        "white-space" => dst.white_space = src.white_space,
        "letter-spacing" => dst.letter_spacing = src.letter_spacing,
        "text-transform" => dst.text_transform = src.text_transform,
        "text-indent" => dst.text_indent = src.text_indent,
        "list-style" | "list-style-type" => dst.list_style = src.list_style,
        "visibility" => dst.visible = src.visible,
        "border-collapse" => dst.border_collapse = src.border_collapse,
        "border-spacing" => dst.border_spacing = src.border_spacing,
        "tab-size" => dst.tab_size = src.tab_size,
        "text-decoration" | "text-decoration-line" => dst.decoration = src.decoration,
        "display" => dst.display = src.display,
        "background" | "background-color" => dst.background = src.background,
        "vertical-align" => dst.vertical_align = src.vertical_align,
        "margin" => dst.margin = src.margin,
        "padding" => dst.padding = src.padding,
        "border" => dst.border = src.border,
        "border-radius" => dst.border_radius = src.border_radius,
        "width" => dst.width = src.width,
        "height" => dst.height = src.height,
        "min-width" => dst.min_width = src.min_width,
        "max-width" => dst.max_width = src.max_width,
        "min-height" => dst.min_height = src.min_height,
        "box-sizing" => dst.border_box = src.border_box,
        "opacity" => dst.opacity = src.opacity,
        _ if name.starts_with("margin-") => {
            let i = side_index(name);
            dst.margin[i] = src.margin[i];
        }
        _ if name.starts_with("padding-") => {
            let i = side_index(name);
            dst.padding[i] = src.padding[i];
        }
        _ if name.starts_with("border-") => {
            let i = side_index(name);
            dst.border[i] = src.border[i];
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars() {
        let mut vars = HashMap::new();
        vars.insert("--a".to_owned(), "red".to_owned());
        assert_eq!(substitute_vars("var(--a)", &vars), "red");
        assert_eq!(
            substitute_vars("1px solid var(--b, var(--a))", &vars),
            "1px solid red"
        );
        assert_eq!(substitute_vars("var(--missing, 3px)", &vars), "3px");
    }

    #[test]
    fn font_shorthand() {
        assert_eq!(
            parse_font_shorthand("italic bold 12px/30px Georgia, serif"),
            Some(("italic bold ", "12px", "/30px Georgia, serif"))
        );
        assert_eq!(
            parse_font_shorthand("700 1.2em sans-serif").map(|f| f.1),
            Some("1.2em")
        );
    }

    #[test]
    fn ua_stylesheet_parses() {
        let rules = css::parse_stylesheet(UA_STYLESHEET);
        assert!(rules.len() > 40);
        for rule in &rules {
            for s in &rule.selectors {
                assert!(
                    Selector::parse(s).is_ok(),
                    "UA selector failed to parse: {s}"
                );
            }
        }
    }
}

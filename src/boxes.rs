//! Turns the parsed DOM + cascade into a tree of layout boxes, with whitespace already
//! processed and inline content grouped into anonymous blocks.

use std::sync::Arc;

use egui::Color32;
use scraper::{ElementRef, Html, Node};

use crate::{
    css::Length,
    style::{
        CascadeContext, ComputedStyle, Display, ListStyle, StyleEngine, TextTransform, WhiteSpace,
    },
};

pub struct BlockBox {
    pub style: Arc<ComputedStyle>,
    pub kind: BlockKind,
    pub anchor: Option<String>,
    pub children: Vec<BoxChild>,
}

pub enum BlockKind {
    Normal,
    ListItem { marker: Option<String> },
    Table(TableBox),
    Image(ImageBox),
}

pub struct TableBox {
    pub captions: Vec<BlockBox>,
    pub rows: Vec<TableRow>,
}

pub struct TableRow {
    pub style: Arc<ComputedStyle>,
    pub cells: Vec<TableCell>,
}

pub struct TableCell {
    pub block: BlockBox,
    pub colspan: usize,
}

pub enum BoxChild {
    Block(BlockBox),
    Inline(InlineContent),
}

pub struct InlineContent {
    pub items: Vec<InlineItem>,
}

pub enum InlineItem {
    Text {
        text: String,
        style: Arc<ComputedStyle>,
        link: Option<Arc<str>>,
        /// Background of the enclosing inline elements (block backgrounds are painted separately).
        background: Color32,
    },
    Image(ImageBox),
    /// An element with an `id` inside inline content (target of `#fragment` links).
    Anchor(String),
}

#[derive(Clone)]
pub struct ImageBox {
    pub src: String,
    pub alt: String,
    pub style: Arc<ComputedStyle>,
    pub link: Option<Arc<str>>,
}

/// Something produced while walking the children of a block: either a block-level box or a
/// piece of inline content.
enum Piece {
    Block(BlockBox),
    Inline(InlineItem),
    LineBreak(Arc<ComputedStyle>),
}

pub struct BoxBuilder<'a> {
    pub engine: &'a StyleEngine,
    pub ctx: CascadeContext<'a>,
}

impl BoxBuilder<'_> {
    /// Builds the root block for the `<html>` element. Returns the root block and the
    /// canvas background (propagated from `<html>` or `<body>`, as browsers do).
    pub fn build_document(&mut self, html: &Html) -> (BlockBox, Color32) {
        let root = html.root_element();
        let root_style = Arc::new(self.engine.compute(root, self.ctx.initial, &self.ctx));
        self.ctx.root_font_size = root_style.font_size;
        let mut canvas_bg = root_style.background;
        if canvas_bg == Color32::TRANSPARENT {
            if let Some(body) = root.child_elements().find(|e| e.value().name() == "body") {
                let body_style = self.engine.compute(body, &root_style, &self.ctx);
                canvas_bg = body_style.background;
            }
        }
        let block = self.build_block(root, root_style, None, None);
        (block, canvas_bg)
    }

    fn build_block(
        &mut self,
        el: ElementRef<'_>,
        style: Arc<ComputedStyle>,
        link: Option<Arc<str>>,
        marker: Option<String>,
    ) -> BlockBox {
        let anchor = element_anchor(el);
        let kind = match style.display {
            Display::Table => BlockKind::Table(self.build_table(el, &style, link.clone())),
            Display::ListItem => BlockKind::ListItem { marker },
            _ if el.value().name() == "img" => {
                BlockKind::Image(image_box(el, &style, link.clone()))
            }
            _ => BlockKind::Normal,
        };
        let children = match kind {
            BlockKind::Normal | BlockKind::ListItem { .. } => {
                let mut pieces = Vec::new();
                self.collect_children(
                    el,
                    &style,
                    link_for(el, link),
                    Color32::TRANSPARENT,
                    &mut pieces,
                );
                group_pieces(pieces)
            }
            _ => Vec::new(),
        };
        BlockBox {
            style,
            kind,
            anchor,
            children,
        }
    }

    fn build_table(
        &mut self,
        table: ElementRef<'_>,
        style: &Arc<ComputedStyle>,
        link: Option<Arc<str>>,
    ) -> TableBox {
        let mut out = TableBox {
            captions: Vec::new(),
            rows: Vec::new(),
        };
        for child in table.child_elements() {
            let child_style = Arc::new(self.engine.compute(child, style, &self.ctx));
            match child_style.display {
                Display::None => {}
                Display::TableRowGroup => {
                    for row in child.child_elements() {
                        let row_style = Arc::new(self.engine.compute(row, &child_style, &self.ctx));
                        if row_style.display != Display::None {
                            let row = self.build_row(row, row_style, link.clone());
                            out.rows.push(row);
                        }
                    }
                }
                Display::TableRow => {
                    let row = self.build_row(child, child_style, link.clone());
                    out.rows.push(row);
                }
                Display::TableCell => {
                    // A cell directly inside a table: wrap it in an anonymous row.
                    let cell = self.build_cell(child, child_style, link.clone());
                    match out.rows.last_mut() {
                        Some(row) if Arc::ptr_eq(&row.style, style) => row.cells.push(cell),
                        _ => out.rows.push(TableRow {
                            style: style.clone(),
                            cells: vec![cell],
                        }),
                    }
                }
                _ => {
                    let caption = self.build_block(child, child_style, link.clone(), None);
                    out.captions.push(caption);
                }
            }
        }
        out
    }

    fn build_row(
        &mut self,
        row: ElementRef<'_>,
        style: Arc<ComputedStyle>,
        link: Option<Arc<str>>,
    ) -> TableRow {
        let mut cells = Vec::new();
        for cell in row.child_elements() {
            let cell_style = Arc::new(self.engine.compute(cell, &style, &self.ctx));
            if cell_style.display != Display::None {
                cells.push(self.build_cell(cell, cell_style, link.clone()));
            }
        }
        TableRow { style, cells }
    }

    fn build_cell(
        &mut self,
        cell: ElementRef<'_>,
        style: Arc<ComputedStyle>,
        link: Option<Arc<str>>,
    ) -> TableCell {
        let colspan = cell
            .value()
            .attr("colspan")
            .and_then(|c| c.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000);
        let mut block_style = style;
        if block_style.display != Display::Block {
            Arc::make_mut(&mut block_style).display = Display::Block;
        }
        TableCell {
            block: self.build_block(cell, block_style, link, None),
            colspan,
        }
    }

    /// Walks the children of `parent`, producing block boxes and inline items.
    fn collect_children(
        &mut self,
        parent: ElementRef<'_>,
        parent_style: &Arc<ComputedStyle>,
        link: Option<Arc<str>>,
        inline_bg: Color32,
        out: &mut Vec<Piece>,
    ) {
        // List item numbering for <ol>/<ul>.
        let is_list = matches!(parent.value().name(), "ol" | "ul" | "menu" | "dir");
        let reversed = is_list && parent.value().attr("reversed").is_some();
        let mut counter: i64 = parent
            .value()
            .attr("start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| {
                if reversed {
                    parent
                        .child_elements()
                        .filter(|e| e.value().name() == "li")
                        .count() as i64
                } else {
                    1
                }
            });

        for child in parent.children() {
            match child.value() {
                Node::Text(text) => {
                    out.push(Piece::Inline(InlineItem::Text {
                        text: text.to_string(),
                        style: parent_style.clone(),
                        link: link.clone(),
                        background: inline_bg,
                    }));
                }
                Node::Element(_) => {
                    let Some(el) = ElementRef::wrap(child) else {
                        continue;
                    };
                    let style = Arc::new(self.engine.compute(el, parent_style, &self.ctx));
                    let tag = el.value().name();
                    match style.display {
                        Display::None => {}
                        Display::Inline => {
                            let child_link = link_for(el, link.clone());
                            let child_bg = if style.background.a() > 0 && style.visible {
                                style.background
                            } else {
                                inline_bg
                            };
                            if let Some(anchor) = element_anchor(el) {
                                out.push(Piece::Inline(InlineItem::Anchor(anchor)));
                            }
                            match tag {
                                "br" => out.push(Piece::LineBreak(style)),
                                "img" => out.push(Piece::Inline(InlineItem::Image(image_box(
                                    el, &style, child_link,
                                )))),
                                "wbr" => out.push(Piece::Inline(InlineItem::Text {
                                    text: "\u{200B}".to_owned(),
                                    style,
                                    link: child_link,
                                    background: child_bg,
                                })),
                                "input" => {
                                    let checked = el.value().attr("checked").is_some();
                                    let radio = el
                                        .value()
                                        .attr("type")
                                        .is_some_and(|t| t.eq_ignore_ascii_case("radio"));
                                    let symbol = match (radio, checked) {
                                        (false, false) => "☐",
                                        (false, true) => "☑",
                                        (true, false) => "○",
                                        (true, true) => "◉",
                                    };
                                    out.push(Piece::Inline(InlineItem::Text {
                                        text: format!("{symbol}\u{a0}"),
                                        style,
                                        link: child_link,
                                        background: child_bg,
                                    }));
                                }
                                _ => self.collect_children(el, &style, child_link, child_bg, out),
                            }
                        }
                        Display::ListItem => {
                            let value = el
                                .value()
                                .attr("value")
                                .and_then(|v| v.trim().parse::<i64>().ok());
                            if let Some(v) = value {
                                counter = v;
                            }
                            let marker = list_marker(style.list_style, counter);
                            counter += if reversed { -1 } else { 1 };
                            let block = self.build_block(el, style, link.clone(), marker);
                            out.push(Piece::Block(block));
                        }
                        _ => {
                            let mut style = style;
                            if matches!(
                                style.display,
                                Display::TableRowGroup
                                    | Display::TableRow
                                    | Display::TableCell
                                    | Display::TableCaption
                            ) {
                                // Table parts outside of a table behave like blocks.
                                Arc::make_mut(&mut style).display = Display::Block;
                            }
                            let child_link = link_for(el, link.clone());
                            let block = self.build_block(el, style, child_link, None);
                            out.push(Piece::Block(block));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn link_for(el: ElementRef<'_>, inherited: Option<Arc<str>>) -> Option<Arc<str>> {
    if el.value().name() == "a" {
        if let Some(href) = el.value().attr("href") {
            return Some(Arc::from(href.trim()));
        }
    }
    inherited
}

fn element_anchor(el: ElementRef<'_>) -> Option<String> {
    let v = el.value();
    v.id()
        .or_else(|| (v.name() == "a").then(|| v.attr("name")).flatten())
        .map(str::to_owned)
}

fn image_box(el: ElementRef<'_>, style: &Arc<ComputedStyle>, link: Option<Arc<str>>) -> ImageBox {
    ImageBox {
        src: el.value().attr("src").unwrap_or_default().trim().to_owned(),
        alt: el.value().attr("alt").unwrap_or_default().to_owned(),
        style: style.clone(),
        link,
    }
}

/// Groups consecutive inline pieces into anonymous inline content and processes whitespace.
fn group_pieces(pieces: Vec<Piece>) -> Vec<BoxChild> {
    let mut out = Vec::new();
    let mut inline: Vec<Piece> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Block(block) => {
                if let Some(content) = finish_inline(std::mem::take(&mut inline)) {
                    out.push(BoxChild::Inline(content));
                }
                out.push(BoxChild::Block(block));
            }
            other => inline.push(other),
        }
    }
    if let Some(content) = finish_inline(inline) {
        out.push(BoxChild::Inline(content));
    }
    out
}

fn is_collapsible_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

fn finish_inline(pieces: Vec<Piece>) -> Option<InlineContent> {
    let mut items: Vec<InlineItem> = Vec::new();
    // Whether the previous character was a (collapsible) space or a line start.
    let mut after_space = true;
    let mut has_content = false;

    for piece in pieces {
        match piece {
            Piece::Block(_) => unreachable!(),
            Piece::LineBreak(style) => {
                trim_trailing_space(&mut items);
                items.push(InlineItem::Text {
                    text: "\n".to_owned(),
                    style,
                    link: None,
                    background: Color32::TRANSPARENT,
                });
                after_space = true;
                has_content = true;
            }
            Piece::Inline(InlineItem::Text {
                text,
                style,
                link,
                background,
            }) => {
                let mut s = String::with_capacity(text.len());
                match style.white_space {
                    ws if ws.collapses_spaces() => {
                        for c in text.chars() {
                            if c == '\n' && ws == WhiteSpace::PreLine {
                                while s.ends_with(' ') {
                                    s.pop();
                                }
                                s.push('\n');
                                after_space = true;
                            } else if is_collapsible_space(c) {
                                if !after_space {
                                    s.push(' ');
                                    after_space = true;
                                }
                            } else {
                                s.push(c);
                                after_space = false;
                            }
                        }
                    }
                    _ => {
                        for c in text.chars() {
                            match c {
                                '\t' => {
                                    let col =
                                        s.rsplit('\n').next().map_or(0, |l| l.chars().count());
                                    let n = style.tab_size.max(1) - col % style.tab_size.max(1);
                                    s.extend(std::iter::repeat_n(' ', n));
                                }
                                '\r' => {}
                                _ => s.push(c),
                            }
                        }
                        if let Some(last) = s.chars().last() {
                            after_space = last == ' ' || last == '\n';
                        }
                    }
                }
                let s = apply_text_transform(&s, style.text_transform);
                if s.is_empty() {
                    continue;
                }
                if s.chars().any(|c| !is_collapsible_space(c))
                    || !style.white_space.collapses_spaces()
                {
                    has_content = true;
                }
                items.push(InlineItem::Text {
                    text: s,
                    style,
                    link,
                    background,
                });
            }
            Piece::Inline(InlineItem::Image(img)) => {
                items.push(InlineItem::Image(img));
                after_space = false;
                has_content = true;
            }
            Piece::Inline(anchor @ InlineItem::Anchor(_)) => items.push(anchor),
        }
    }
    trim_trailing_space(&mut items);

    // `white-space: nowrap`: prevent wrapping by using non-breaking spaces.
    for item in &mut items {
        if let InlineItem::Text { text, style, .. } = item {
            if style.white_space == WhiteSpace::NoWrap && text.contains(' ') {
                *text = text.replace(' ', "\u{a0}");
            }
        }
    }
    items.retain(|i| !matches!(i, InlineItem::Text { text, .. } if text.is_empty()));

    has_content.then_some(InlineContent { items })
}

fn trim_trailing_space(items: &mut [InlineItem]) {
    for item in items.iter_mut().rev() {
        match item {
            InlineItem::Text { text, style, .. } => {
                if !style.white_space.collapses_spaces() {
                    return;
                }
                while text.ends_with(' ') {
                    text.pop();
                }
                if !text.is_empty() {
                    return;
                }
            }
            InlineItem::Anchor(_) => {}
            InlineItem::Image(_) => return,
        }
    }
}

fn apply_text_transform(s: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None => s.to_owned(),
        TextTransform::Uppercase => s.to_uppercase(),
        TextTransform::Lowercase => s.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(s.len());
            let mut at_word_start = true;
            for c in s.chars() {
                if at_word_start && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                } else {
                    out.push(c);
                }
                at_word_start = c.is_whitespace() || c == '-';
            }
            out
        }
    }
}

fn list_marker(style: ListStyle, n: i64) -> Option<String> {
    let alpha = |n: i64, base: u8| -> String {
        if n <= 0 {
            return n.to_string();
        }
        let mut n = n;
        let mut s = Vec::new();
        while n > 0 {
            n -= 1;
            s.push((base + (n % 26) as u8) as char);
            n /= 26;
        }
        s.iter().rev().collect()
    };
    Some(match style {
        ListStyle::None => return None,
        ListStyle::Disc => "•".to_owned(),
        ListStyle::Circle => "◦".to_owned(),
        ListStyle::Square => "▪".to_owned(),
        ListStyle::Decimal => format!("{n}."),
        ListStyle::DecimalLeadingZero => format!("{n:02}."),
        ListStyle::LowerAlpha => format!("{}.", alpha(n, b'a')),
        ListStyle::UpperAlpha => format!("{}.", alpha(n, b'A')),
        ListStyle::LowerRoman => format!("{}.", roman(n).to_lowercase()),
        ListStyle::UpperRoman => format!("{}.", roman(n)),
        ListStyle::LowerGreek => {
            const GREEK: &str = "αβγδεζηθικλμνξοπρστυφχψω";
            let letters: Vec<char> = GREEK.chars().collect();
            if n >= 1 {
                format!("{}.", letters[((n - 1) as usize) % letters.len()])
            } else {
                format!("{n}.")
            }
        }
    })
}

fn roman(n: i64) -> String {
    if !(1..4000).contains(&n) {
        return n.to_string();
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (value, numeral) in TABLE {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

impl BlockBox {
    /// True if this block has no in-flow content and nothing that separates its margins,
    /// so that its top and bottom margins collapse together.
    pub fn is_empty_collapsible(&self) -> bool {
        let s = &self.style;
        matches!(self.kind, BlockKind::Normal)
            && self.children.is_empty()
            && s.height.is_auto()
            && matches!(s.min_height, Length::Auto | Length::Px(0.0))
            && s.border[0].used_width() == 0.0
            && s.border[2].used_width() == 0.0
            && s.padding[0] == Length::ZERO
            && s.padding[2] == Length::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers() {
        assert_eq!(
            list_marker(ListStyle::LowerAlpha, 28).as_deref(),
            Some("ab.")
        );
        assert_eq!(
            list_marker(ListStyle::UpperRoman, 1994).as_deref(),
            Some("MCMXCIV.")
        );
        assert_eq!(list_marker(ListStyle::None, 1), None);
    }

    #[test]
    fn capitalize() {
        assert_eq!(
            apply_text_transform("hello big-world", TextTransform::Capitalize),
            "Hello Big-World"
        );
    }
}

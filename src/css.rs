//! A small, forgiving CSS parser: style sheets, declarations, selectors' specificity,
//! media queries and value parsing (colors, lengths).

use egui::Color32;

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// One qualified rule. `selectors` holds the comma-separated parts of the prelude.
#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: Vec<String>,
    pub declarations: Vec<Declaration>,
    /// Media conditions of the enclosing `@media` blocks (all must match).
    pub media: Vec<String>,
}

/// Environment used to evaluate media queries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaEnv {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    pub dark: bool,
}

pub fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

pub fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    parse_rules(&css, &[], &mut rules);
    rules
}

/// Finds the index of the `}` matching the `{` that precedes `s`. Returns `s.len()` if unbalanced.
fn block_end(s: &str) -> usize {
    let mut depth = 1usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    s.len()
}

fn parse_rules(css: &str, media: &[String], out: &mut Vec<Rule>) {
    let mut rest = css;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        // Skip stray closing braces and HTML comment tokens.
        if let Some(r) = rest.strip_prefix('}') {
            rest = r;
            continue;
        }
        if let Some(r) = rest
            .strip_prefix("<!--")
            .or_else(|| rest.strip_prefix("-->"))
        {
            rest = r;
            continue;
        }

        let brace = rest.find('{');
        let semi = rest.find(';');
        if rest.starts_with('@') {
            // Statement at-rules (`@import ...;`, `@charset ...;`).
            if let Some(semi) = semi {
                if brace.is_none_or(|b| semi < b) {
                    rest = &rest[semi + 1..];
                    continue;
                }
            }
        }
        let Some(brace) = brace else { break };
        let prelude = rest[..brace].trim();
        let body_start = brace + 1;
        let end = body_start + block_end(&rest[body_start..]);
        let body = &rest[body_start..end.min(rest.len())];
        rest = if end < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };

        if let Some(at) = prelude.strip_prefix('@') {
            let (keyword, condition) = at
                .split_once(|c: char| c.is_whitespace() || c == '(')
                .map_or((at, ""), |(k, _)| (k, at[k.len()..].trim()));
            match keyword.to_ascii_lowercase().as_str() {
                "media" => {
                    let mut media = media.to_vec();
                    media.push(condition.to_ascii_lowercase());
                    parse_rules(body, &media, out);
                }
                // Feature queries: we optimistically assume support.
                "supports" | "layer" | "container" | "document" | "-moz-document" => {
                    parse_rules(body, media, out);
                }
                // `@font-face`, `@keyframes`, `@page`, ... are ignored.
                _ => {}
            }
            continue;
        }

        let selectors = split_top_level(prelude, ',')
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if selectors.is_empty() {
            continue;
        }
        out.push(Rule {
            selectors,
            declarations: parse_declarations(body),
            media: media.to_vec(),
        });
    }
}

/// Splits on `sep` outside of parentheses, brackets and quotes.
pub fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ if c == sep && depth <= 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

pub fn parse_declarations(block: &str) -> Vec<Declaration> {
    let block = if block.contains("/*") {
        std::borrow::Cow::Owned(strip_comments(block))
    } else {
        std::borrow::Cow::Borrowed(block)
    };
    let mut out = Vec::new();
    for decl in split_top_level(&block, ';') {
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut value = value.trim();
        let mut important = false;
        if let Some(pos) = value.rfind('!') {
            if value[pos + 1..].trim().eq_ignore_ascii_case("important") {
                important = true;
                value = value[..pos].trim_end();
            }
        }
        if value.is_empty() {
            continue;
        }
        // Custom properties are case-sensitive.
        let name = if name.starts_with("--") {
            name.to_owned()
        } else {
            name.to_ascii_lowercase()
        };
        out.push(Declaration {
            name,
            value: value.to_owned(),
            important,
        });
    }
    out
}

// ----------------------------------------------------------------------------
// Specificity

/// Packed `(a, b, c)` specificity: ids, classes/attributes/pseudo-classes, types/pseudo-elements.
pub fn specificity(selector: &str) -> u32 {
    let (a, b, c) = specificity_tuple(selector);
    (a.min(1023) << 20) | (b.min(1023) << 10) | c.min(1023)
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == '\\' || !c.is_ascii()
}

fn specificity_tuple(sel: &str) -> (u32, u32, u32) {
    let chars: Vec<char> = sel.chars().collect();
    let n = chars.len();
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    let mut i = 0;
    let skip_ident = |i: &mut usize| {
        while *i < n && is_ident_char(chars[*i]) {
            *i += 1;
        }
    };
    while i < n {
        match chars[i] {
            '#' => {
                a += 1;
                i += 1;
                skip_ident(&mut i);
            }
            '.' => {
                b += 1;
                i += 1;
                skip_ident(&mut i);
            }
            '[' => {
                b += 1;
                while i < n && chars[i] != ']' {
                    i += 1;
                }
                i += 1;
            }
            ':' => {
                i += 1;
                let pseudo_element = i < n && chars[i] == ':';
                if pseudo_element {
                    i += 1;
                }
                let start = i;
                skip_ident(&mut i);
                let name: String = chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase();
                if i < n && chars[i] == '(' {
                    let arg_start = i + 1;
                    let mut depth = 0;
                    while i < n {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    let arg: String = chars[arg_start..i.min(n)].iter().collect();
                    i += 1;
                    match name.as_str() {
                        "not" | "is" | "has" | "matches" => {
                            let best = split_top_level(&arg, ',')
                                .into_iter()
                                .map(specificity_tuple)
                                .max()
                                .unwrap_or_default();
                            a += best.0;
                            b += best.1;
                            c += best.2;
                        }
                        "where" => {}
                        _ if pseudo_element => c += 1,
                        _ => b += 1,
                    }
                } else if pseudo_element
                    || matches!(
                        name.as_str(),
                        "before" | "after" | "first-line" | "first-letter"
                    )
                {
                    c += 1;
                } else {
                    b += 1;
                }
            }
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < n && chars[i] != q {
                    i += 1;
                }
                i += 1;
            }
            ch if is_ident_char(ch) => {
                c += 1;
                skip_ident(&mut i);
            }
            _ => i += 1,
        }
    }
    (a, b, c)
}

/// Key of the right-most compound selector, used to bucket rules for fast matching.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectorKey {
    Id(String),
    Class(String),
    Tag(String),
    Universal,
}

pub fn selector_key(selector: &str) -> SelectorKey {
    // Find the start of the last compound selector (after the last top-level combinator).
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in selector.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ' ' | '>' | '+' | '~' | '\t' | '\n' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    let compound = &selector[start..];
    let ident_after = |pos: usize| -> String {
        compound[pos + 1..]
            .chars()
            .take_while(|&c| is_ident_char(c) && c != '\\')
            .collect()
    };

    // Only consider the part outside of brackets / parentheses.
    let mut depth = 0i32;
    let mut class = None;
    for (i, c) in compound.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '#' if depth == 0 => {
                let id = ident_after(i);
                if !id.is_empty() {
                    return SelectorKey::Id(id);
                }
            }
            '.' if depth == 0 && class.is_none() => {
                let cls = ident_after(i);
                if !cls.is_empty() {
                    class = Some(cls);
                }
            }
            _ => {}
        }
    }
    if let Some(class) = class {
        return SelectorKey::Class(class);
    }
    let tag: String = compound
        .chars()
        .take_while(|&c| is_ident_char(c) && c != '\\')
        .collect();
    if tag.is_empty() {
        SelectorKey::Universal
    } else {
        SelectorKey::Tag(tag.to_ascii_lowercase())
    }
}

// ----------------------------------------------------------------------------
// Media queries

pub fn media_matches(condition: &str, env: &MediaEnv) -> bool {
    let condition = condition.trim();
    if condition.is_empty() {
        return true;
    }
    split_top_level(condition, ',')
        .into_iter()
        .any(|q| media_query_matches(q.trim(), env))
}

fn media_query_matches(query: &str, env: &MediaEnv) -> bool {
    let mut q = query;
    let mut negate = false;
    if let Some(r) = q.strip_prefix("not ") {
        negate = true;
        q = r.trim_start();
    } else if let Some(r) = q.strip_prefix("only ") {
        q = r.trim_start();
    }

    let mut result = true;
    for part in split_and(q) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let ok = if let Some(inner) = part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
            media_feature_matches(inner.trim(), env)
        } else {
            matches!(part, "all" | "screen")
        };
        if !ok {
            result = false;
            break;
        }
    }
    result != negate
}

fn split_and(q: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = q.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b' ' if depth == 0 && q[i..].starts_with(" and ") => {
                parts.push(&q[start..i]);
                i += 5;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&q[start..]);
    parts
}

fn media_feature_matches(feature: &str, env: &MediaEnv) -> bool {
    let Some((name, value)) = feature.split_once(':') else {
        // Boolean features like `(color)` or `(hover)`.
        return matches!(
            feature,
            "color" | "hover" | "pointer" | "any-hover" | "any-pointer"
        );
    };
    let name = name.trim();
    let value = value.trim();
    let px = || parse_media_length(value);
    match name {
        "prefers-color-scheme" => (value == "dark") == env.dark,
        "min-width" => px().is_some_and(|v| env.width >= v),
        "max-width" => px().is_some_and(|v| env.width <= v),
        "width" => px().is_some_and(|v| (env.width - v).abs() < 0.5),
        "min-height" => px().is_some_and(|v| env.height >= v),
        "max-height" => px().is_some_and(|v| env.height <= v),
        "orientation" => (value == "landscape") == (env.width >= env.height),
        "hover" | "any-hover" => value == "hover",
        "pointer" | "any-pointer" => value == "fine",
        "prefers-reduced-motion" | "prefers-contrast" | "forced-colors" => {
            value == "no-preference" || value == "none"
        }
        _ => false,
    }
}

fn parse_media_length(value: &str) -> Option<f32> {
    match parse_length(value, 16.0, 16.0)? {
        Length::Px(v) => Some(v),
        _ => None,
    }
}

// ----------------------------------------------------------------------------
// Values

/// A computed length. `em`/`rem`/absolute units are resolved to [`Length::Px`] at cascade
/// time; percentages and viewport units are resolved during layout.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Length {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
    Vw(f32),
    Vh(f32),
}

impl Length {
    pub const ZERO: Self = Self::Px(0.0);

    pub fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

/// Splits a number from its unit: `"12.5px"` -> `(12.5, "px")`.
pub fn split_number(s: &str) -> Option<(f32, &str)> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0))
                && !((c == 'e' || c == 'E')
                    && s[i + 1..].starts_with(|n: char| n.is_ascii_digit() || n == '-'))
        })
        .map_or(s.len(), |(i, _)| i);
    if end == 0 {
        return None;
    }
    let number = s[..end].parse::<f32>().ok()?;
    Some((number, s[end..].trim()))
}

/// Parses a CSS length. `em` resolves against `font_size`, `rem` against `root_font_size`.
pub fn parse_length(s: &str, font_size: f32, root_font_size: f32) -> Option<Length> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("auto") {
        return Some(Length::Auto);
    }
    if let Some(inner) = s
        .strip_prefix("calc(")
        .or_else(|| s.strip_prefix("min("))
        .or_else(|| s.strip_prefix("max("))
        .or_else(|| s.strip_prefix("clamp("))
        .and_then(|r| r.strip_suffix(')'))
    {
        // Rough support: `calc()` of a single term and `min/max/clamp` → first resolvable arg.
        for part in split_top_level(inner, ',') {
            if let Some(len) = parse_length(part, font_size, root_font_size) {
                return Some(len);
            }
        }
        return None;
    }
    let (n, unit) = split_number(s)?;
    let px = match unit.to_ascii_lowercase().as_str() {
        "" if n == 0.0 => 0.0,
        "px" => n,
        "em" => n * font_size,
        "rem" => n * root_font_size,
        "ex" | "ch" => n * font_size * 0.5,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "q" => n * 96.0 / 101.6,
        "%" => return Some(Length::Percent(n)),
        "vw" | "vmin" => return Some(Length::Vw(n)),
        "vh" | "vmax" => return Some(Length::Vh(n)),
        _ => return None,
    };
    Some(Length::Px(px))
}

/// Parses a CSS color. `current` is used for `currentcolor`.
pub fn parse_color(s: &str, current: Color32) -> Option<Color32> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }
    if lower == "transparent" {
        return Some(Color32::TRANSPARENT);
    }
    if lower == "currentcolor" {
        return Some(current);
    }
    if let Some((func, args)) = lower.split_once('(') {
        let args = args.strip_suffix(')')?;
        return parse_color_function(func.trim(), args);
    }
    named_color(&lower)
}

fn parse_hex(hex: &str) -> Option<Color32> {
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    if !hex.is_ascii() {
        return None;
    }
    match hex.len() {
        3 => Some(Color32::from_rgb(digit(0)?, digit(1)?, digit(2)?)),
        4 => Some(Color32::from_rgba_unmultiplied(
            digit(0)?,
            digit(1)?,
            digit(2)?,
            digit(3)?,
        )),
        6 => Some(Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            pair(0)?,
            pair(2)?,
            pair(4)?,
            pair(6)?,
        )),
        _ => None,
    }
}

fn parse_color_function(func: &str, args: &str) -> Option<Color32> {
    // Accept both `rgb(1, 2, 3, 0.5)` and `rgb(1 2 3 / 50%)`.
    let normalized = args.replace(['/', ','], " ");
    let parts: Vec<&str> = normalized.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let alpha = match parts.get(3) {
        Some(a) => parse_alpha(a)?,
        None => 1.0,
    };
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    match func {
        "rgb" | "rgba" => {
            let channel = |p: &str| -> Option<u8> {
                let v = if let Some(pct) = p.strip_suffix('%') {
                    pct.parse::<f32>().ok()? * 2.55
                } else {
                    p.parse::<f32>().ok()?
                };
                Some(v.round().clamp(0.0, 255.0) as u8)
            };
            Some(Color32::from_rgba_unmultiplied(
                channel(parts[0])?,
                channel(parts[1])?,
                channel(parts[2])?,
                a,
            ))
        }
        "hsl" | "hsla" => {
            let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
            let s = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let l = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let (r, g, b) = hsl_to_rgb(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
            Some(Color32::from_rgba_unmultiplied(r, g, b, a))
        }
        _ => None,
    }
}

fn parse_alpha(s: &str) -> Option<f32> {
    if let Some(pct) = s.strip_suffix('%') {
        Some(pct.parse::<f32>().ok()? / 100.0)
    } else {
        s.parse::<f32>().ok()
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let to_u8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    (
        to_u8(hue(h + 1.0 / 3.0)),
        to_u8(hue(h)),
        to_u8(hue(h - 1.0 / 3.0)),
    )
}

fn named_color(name: &str) -> Option<Color32> {
    NAMED_COLORS
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| {
            let v = NAMED_COLORS[i].1;
            Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
        })
}

/// All CSS named colors, sorted by name.
const NAMED_COLORS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_are_sorted() {
        assert!(NAMED_COLORS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn colors() {
        let c = Color32::BLACK;
        assert_eq!(parse_color("#f00", c), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(
            parse_color("#00ff0080", c),
            Some(Color32::from_rgba_unmultiplied(0, 255, 0, 128))
        );
        assert_eq!(
            parse_color("rgb(1, 2, 3)", c),
            Some(Color32::from_rgb(1, 2, 3))
        );
        assert_eq!(
            parse_color("rgb(10 20 30 / 50%)", c),
            Some(Color32::from_rgba_unmultiplied(10, 20, 30, 128))
        );
        assert_eq!(
            parse_color("hsl(0, 100%, 50%)", c),
            Some(Color32::from_rgb(255, 0, 0))
        );
        assert_eq!(
            parse_color("RebeccaPurple", c),
            Some(Color32::from_rgb(0x66, 0x33, 0x99))
        );
        assert_eq!(parse_color("nonsense", c), None);
    }

    #[test]
    fn lengths() {
        assert_eq!(parse_length("10px", 16.0, 16.0), Some(Length::Px(10.0)));
        assert_eq!(parse_length("2em", 10.0, 16.0), Some(Length::Px(20.0)));
        assert_eq!(parse_length("1.5rem", 10.0, 16.0), Some(Length::Px(24.0)));
        assert_eq!(parse_length("50%", 10.0, 16.0), Some(Length::Percent(50.0)));
        assert_eq!(parse_length("0", 10.0, 16.0), Some(Length::Px(0.0)));
        assert_eq!(parse_length("12pt", 10.0, 16.0), Some(Length::Px(16.0)));
        assert_eq!(parse_length("-1e1px", 10.0, 16.0), Some(Length::Px(-10.0)));
        assert_eq!(parse_length("10", 10.0, 16.0), None);
    }

    #[test]
    fn stylesheet() {
        let rules = parse_stylesheet(
            "/* c */ @charset \"utf-8\"; h1, .a > b { color: red !important; margin:0 }\
             @media (max-width: 600px) { p { color: blue } }\
             @font-face { font-family: x; src: url(a;b) }\
             div { background: url(\"a;b\") red; }",
        );
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].selectors, vec!["h1", ".a > b"]);
        assert!(rules[0].declarations[0].important);
        assert_eq!(rules[1].media, vec!["(max-width: 600px)"]);
        assert_eq!(rules[2].declarations[0].value, "url(\"a;b\") red");
    }

    #[test]
    fn specificities() {
        assert!(specificity("#a") > specificity(".a.b.c"));
        assert!(specificity(".a") > specificity("div p span"));
        assert_eq!(specificity("div > p"), 2);
        assert_eq!(specificity("a:not(.x)"), (1 << 10) | 1);
        assert_eq!(specificity("li:nth-child(2n+1)"), (1 << 10) | 1);
        assert_eq!(specificity("*"), 0);
    }

    #[test]
    fn keys() {
        assert_eq!(
            selector_key("div > p.note"),
            SelectorKey::Class("note".into())
        );
        assert_eq!(selector_key("ul li"), SelectorKey::Tag("li".into()));
        assert_eq!(
            selector_key("#main a:not(.x)"),
            SelectorKey::Tag("a".into())
        );
        assert_eq!(selector_key("a#top"), SelectorKey::Id("top".into()));
        assert_eq!(selector_key("[href]"), SelectorKey::Universal);
    }

    #[test]
    fn media() {
        let env = MediaEnv {
            width: 800.0,
            height: 600.0,
            dark: true,
        };
        assert!(media_matches("screen and (min-width: 600px)", &env));
        assert!(!media_matches("print", &env));
        assert!(media_matches("print, (prefers-color-scheme: dark)", &env));
        assert!(!media_matches("(max-width: 40em)", &env));
        assert!(media_matches("not print", &env));
    }
}

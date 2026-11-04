//! 文本排版引擎。
//!
//! 核心设计 —— **字体回退链（fallback chain）= 单文档多字体的实现机制**：
//! 一次排版传入若干字体构成的候选序列，逐字符从左到右，第一个包含该字符的
//! 字体负责绘制该字符；都不含时留空并记入 `missing`。因此「中文楷体 + Hershey
//! 英文」自动实现中英混排，与 Python 原版行为一致。
//!
//! 所有输出坐标为页面单位(mm)，Y 轴向上，行首基线起点由 `origin` 指定。
//!
//! 本模块是 `fonts/layout.py` 的移植：
//! * [`measure_text`] ↔ `measure_text_width`（免笔画快速度量）
//! * [`layout_text`] ↔ `layout_text` / `_layout_horizontal`
//! * [`wrap_text`] ↔ `content/markdown.py:_wrap`（Markdown 用的分词折行）

use std::collections::HashSet;

use crate::font::FontFamily;
use crate::geom::Vec2;
use crate::stroke::Stroke;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// 排版样式（mm）。
#[derive(Clone, Debug)]
pub struct TextStyle {
    /// 字号（em 高度，mm）
    pub size: f64,
    /// 行距倍数（× 行高）
    pub line_spacing: f64,
    /// 额外字距（mm），均匀加在每字步距上
    pub char_spacing: f64,
    pub align: Align,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 10.0,
            line_spacing: 1.4,
            char_spacing: 0.0,
            align: Align::Left,
        }
    }
}

impl TextStyle {
    pub fn new(size: f64) -> Self {
        Self {
            size,
            ..Default::default()
        }
    }

    pub fn line_height(&self, font: &FontFamily) -> f64 {
        font.line_height(self.size, self.line_spacing)
    }
}

/// 单个已排版字符（页面单位，已含 origin 平移）。
#[derive(Clone, Debug)]
pub struct CharPlacement {
    pub ch: char,
    /// 在 `fonts` 里的下标
    pub font: usize,
    /// 该字基线左端点在页面坐标中的位置
    pub origin: Vec2,
    /// 字体单位 → mm
    pub scale: f64,
    pub advance: f64,
    pub strokes: Vec<Stroke>,
}

impl CharPlacement {
    pub fn bbox(&self) -> crate::geom::BBox {
        crate::geom::BBox::from_points(self.strokes.iter().flat_map(|s| s.points.iter().copied()))
    }
}

/// 一行（视觉行）的排版结果。
#[derive(Clone, Debug, Default)]
pub struct LineLayout {
    pub text: String,
    pub baseline_y: f64,
    pub width: f64,
    pub chars: Vec<CharPlacement>,
    pub x_start: f64,
}

impl LineLayout {
    pub fn bbox(&self) -> crate::geom::BBox {
        let mut b = crate::geom::BBox::empty();
        for c in &self.chars {
            b = b.union(c.bbox());
        }
        b
    }
}

/// 整段文本的排版结果。
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub lines: Vec<LineLayout>,
    /// 回退链中没有任何字体可绘制、被跳过并留空的字符（去重、保持顺序）
    pub missing: Vec<char>,
}

impl TextLayout {
    pub fn strokes(&self) -> Vec<Stroke> {
        self.lines
            .iter()
            .flat_map(|l| l.chars.iter())
            .flat_map(|c| c.strokes.iter().cloned())
            .collect()
    }

    /// 非空行的最大**步距跨度**（mm），不依赖字形笔画。
    pub fn advance_span(&self) -> f64 {
        self.lines
            .iter()
            .filter(|l| !l.chars.is_empty())
            .map(|l| l.x_start + l.width)
            .fold(0.0_f64, f64::max)
    }
}

/// 为字符挑选字体：链上第一个包含该字的字体（= 互相兜底）。返回下标。
pub fn pick_font(fonts: &[FontFamily], ch: char) -> Option<usize> {
    fonts.iter().position(|f| f.has(ch))
}

/// 宽字符（CJK 各块 + 全角形式）：排版上可在其前后断行。
pub fn is_wide(ch: char) -> bool {
    let o = ch as u32;
    (0x1100..=0x115F).contains(&o)
        || (0x2E80..=0xA4CF).contains(&o)
        || (0xAC00..=0xD7A3).contains(&o)
        || (0xF900..=0xFAFF).contains(&o)
        || (0xFE30..=0xFE4F).contains(&o)
        || (0xFF00..=0xFF60).contains(&o)
        || (0xFFE0..=0xFFE6).contains(&o)
        || (0x20000..=0x3FFFD).contains(&o)
}

/// 行首禁排（闭合标点不落行首）
const CLOSING_NO_HEAD: &str = "、。，．！？：；）］｝》」』】〉％‰°…—”’.,;:!?%)]}";
/// 行尾禁排（开口标点不悬行尾）
const OPENING_NO_TAIL: &str = "（［｛《「『【〈“‘([{";

/// `prev` 与 `ch` 之间是否允许换行（中文排版惯例的简化版）。
pub fn can_break_before(prev: Option<char>, ch: char) -> bool {
    let Some(prev) = prev else { return false };
    if prev == '\n' {
        return false;
    }
    if ch.is_whitespace() {
        return false;
    }
    if prev.is_whitespace() {
        return true;
    }
    if CLOSING_NO_HEAD.contains(ch) {
        return false;
    }
    if OPENING_NO_TAIL.contains(prev) {
        return false;
    }
    is_wide(ch) || is_wide(prev)
}

/// 只算一行文本的**步距宽度**（mm），不构造任何排版对象。
pub fn measure_text(text: &str, fonts: &[FontFamily], style: &TextStyle) -> f64 {
    if text.is_empty() || fonts.is_empty() {
        return 0.0;
    }
    let fallback_scale = fonts[0].scale_for_size(style.size);
    let mut total = 0.0;
    for ch in text.chars() {
        if ch == '\n' {
            break;
        }
        let Some(fi) = pick_font(fonts, ch) else {
            total += fonts[0].default_advance * fallback_scale + style.char_spacing;
            continue;
        };
        let font = &fonts[fi];
        let scale = font.scale_for_size(style.size);
        if ch == '\t' {
            total += 4.0 * font.default_advance * scale;
            continue;
        }
        let Some(g) = font.glyph(ch) else { continue };
        total += g.advance * scale + style.char_spacing;
    }
    total
}

/// 文本中最宽单字符的步距宽（表格列宽下限：至少放得下一个字）。
pub fn widest_char_width(text: &str, fonts: &[FontFamily], style: &TextStyle) -> f64 {
    let mut best: f64 = 0.0;
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        best = best.max(measure_text(s, fonts, style));
    }
    best
}

fn line_height(fonts: &[FontFamily], style: &TextStyle) -> f64 {
    match fonts.first() {
        Some(f) => f.line_height(style.size, style.line_spacing),
        None => style.size * style.line_spacing,
    }
}

struct MissingSink {
    seen: HashSet<char>,
    out: Vec<char>,
}

impl MissingSink {
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            out: Vec::new(),
        }
    }
    fn note(&mut self, ch: char) {
        if ch != ' ' && self.seen.insert(ch) {
            self.out.push(ch);
        }
    }
}

/// 沿一条基线累积一行字符（不折行、不做对齐），返回 (字符, 行内 x 位置)。
fn accumulate_line(
    raw: &[char],
    fonts: &[FontFamily],
    style: &TextStyle,
    origin: Vec2,
    with_strokes: bool,
    missing: &mut MissingSink,
) -> Vec<(CharPlacement, f64)> {
    let (ox, oy) = origin;
    let mut cand: Vec<(CharPlacement, f64)> = Vec::with_capacity(raw.len());
    let mut x = 0.0;
    for ch in raw.iter().copied() {
        let Some(fi) = pick_font(fonts, ch) else {
            // 字体链都画不出：先用内置单线符号补（Ω ± ≤ • …），补不上才留空
            if let Some((sym_adv, sym_strokes)) =
                crate::symbols::fallback(ch, style.size, (ox + x, oy))
            {
                let adv = sym_adv + style.char_spacing;
                cand.push((
                    CharPlacement {
                        ch,
                        font: usize::MAX, // 内置符号字体
                        origin: (ox + x, oy),
                        scale: 1.0,
                        advance: adv,
                        strokes: if with_strokes {
                            sym_strokes
                        } else {
                            Vec::new()
                        },
                    },
                    x,
                ));
                x += adv;
                continue;
            }
            missing.note(ch);
            // 无字体可绘制：按链首字体的平均宽度留空
            if let Some(f0) = fonts.first() {
                x += f0.default_advance * f0.scale_for_size(style.size) + style.char_spacing;
            }
            continue;
        };
        let font = &fonts[fi];
        let scale = font.scale_for_size(style.size);
        if ch == '\t' {
            x += 4.0 * font.default_advance * scale;
            continue;
        }
        let Some(g) = font.glyph(ch) else { continue };
        let adv = g.advance * scale + style.char_spacing;
        let strokes = if with_strokes {
            g.to_strokes(scale, (ox + x, oy))
        } else {
            Vec::new()
        };
        cand.push((
            CharPlacement {
                ch,
                font: fi,
                origin: (ox + x, oy),
                scale,
                advance: adv,
                strokes,
            },
            x,
        ));
        x += adv;
    }
    cand
}

/// 把一行的候选字符按宽度分组（自动换行），返回每个视觉行的候选下标区间。
fn split_wrapped(cand: &[(CharPlacement, f64)], raw: &[char], wrap_w: f64) -> Vec<(usize, usize)> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut last_break: Option<usize> = None;
    for i in 0..cand.len() {
        let (pl, xi) = &cand[i];
        while i > start && xi + pl.advance - cand[start].1 > wrap_w {
            let bp = match last_break {
                Some(b) if b > start => b,
                _ => i,
            };
            if bp <= start {
                break; // 当前字符独占一行也放不下：允许超宽，防止死循环
            }
            groups.push((start, bp));
            start = bp;
            last_break = None;
        }
        // 该字符前的原始字符
        let ti = cand[i].0.ch;
        if i > 0 {
            let prev = raw.get(i - 1).copied();
            let _ = ti;
            if can_break_before(prev, cand[i].0.ch) {
                last_break = Some(i);
            }
        }
    }
    groups.push((start, cand.len()));
    groups
}

/// 把 `text` 按 `fonts` 回退链排版。
///
/// `text` 中的 `\n` 换行；空行保留行高。`wrap_width > 0` 时按该宽度自动折行。
pub fn layout_text(
    text: &str,
    fonts: &[FontFamily],
    style: &TextStyle,
    origin: Vec2,
    wrap_width: f64,
) -> TextLayout {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = TextLayout::default();
    if fonts.is_empty() {
        return result;
    }
    let lh = line_height(fonts, style);
    let mut missing = MissingSink::new();
    let (ox, oy) = origin;
    let mut row_index = 0usize;

    for raw_str in text.split('\n') {
        let raw: Vec<char> = raw_str.chars().collect();
        let mut cand = accumulate_line(&raw, fonts, style, origin, true, &mut missing);
        let groups: Vec<(usize, usize)> = if wrap_width > 0.0 && !cand.is_empty() {
            split_wrapped(&cand, &raw, wrap_width)
        } else {
            vec![(0, cand.len())]
        };

        for (a, b) in groups {
            let baseline_y = oy - row_index as f64 * lh;
            let mut line = LineLayout {
                baseline_y,
                ..Default::default()
            };
            if b > a {
                let x0 = cand[a].1;
                let x_last = cand[b - 1].1;
                let last_advance = cand[b - 1].0.advance;
                let mut w_line = x_last + last_advance - x0;
                // 末字额外字距是「字与字之间」的量，不计入行宽
                if style.char_spacing != 0.0 {
                    w_line -= style.char_spacing;
                }
                // 行尾空白不占宽度
                for j in (a..b).rev() {
                    let pl = &cand[j].0;
                    if pl.ch == ' ' || pl.ch == '\t' {
                        w_line -= pl.advance;
                    } else {
                        break;
                    }
                }
                let dx = match style.align {
                    Align::Center => -w_line / 2.0,
                    Align::Right => -w_line,
                    Align::Left => 0.0,
                };
                let sdx = dx - x0;
                let sdy = baseline_y - oy;
                for j in a..b {
                    let (pl, xi) = &mut cand[j];
                    if sdx != 0.0 || sdy != 0.0 {
                        for s in &mut pl.strokes {
                            for p in &mut s.points {
                                p.0 += sdx;
                                p.1 += sdy;
                            }
                        }
                    }
                    pl.origin = (ox + *xi - x0 + dx, baseline_y);
                    line.chars.push(pl.clone());
                }
                line.width = w_line;
                line.x_start = ox + dx;
                line.text = raw_str.to_string();
            } else {
                line.x_start = ox;
                line.text = raw_str.to_string();
            }
            result.lines.push(line);
            row_index += 1;
        }
    }
    result.missing = missing.out;
    result
}

/// 单行文字排版（不折行）——Markdown 的 `_append_text` 用这个。
///
/// 返回该行的步距宽度（mm）。
pub fn append_text(
    strokes: &mut Vec<Stroke>,
    text: &str,
    fonts: &[FontFamily],
    style: &TextStyle,
    x: f64,
    baseline_y: f64,
    missing: &mut Vec<char>,
    missing_seen: &mut HashSet<char>,
) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let raw: Vec<char> = text.chars().collect();
    let mut sink = MissingSink::new();
    let cand = accumulate_line(&raw, fonts, style, (x, baseline_y), true, &mut sink);
    for ch in sink.out {
        if missing_seen.insert(ch) {
            missing.push(ch);
        }
    }
    if cand.is_empty() {
        return 0.0;
    }
    let mut width = 0.0;
    for (pl, xi) in &cand {
        width = *xi + pl.advance;
        strokes.extend(pl.strokes.iter().cloned());
    }
    // 末字额外字距不计入行宽
    if style.char_spacing != 0.0 {
        width -= style.char_spacing;
    }
    width
}

// ---------------------------------------------------------------------------
// Markdown 用的分词折行（`content/markdown.py:_wrap` 的移植）
// ---------------------------------------------------------------------------
fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '@' | '#' | '$' | '%' | '&' | '.' | '-' | '_')
}

/// 按「西文单词 + 单个 CJK 字符」切分成 token。
fn tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if is_word_char(ch) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else if ch.is_whitespace() {
            let start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else {
            out.push(ch.to_string());
            i += 1;
        }
    }
    out
}

/// 按宽度自动换行（对中英文均按字符/单词边界处理；超宽 token 硬切）。
pub fn wrap_text(
    text: &str,
    fonts: &[FontFamily],
    style: &TextStyle,
    max_width: f64,
) -> Vec<String> {
    if max_width <= 0.0 || text.is_empty() {
        return vec![text.to_string()];
    }
    if measure_text(text, fonts, style) <= max_width {
        return vec![text.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for tok in tokenize(text) {
        let mut tok = tok;
        // 单个 token 比整行还宽：硬切到放得下为止
        while measure_text((cur.clone() + &tok).trim(), fonts, style) > max_width
            && measure_text(tok.trim(), fonts, style) > max_width
        {
            if !cur.trim().is_empty() {
                lines.push(cur.trim_end().to_string());
                cur.clear();
            }
            let chars: Vec<char> = tok.chars().collect();
            let mut cut = chars.len();
            while cut > 1 {
                let head: String = chars[..cut].iter().collect();
                if measure_text(head.trim(), fonts, style) <= max_width {
                    break;
                }
                cut -= 1;
            }
            if cut <= 1 {
                break; // 一个字符都放不下（max_width 过小）：放弃硬切
            }
            let head: String = chars[..cut].iter().collect();
            lines.push(head.trim_end().to_string());
            let rest: String = chars[cut..].iter().collect();
            tok = rest.trim_start().to_string();
            if tok.is_empty() {
                break;
            }
        }
        if tok.is_empty() {
            continue;
        }
        let candidate = format!("{cur}{tok}");
        if !cur.is_empty() && measure_text(candidate.trim(), fonts, style) > max_width {
            lines.push(cur.trim_end().to_string());
            cur = tok.trim_start().to_string();
        } else {
            cur = candidate;
        }
    }
    if !cur.trim().is_empty() {
        lines.push(cur.trim_end().to_string());
    }
    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontRegistry;

    fn fonts() -> Vec<FontFamily> {
        let mut reg = FontRegistry::builtin();
        vec![reg.get("futural").unwrap().clone()]
    }

    #[test]
    fn measure_matches_python_reference() {
        let f = fonts();
        let st = TextStyle::new(4.0);
        // Python: measure_text_width("Hello", [futural], TextStyle(size=4)) == 9.375
        assert!((measure_text("Hello", &f, &st) - 9.375).abs() < 1e-12);
        assert!((measure_text("AB", &f, &st) - 4.875).abs() < 1e-12);
    }

    #[test]
    fn wrap_breaks_at_spaces() {
        let f = fonts();
        let st = TextStyle::new(4.0);
        // Python: _wrap("hello world foo bar baz", [futural], size 4, 25.0) → 2 行
        let lines = wrap_text("hello world foo bar baz", &f, &st, 25.0);
        assert_eq!(lines.len(), 2, "lines={lines:?}");
        for l in &lines {
            assert!(measure_text(l, &f, &st) <= 25.0 + 1e-6, "line={l}");
        }
    }

    #[test]
    fn cjk_breaks_between_characters() {
        assert!(can_break_before(Some('中'), '文'));
        assert!(can_break_before(Some('a'), '中'));
        assert!(!can_break_before(Some('中'), '，'));
        assert!(!can_break_before(Some('（'), '中'));
        assert!(can_break_before(Some(' '), 'a'));
    }

    #[test]
    fn layout_produces_strokes() {
        let f = fonts();
        let st = TextStyle::new(4.0);
        let lay = layout_text("AB", &f, &st, (0.0, 0.0), 0.0);
        // Python: layout_text("AB", [futural], size 4) → 1 行 / 2 字 / 6 笔 / 宽 4.875
        assert_eq!(lay.lines.len(), 1);
        assert_eq!(lay.lines[0].chars.len(), 2);
        assert!((lay.lines[0].width - 4.875).abs() < 1e-12);
        assert_eq!(lay.strokes().len(), 6);
    }
}

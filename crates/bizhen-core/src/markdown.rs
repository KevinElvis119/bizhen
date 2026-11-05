//! Markdown 排版：把 Markdown 文本渲染成**分页的**笔画。
//!
//! 覆盖：标题（1–6 级）、段落（按行宽自动折行）、无序/有序列表、引用、代码块、
//! 水平线、**表格**（自动列宽 + 对齐 + 合并单元格 + 手绘抖动）、**图片占位**、
//! 行内/独立公式。
//!
//! 与原 Python 版的关系：块级渲染逐条照搬 `content/markdown.py` 的算法与默认参数
//! （基线推进、标题缩放、表格贴底线写字、单元格内边距……），**新增**的是分页：
//! 原版把所有内容排进一个无限高的对象里，这里按纸张尺寸切页。
//!
//! 坐标：页面坐标，单位 mm，**Y 轴向上**，原点在**纸张左下角**。

use std::collections::HashSet;

use pulldown_cmark::{Alignment as MdAlign, Event, Options, Parser, Tag, TagEnd};

use crate::font::FontFamily;
use crate::geom::BBox;
use crate::layout::{self, measure_text, widest_char_width, Align, TextStyle};
use crate::math;
use crate::stroke::{Role, Stroke};
use crate::symbols::SymbolFonts;

/// 单元格内边距（mm）
pub const CELL_PAD_MM: f64 = 1.5;

/// Markdown 排版样式（mm）。默认值与原版一致。
#[derive(Clone, Debug)]
pub struct MarkdownStyle {
    /// 正文字号
    pub size: f64,
    /// 行距倍数（× 行高）
    pub line_spacing: f64,
    /// 额外字距（mm）
    pub char_spacing: f64,
    /// 段落折行宽度（mm）；0 = 用页面内容宽
    pub wrap_width: f64,
    /// 表格整体宽度（mm）；0 = 按内容自适应
    pub table_width: f64,
    /// 表格线端随机偏移 σ(mm)
    pub table_end_jitter: f64,
    /// 表格线端随机出头上限(mm)
    pub table_overshoot: f64,
    /// 行界随机偏移 σ(mm)
    pub table_row_jitter: f64,
    /// 列界随机偏移 σ(mm)
    pub table_col_jitter: f64,
    pub table_seed: u64,
    pub heading_scale: [f64; 6],
    pub list_indent: f64,
    pub quote_indent: f64,
    pub math_size_scale: f64,
    /// 手写随机扰动（默认关闭）
    pub jitter: crate::sim::JitterParams,
}

impl Default for MarkdownStyle {
    fn default() -> Self {
        Self {
            size: 4.0,
            line_spacing: 1.5,
            char_spacing: 0.0,
            wrap_width: 160.0,
            table_width: 0.0,
            table_end_jitter: 0.5,
            table_overshoot: 0.6,
            table_row_jitter: 0.0,
            table_col_jitter: 0.0,
            table_seed: 20260915,
            heading_scale: [1.9, 1.6, 1.4, 1.25, 1.15, 1.1],
            list_indent: 6.0,
            quote_indent: 6.0,
            math_size_scale: 1.0,
            jitter: crate::sim::JitterParams::default(),
        }
    }
}

/// 纸张规格（mm）。内容区 = 去掉四周页边距。
#[derive(Clone, Copy, Debug)]
pub struct PageSpec {
    pub width: f64,
    pub height: f64,
    /// 上边距（mm）
    pub margin_top: f64,
    /// 下边距（mm）
    pub margin_bottom: f64,
    /// 左边距（mm）
    pub margin_left: f64,
    /// 右边距（mm）
    pub margin_right: f64,
}

impl PageSpec {
    /// 四边相同的页边距。
    pub fn with_margin(width: f64, height: f64, margin: f64) -> Self {
        Self {
            width,
            height,
            margin_top: margin,
            margin_bottom: margin,
            margin_left: margin,
            margin_right: margin,
        }
    }

    pub fn a4() -> Self {
        Self::with_margin(210.0, 297.0, 10.0)
    }

    /// 四边相同时返回该值，否则返回 `None`。
    pub fn uniform_margin(&self) -> Option<f64> {
        let m = self.margin_top;
        (self.margin_bottom == m && self.margin_left == m && self.margin_right == m).then_some(m)
    }

    pub fn content_width(&self) -> f64 {
        self.width - self.margin_left - self.margin_right
    }

    pub fn content_height(&self) -> f64 {
        self.height - self.margin_top - self.margin_bottom
    }

    pub fn landscape(self) -> Self {
        Self {
            width: self.height,
            height: self.width,
            margin_top: self.margin_top,
            margin_bottom: self.margin_bottom,
            margin_left: self.margin_left,
            margin_right: self.margin_right,
        }
    }
}

impl Default for PageSpec {
    fn default() -> Self {
        Self::a4()
    }
}

/// 行内片段：普通文字 / 行内公式。
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Text(String),
    Math(String),
}

impl Seg {
    fn plain(&self) -> String {
        match self {
            Seg::Text(t) => t.clone(),
            Seg::Math(m) => format!("${m}$"),
        }
    }
}

fn segs_plain(segs: &[Seg]) -> String {
    segs.iter().map(|s| s.plain()).collect()
}

/// 一页的排版结果。
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub strokes: Vec<Stroke>,
}

/// 整份文档的排版结果。
#[derive(Clone, Debug, Default)]
pub struct RenderedDoc {
    pub pages: Vec<Page>,
    /// 字体链与符号兜底都画不出的字符（去重、保持出现顺序）
    pub missing: Vec<char>,
    /// 每个表格的外框 `(x0, y_top, x1, y_bottom)`（页面坐标）
    pub tables: Vec<(f64, f64, f64, f64)>,
}

impl RenderedDoc {
    pub fn strokes(&self) -> Vec<Stroke> {
        self.pages
            .iter()
            .flat_map(|p| p.strokes.iter().cloned())
            .collect()
    }

    pub fn bbox(&self) -> BBox {
        BBox::from_points(
            self.pages
                .iter()
                .flat_map(|p| p.strokes.iter())
                .flat_map(|s| s.points.iter().copied()),
        )
    }
}

// ---------------------------------------------------------------------------
// Markdown → 块
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
enum Block {
    Heading(usize, Vec<Seg>),
    Paragraph(Vec<Seg>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Seg>>,
    },
    Quote(Vec<Seg>),
    Code(String),
    Hr,
    Table {
        header: Vec<Vec<Seg>>,
        rows: Vec<Vec<Vec<Seg>>>,
        aligns: Vec<Align>,
    },
    Image {
        alt: String,
        width: Option<f64>,
        height: Option<f64>,
    },
    Math(String),
}

#[derive(Default)]
struct Ctx {
    blocks: Vec<Block>,
    text: Vec<Seg>,
    text_buf: String,
    code: String,
    in_code: bool,
    heading: Option<usize>,
    in_quote: bool,
    quote_text: String,
    lists: Vec<(bool, u64, Vec<Vec<Seg>>)>,
    item: Option<Vec<Seg>>,
    table: Option<TableCtx>,
    image: Option<String>,
    image_attrs: Option<(Option<f64>, Option<f64>)>,
    pending_image: Option<Block>,
}

struct TableCtx {
    aligns: Vec<Align>,
    header: Vec<Vec<Seg>>,
    rows: Vec<Vec<Vec<Seg>>>,
    cur_row: Vec<Vec<Seg>>,
    cur_cell: Vec<Seg>,
    in_head: bool,
    in_cell: bool,
}

fn align_from_md(a: MdAlign) -> Align {
    match a {
        MdAlign::Center => Align::Center,
        MdAlign::Right => Align::Right,
        _ => Align::Left,
    }
}

impl Ctx {
    fn flush_text(&mut self) {
        if !self.text_buf.is_empty() {
            self.text
                .push(Seg::Text(std::mem::take(&mut self.text_buf)));
        }
    }

    fn take_inline(&mut self) -> Vec<Seg> {
        self.flush_text();
        std::mem::take(&mut self.text)
    }

    fn push_block(&mut self, b: Block) {
        // 列表/引用里的块并入对应容器
        if let Some(item) = self.item.as_mut() {
            if let Block::Paragraph(segs) = &b {
                if !item.is_empty() {
                    item.push(Seg::Text(" ".into()));
                }
                item.extend(segs.iter().cloned());
            }
            return;
        }
        if self.in_quote {
            if let Block::Paragraph(segs) = &b {
                if !self.quote_text.is_empty() {
                    self.quote_text.push(' ');
                }
                self.quote_text.push_str(&segs_plain(segs));
            }
            return;
        }
        self.blocks.push(b);
    }
}

/// 把 Markdown 源码解析为块序列。
fn parse_blocks(src: &str) -> Vec<Block> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_MATH);

    let parser = Parser::new_ext(src, opts);
    let mut cx = Ctx::default();

    for ev in parser {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    cx.heading = Some(match level {
                        pulldown_cmark::HeadingLevel::H1 => 1,
                        pulldown_cmark::HeadingLevel::H2 => 2,
                        pulldown_cmark::HeadingLevel::H3 => 3,
                        pulldown_cmark::HeadingLevel::H4 => 4,
                        pulldown_cmark::HeadingLevel::H5 => 5,
                        pulldown_cmark::HeadingLevel::H6 => 6,
                    });
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    cx.in_quote = true;
                    cx.quote_text.clear();
                }
                Tag::CodeBlock(_) => {
                    cx.in_code = true;
                    cx.code.clear();
                }
                Tag::List(start) => {
                    cx.lists
                        .push((start.is_some(), start.unwrap_or(1), Vec::new()));
                }
                Tag::Item => {
                    cx.item = Some(Vec::new());
                }
                Tag::Table(aligns) => {
                    cx.table = Some(TableCtx {
                        aligns: aligns.iter().map(|a| align_from_md(*a)).collect(),
                        header: Vec::new(),
                        rows: Vec::new(),
                        cur_row: Vec::new(),
                        cur_cell: Vec::new(),
                        in_head: false,
                        in_cell: false,
                    });
                }
                Tag::TableHead => {
                    if let Some(t) = cx.table.as_mut() {
                        t.in_head = true;
                        t.cur_row.clear();
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = cx.table.as_mut() {
                        t.cur_row.clear();
                    }
                }
                Tag::TableCell => {
                    if let Some(t) = cx.table.as_mut() {
                        t.in_cell = true;
                        t.cur_cell.clear();
                    }
                }
                Tag::Image { .. } => {
                    cx.image = Some(String::new());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    let level = cx.heading.unwrap_or(1);
                    let segs = cx.take_inline();
                    cx.push_block(Block::Heading(level, segs));
                    cx.heading = None;
                }
                TagEnd::Paragraph => {
                    if let Some(mut img) = cx.pending_image.take() {
                        if let Some((w, h)) = cx.image_attrs.take() {
                            if let Block::Image { width, height, .. } = &mut img {
                                *width = w;
                                *height = h;
                            }
                        }
                        cx.blocks.push(img);
                        cx.text.clear();
                        cx.text_buf.clear();
                    } else {
                        let segs = cx.take_inline();
                        if !segs.is_empty() {
                            cx.push_block(Block::Paragraph(segs));
                        }
                    }
                }
                TagEnd::BlockQuote(_) => {
                    cx.in_quote = false;
                    let text = cx.quote_text.trim().to_string();
                    cx.quote_text.clear();
                    if !text.is_empty() {
                        cx.push_block(Block::Quote(vec![Seg::Text(text)]));
                    }
                }
                TagEnd::CodeBlock => {
                    cx.in_code = false;
                    let code = std::mem::take(&mut cx.code);
                    cx.push_block(Block::Code(code));
                }
                TagEnd::List(_) => {
                    if let Some((ordered, start, items)) = cx.lists.pop() {
                        cx.push_block(Block::List {
                            ordered,
                            start,
                            items,
                        });
                    }
                }
                TagEnd::Item => {
                    if let Some(mut item) = cx.item.take() {
                        // 紧凑列表里文字直接落在 Item 上（没有 Paragraph 包裹）
                        let segs = cx.take_inline();
                        item.extend(segs);
                        if let Some(list) = cx.lists.last_mut() {
                            list.2.push(item);
                        }
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = cx.table.take() {
                        cx.blocks.push(Block::Table {
                            header: t.header,
                            rows: t.rows,
                            aligns: t.aligns,
                        });
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = cx.table.as_mut() {
                        t.in_head = false;
                        t.header = std::mem::take(&mut t.cur_row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = cx.table.as_mut() {
                        let row = std::mem::take(&mut t.cur_row);
                        if !row.is_empty() {
                            if t.in_head {
                                t.header = row;
                            } else {
                                t.rows.push(row);
                            }
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(t) = cx.table.as_mut() {
                        let cell = std::mem::take(&mut t.cur_cell);
                        t.cur_row.push(cell);
                        t.in_cell = false;
                    }
                }
                TagEnd::Image => {
                    let alt = cx.image.take().unwrap_or_default();
                    // 若整段只有这张图 → 块级图片占位
                    cx.flush_text();
                    if cx.text.is_empty() {
                        cx.pending_image = Some(Block::Image {
                            alt,
                            width: None,
                            height: None,
                        });
                    } else if !alt.is_empty() {
                        cx.text_buf.push_str(&alt);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if cx.in_code {
                    cx.code.push_str(&t);
                } else if let Some(img_alt) = cx.image.as_mut() {
                    img_alt.push_str(&t);
                } else if cx.table.as_ref().map(|t| t.in_cell).unwrap_or(false) {
                    // 单元格里可能是图片尺寸后缀
                    if let Some((w, h)) = parse_size_attrs(&t) {
                        cx.image_attrs = Some((w, h));
                    } else {
                        if let Some(tb) = cx.table.as_mut() {
                            tb.cur_cell.push(Seg::Text(t.to_string()));
                        }
                    }
                } else {
                    // 图片后的尺寸后缀 `{width=80,height=50}` 不当作正文
                    if cx.pending_image.is_some() {
                        if let Some((w, h)) = parse_size_attrs(&t) {
                            cx.image_attrs = Some((w, h));
                            continue;
                        }
                    }
                    cx.text_buf.push_str(&t);
                }
            }
            Event::Code(t) => {
                cx.text_buf.push_str(&t);
            }
            Event::InlineMath(t) => {
                cx.flush_text();
                if cx.table.as_ref().map(|tb| tb.in_cell).unwrap_or(false) {
                    if let Some(tb) = cx.table.as_mut() {
                        tb.cur_cell.push(Seg::Math(t.to_string()));
                    }
                } else {
                    cx.text.push(Seg::Math(t.to_string()));
                }
            }
            Event::DisplayMath(t) => {
                cx.blocks.push(Block::Math(t.to_string()));
            }
            Event::SoftBreak => {
                if cx.in_code {
                    cx.code.push('\n');
                } else {
                    cx.text_buf.push(' ');
                }
            }
            Event::HardBreak => {
                cx.text_buf.push(' ');
            }
            Event::Rule => cx.blocks.push(Block::Hr),
            Event::Html(t) | Event::InlineHtml(t) => {
                // 简单 HTML 里可能藏图片尺寸，其余忽略
                if let Some((w, h)) = parse_size_attrs(&t) {
                    cx.image_attrs = Some((w, h));
                }
            }
            Event::TaskListMarker(checked) => {
                cx.text_buf.push_str(if checked { "☑ " } else { "☐ " });
            }
            _ => {}
        }
    }

    // 收尾：未被容器接住的图片
    if let Some(img) = cx.pending_image.take() {
        let mut img = img;
        if let Some((w, h)) = cx.image_attrs.take() {
            if let Block::Image { width, height, .. } = &mut img {
                *width = w;
                *height = h;
            }
        }
        cx.blocks.push(img);
    }
    cx.blocks
}

/// 解析 `{width=80,height=50}` / `{width=80mm}`。
fn parse_size_attrs(text: &str) -> Option<(Option<f64>, Option<f64>)> {
    let t = text.trim();
    let inner = t.strip_prefix('{')?.strip_suffix('}')?;
    if !inner.contains('=') {
        return None;
    }
    let mut w = None;
    let mut h = None;
    for part in inner.split([',', ' ']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=')?;
        let v = v.trim().trim_end_matches("mm").trim();
        let val: f64 = v.parse().ok()?;
        match k.trim() {
            "width" | "w" => w = Some(val),
            "height" | "h" => h = Some(val),
            _ => {}
        }
    }
    if w.is_none() && h.is_none() {
        None
    } else {
        Some((w, h))
    }
}

// ---------------------------------------------------------------------------
// 随机抖动（表格手绘感）
// ---------------------------------------------------------------------------
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * u
    }
    /// Box–Muller 正态分布
    fn gauss(&mut self, sigma: f64) -> f64 {
        let u1 = ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64).max(1e-12);
        let u2 = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        z * sigma
    }
}

// ---------------------------------------------------------------------------
// 渲染器
// ---------------------------------------------------------------------------
struct Renderer<'a> {
    fonts: &'a [FontFamily],
    symbols: &'a SymbolFonts,
    style: &'a MarkdownStyle,
    page: PageSpec,
    pages: Vec<Vec<Stroke>>,
    y: f64,
    max_x: f64,
    missing: Vec<char>,
    missing_seen: HashSet<char>,
    tables: Vec<(f64, f64, f64, f64)>,
    table_n: usize,
    wrap_width: f64,
    sim: Option<crate::sim::Simulator>,
}

impl<'a> Renderer<'a> {
    fn new(
        fonts: &'a [FontFamily],
        symbols: &'a SymbolFonts,
        style: &'a MarkdownStyle,
        page: PageSpec,
    ) -> Self {
        let wrap_width = if style.wrap_width > 0.0 {
            style.wrap_width
        } else {
            page.content_width()
        };
        Self {
            fonts,
            symbols,
            style,
            page,
            pages: vec![Vec::new()],
            y: page.height - page.margin_top,
            max_x: 0.0,
            missing: Vec::new(),
            missing_seen: HashSet::new(),
            tables: Vec::new(),
            table_n: 0,
            wrap_width,
            sim: if style.jitter.is_active() {
                Some(crate::sim::Simulator::new(&style.jitter))
            } else {
                None
            },
        }
    }

    fn left(&self) -> f64 {
        self.page.margin_left
    }
    fn bottom(&self) -> f64 {
        self.page.margin_bottom
    }
    fn top(&self) -> f64 {
        self.page.height - self.page.margin_top
    }

    fn lh(&self, size: f64) -> f64 {
        match self.fonts.first() {
            Some(f) => f.line_height(size, self.style.line_spacing),
            None => size * self.style.line_spacing,
        }
    }

    fn text_style(&self, size: f64) -> TextStyle {
        TextStyle {
            size,
            line_spacing: self.style.line_spacing,
            char_spacing: self.style.char_spacing,
            align: Align::Left,
        }
    }

    fn cur(&mut self) -> &mut Vec<Stroke> {
        self.pages.last_mut().expect("至少一页")
    }

    fn page_has_content(&self) -> bool {
        self.pages.last().map(|p| !p.is_empty()).unwrap_or(false)
    }

    /// 换页：内容放不下时调用。
    fn ensure(&mut self, need: f64) {
        if self.y - need < self.bottom() - 1e-9 && self.page_has_content() {
            self.pages.push(Vec::new());
            self.y = self.top();
        }
    }

    /// 开始一行（重抽行级扰动参数）。width 用于摊开行尾落差。
    fn begin_sim_line(&mut self, width: f64) {
        if let Some(sim) = self.sim.as_mut() {
            sim.begin_line(width);
        }
    }

    fn note_missing(&mut self, ch: char) {
        if self.missing_seen.insert(ch) {
            self.missing.push(ch);
        }
    }

    fn note_missing_math(&mut self, chars: Vec<char>) {
        for ch in chars {
            self.note_missing(ch);
        }
    }

    /// 在给定基线处画一行片段，返回宽度（mm）。
    fn draw_segments(
        &mut self,
        segs: &[Seg],
        size: f64,
        x: f64,
        baseline: f64,
        align: Align,
        avail: f64,
    ) -> f64 {
        let ts = self.text_style(size);
        // 先量总宽（用于对齐）
        let mut total = 0.0;
        for seg in segs {
            total += match seg {
                Seg::Text(t) => measure_text(t, self.fonts, &ts),
                Seg::Math(m) => {
                    let b = math::measure_box(
                        m,
                        size * self.style.math_size_scale,
                        self.fonts,
                        self.symbols,
                    );
                    if b.is_empty() {
                        measure_text(&format!("${m}$"), self.fonts, &ts)
                    } else {
                        b.width() + size * 0.2
                    }
                }
            };
        }
        let dx = match align {
            Align::Center => (avail - total) / 2.0,
            Align::Right => avail - total,
            Align::Left => 0.0,
        }
        .max(0.0);
        let mut x = x + dx;
        let start_x = x;
        for seg in segs {
            match seg {
                Seg::Text(t) => {
                    let mut strokes = Vec::new();
                    let w = layout::append_text(
                        &mut strokes,
                        t,
                        self.fonts,
                        &ts,
                        x,
                        baseline,
                        &mut self.missing,
                        &mut self.missing_seen,
                        self.sim.as_mut(),
                    );
                    self.cur().extend(strokes);
                    x += w;
                }
                Seg::Math(m) => {
                    let strokes = math::render(
                        m,
                        size * self.style.math_size_scale,
                        self.fonts,
                        self.symbols,
                    );
                    let box_ =
                        BBox::from_points(strokes.iter().flat_map(|s| s.points.iter().copied()));
                    for mut s in strokes {
                        for p in &mut s.points {
                            p.0 += x;
                            p.1 += baseline;
                        }
                        self.cur().push(s);
                    }
                    x += box_.width() + size * 0.2;
                }
            }
        }
        let w = x - start_x;
        self.max_x = self.max_x.max(x);
        w
    }

    /// 输出一行文字（不折行），并按行高推进光标。
    fn emit_line(&mut self, segs: &[Seg], size: f64, indent: f64, align: Align) {
        let lh = self.lh(size);
        self.ensure(lh);
        // 手写扰动：每行重新抽行级参数（行基线 / 行首偏移 / 行尾落差）
        let line_w = self.segment_width(segs, size);
        self.begin_sim_line(line_w);
        let baseline = self.y - size;
        let x = self.left() + indent;
        let avail = self.wrap_width - indent;
        self.draw_segments(segs, size, x, baseline, align, avail);
        self.y = baseline - (lh - size);
    }

    fn emit_text_line(&mut self, text: &str, size: f64, indent: f64, align: Align) {
        let segs = vec![Seg::Text(text.to_string())];
        self.emit_line(&segs, size, indent, align);
    }

    // -- 各块 ---------------------------------------------------------------
    fn heading(&mut self, level: usize, segs: &[Seg]) {
        let scale = self.style.heading_scale[(level.clamp(1, 6)) - 1];
        let size = self.style.size * scale;
        self.emit_line(segs, size, 0.0, Align::Left);
        self.y -= self.style.size * 0.6;
    }

    fn paragraph(&mut self, segs: &[Seg], indent: f64) {
        let size = self.style.size;
        let avail = (self.wrap_width - indent).max(size * 0.5);
        for line in self.wrap_segments(segs, size, avail) {
            self.emit_line(&line, size, indent, Align::Left);
        }
        self.y -= size * 0.8;
    }

    /// 「文字 + 行内公式」折行：公式整块不可拆。
    fn wrap_segments(&mut self, segs: &[Seg], size: f64, avail: f64) -> Vec<Vec<Seg>> {
        let ts = self.text_style(size);
        let mut lines: Vec<Vec<Seg>> = Vec::new();
        let mut cur: Vec<Seg> = Vec::new();
        let mut cur_w = 0.0;

        for seg in segs {
            match seg {
                Seg::Math(m) => {
                    let b = math::measure_box(
                        m,
                        size * self.style.math_size_scale,
                        self.fonts,
                        self.symbols,
                    );
                    let w = if b.is_empty() {
                        measure_text(&format!("${m}$"), self.fonts, &ts) + size * 0.2
                    } else {
                        b.width() + size * 0.2
                    };
                    if !cur.is_empty() && cur_w + w > avail {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0.0;
                    }
                    cur.push(Seg::Math(m.clone()));
                    cur_w += w;
                }
                Seg::Text(t) => {
                    let mut rest = t.clone();
                    while !rest.is_empty() {
                        let room = (avail - cur_w).max(size * 0.5);
                        if !cur.is_empty() && avail - cur_w <= size * 0.6 {
                            lines.push(std::mem::take(&mut cur));
                            cur_w = 0.0;
                            continue;
                        }
                        let parts = layout::wrap_text(&rest, self.fonts, &ts, room);
                        let first = parts.first().cloned().unwrap_or_default();
                        cur_w += measure_text(&first, self.fonts, &ts);
                        cur.push(Seg::Text(first.clone()));
                        let consumed = first.chars().count();
                        let rest_chars: Vec<char> = rest.chars().collect();
                        rest = rest_chars[consumed.min(rest_chars.len())..]
                            .iter()
                            .collect::<String>();
                        rest = rest.trim_start().to_string();
                        if !rest.is_empty() {
                            lines.push(std::mem::take(&mut cur));
                            cur_w = 0.0;
                        }
                    }
                }
            }
        }
        if !cur.is_empty() || lines.is_empty() {
            lines.push(cur);
        }
        lines
    }

    fn bullet_list(&mut self, items: &[Vec<Seg>], ordered: bool, start: u64) {
        let size = self.style.size;
        let ind = self.style.list_indent;
        for (i, item) in items.iter().enumerate() {
            let marker = if ordered {
                format!("{}.", start + i as u64)
            } else {
                "•".to_string()
            };
            let lh = self.lh(size);
            self.ensure(lh);
            let baseline = self.y - size;
            let ts = self.text_style(size);
            let mut strokes = Vec::new();
            let marker_w = layout::append_text(
                &mut strokes,
                &marker,
                self.fonts,
                &ts,
                self.left(),
                baseline,
                &mut self.missing,
                &mut self.missing_seen,
                self.sim.as_mut(),
            );
            self.cur().extend(strokes);
            let avail = (self.wrap_width - ind).max(size * 0.5);
            let lines = self.wrap_segments(item, size, avail);
            for (k, line) in lines.iter().enumerate() {
                let y = baseline - k as f64 * lh;
                if k > 0 {
                    self.ensure(lh);
                }
                let line_w = self.segment_width(line, size);
                self.begin_sim_line(line_w);
                self.draw_segments(line, size, self.left() + ind, y, Align::Left, avail);
            }
            let consumed = lines.len().max(1) as f64;
            self.y = baseline - (lh - size) - (consumed - 1.0) * lh;
            let _ = marker_w;
        }
        self.y -= size * 0.4;
    }

    fn quote(&mut self, segs: &[Seg]) {
        let size = self.style.size;
        let ind = self.style.quote_indent;
        let top = self.y;
        let avail = (self.wrap_width - ind).max(size * 0.5);
        for line in self.wrap_segments(segs, size, avail) {
            self.emit_line(&line, size, ind, Align::Left);
        }
        let bottom = self.y;
        // 左侧竖线（页面坐标）
        let x = self.left() + ind * 0.4;
        self.cur().push(Stroke::new(
            vec![(x, top - size * 0.4), (x, bottom + size * 0.4)],
            false,
            Role::Structure,
        ));
    }

    fn code_block(&mut self, code: &str) {
        let size = self.style.size * 0.92;
        for line in code.trim_end_matches('\n').split('\n') {
            self.emit_text_line(line, size, 0.0, Align::Left);
        }
        self.y -= size * 0.6;
    }

    fn hr(&mut self) {
        let size = self.style.size;
        let y = self.y - size * 0.5;
        let left = self.left();
        let right = left + self.wrap_width;
        self.ensure(0.0);
        self.cur().push(Stroke::new(
            vec![(left, y), (right, y)],
            false,
            Role::Structure,
        ));
        self.y = y - size * 0.5;
    }

    fn image(&mut self, _alt: &str, w: Option<f64>, h: Option<f64>) {
        let width = w.unwrap_or(self.wrap_width);
        let height = h.unwrap_or(width * 2.0 / 3.0);
        self.ensure(height);
        // 位置留空：不画内容，只把光标往下推
        self.y -= height;
        self.y -= self.style.size * 0.8;
    }

    fn math_block(&mut self, latex: &str) {
        let size = self.style.size * self.style.math_size_scale;
        let strokes = math::render(latex, size, self.fonts, self.symbols);
        if strokes.is_empty() {
            self.emit_text_line(&format!("$${latex}$$"), self.style.size, 0.0, Align::Left);
            return;
        }
        let b = BBox::from_points(strokes.iter().flat_map(|s| s.points.iter().copied()));
        let h = b.height() + self.style.size * 0.4;
        self.ensure(h);
        let baseline = self.y - b.y1;
        let width = b.width();
        let x = self.left() + (self.wrap_width - width) / 2.0;
        for mut s in strokes {
            for p in &mut s.points {
                p.0 += x;
                p.1 += baseline;
            }
            self.cur().push(s);
        }
        self.y = baseline - b.y0 - self.style.size * 0.8;
        self.note_missing_math(math::missing_chars(latex, size, self.fonts, self.symbols));
    }

    // -- 表格 ---------------------------------------------------------------
    fn table(&mut self, header: &[Vec<Seg>], rows: &[Vec<Vec<Seg>>], aligns: &[Align]) {
        let size = self.style.size;
        let ncol = rows
            .iter()
            .map(|r| r.len())
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0);
        if ncol == 0 {
            return;
        }
        let mut all_rows: Vec<Vec<String>> = Vec::with_capacity(rows.len() + 1);
        let mut header_txt: Vec<String> = header.iter().map(|c| segs_plain(c)).collect();
        header_txt.resize(ncol, String::new());
        all_rows.push(header_txt);
        for r in rows {
            let mut row: Vec<String> = r.iter().map(|c| segs_plain(c)).collect();
            row.resize(ncol, String::new());
            all_rows.push(row);
        }
        let nrow = all_rows.len();

        let seed = self
            .style
            .table_seed
            .wrapping_mul(1_000_003)
            .wrapping_add(self.table_n as u64 * 7919);
        self.table_n += 1;
        let mut rng = Rng::new(seed);

        // 1) 合并单元格 → 矩形合并区
        let mut reg_of: Vec<Vec<i32>> = vec![vec![-1; ncol]; nrow];
        let mut regions: Vec<Region> = Vec::new();
        for r in 0..nrow {
            for c in 0..ncol {
                let raw = all_rows[r][c].trim().to_string();
                if raw == "<<" && c > 0 {
                    let p = reg_of[r][c - 1];
                    if p >= 0 {
                        let pi = p as usize;
                        if regions[pi].r1 == r && regions[pi].c1 == c - 1 {
                            regions[pi].c1 = c;
                            reg_of[r][c] = p;
                            continue;
                        }
                    }
                } else if raw == "^^" && r > 0 {
                    let p = reg_of[r - 1][c];
                    if p >= 0 {
                        let pi = p as usize;
                        if regions[pi].r1 == r - 1 {
                            regions[pi].r1 = r;
                            reg_of[r][c] = p;
                            continue;
                        }
                    }
                }
                reg_of[r][c] = regions.len() as i32;
                regions.push(Region {
                    r0: r,
                    r1: r,
                    c0: c,
                    c1: c,
                    text: raw,
                    need: 0.0,
                });
            }
        }

        let ts = self.text_style(size);
        // 2) 自然列宽 / 最小列宽
        let mut natural = vec![0.0f64; ncol];
        let mut mins = vec![0.0f64; ncol];
        for reg in &regions {
            let w = self.segment_width(&reg.segs(), size);
            let txt = reg.text.clone();
            reg_need(&mut natural, &mut mins, reg, w, &txt, self.fonts, &ts, size);
        }
        // 跨列区：不足的宽度摊到各列（两轮）
        for _ in 0..2 {
            for reg in &regions {
                if reg.c0 == reg.c1 {
                    continue;
                }
                let span = (reg.c1 - reg.c0 + 1) as f64;
                let avail: f64 = natural[reg.c0..=reg.c1].iter().sum();
                if avail < reg.need {
                    let add = (reg.need - avail) / span;
                    for c in reg.c0..=reg.c1 {
                        natural[c] += add;
                    }
                }
            }
        }
        let explicit = self.style.table_width > 0.0;
        let max_total = if explicit {
            self.style.table_width
        } else {
            self.wrap_width
        };
        let col_widths = fit_widths(&natural, max_total, &mins, explicit);
        let total_w: f64 = col_widths.iter().sum();
        let mut col_x = Vec::with_capacity(ncol);
        let mut acc = 0.0;
        for w in &col_widths {
            col_x.push(acc);
            acc += w;
        }
        let mut col_widths = col_widths;

        // 3) 列界抖动
        if self.style.table_col_jitter > 0.0 && ncol > 1 {
            let cj = self.style.table_col_jitter;
            for i in 1..ncol {
                col_x[i] += rng.gauss(cj);
                if col_x[i] < col_x[i - 1] + mins[i - 1] * 0.5 {
                    col_x[i] = col_x[i - 1] + mins[i - 1] * 0.5;
                }
            }
            if total_w - col_x[ncol - 1] < mins[ncol - 1] * 0.5 {
                col_x[ncol - 1] = total_w - mins[ncol - 1] * 0.5;
            }
            col_widths = (0..ncol)
                .map(|i| {
                    let right = if i + 1 < ncol { col_x[i + 1] } else { total_w };
                    right - col_x[i]
                })
                .collect();
        }

        // 4) 单元格折行 → 行高
        let lh = self.lh(size);
        let mut cell_lines: Vec<Vec<Vec<Seg>>> = vec![Vec::new(); regions.len()];
        let mut row_heights = vec![0.0f64; nrow];
        let mut spans: Vec<(usize, f64)> = Vec::new();
        for (p, reg) in regions.iter().enumerate() {
            let inner =
                (col_widths[reg.c0..=reg.c1].iter().sum::<f64>() - 2.0 * CELL_PAD_MM - size * 0.2)
                    .max(1.0);
            let segs = reg.segs();
            let lines = self.wrap_segments(&segs, size, inner);
            let mut tall: f64 = 0.0;
            for ln in &lines {
                for seg in ln {
                    if let Seg::Math(m) = seg {
                        let b = math::measure_box(
                            m,
                            size * self.style.math_size_scale,
                            self.fonts,
                            self.symbols,
                        );
                        if !b.is_empty() {
                            tall = tall.max(b.height() + size * 0.3);
                        }
                    }
                }
            }
            let need = (lines.len() as f64 * lh).max(tall);
            cell_lines[p] = lines;
            if reg.r0 == reg.r1 {
                row_heights[reg.r0] = row_heights[reg.r0].max(need);
            } else {
                spans.push((p, need));
            }
        }
        for (p, need) in &spans {
            let reg = &regions[*p];
            let have: f64 = row_heights[reg.r0..=reg.r1].iter().sum();
            if have < *need {
                let add = (*need - have) / (reg.r1 - reg.r0 + 1) as f64;
                for r in reg.r0..=reg.r1 {
                    row_heights[r] += add;
                }
            }
        }

        // 5) 行界、表格高度、是否要换页
        let mut ys: Vec<f64> = Vec::with_capacity(nrow + 1);
        let top = self.y;
        ys.push(top);
        for h in &row_heights {
            let last = *ys.last().unwrap();
            ys.push(last - h);
        }
        if self.style.table_row_jitter > 0.0 && ys.len() > 2 {
            let rj = self.style.table_row_jitter;
            let min_h = size * 0.8;
            for i in 1..ys.len() - 1 {
                ys[i] -= rng.gauss(rj);
                if ys[i] > ys[i - 1] - min_h {
                    ys[i] = ys[i - 1] - min_h;
                }
            }
            if ys[ys.len() - 2] < ys[ys.len() - 1] + min_h {
                let n = ys.len();
                ys[n - 2] = ys[n - 1] + min_h;
            }
            for r in 0..nrow {
                row_heights[r] = ys[r] - ys[r + 1];
            }
        }
        for (p, need) in &spans {
            let reg = &regions[*p];
            let have = ys[reg.r0] - ys[reg.r1 + 1];
            if have < *need {
                let delta = *need - have;
                for i in reg.r1 + 1..ys.len() {
                    ys[i] -= delta;
                }
            }
        }
        let table_h = ys[0] - ys[ys.len() - 1];
        self.ensure(table_h + size * 0.6);
        // ensure 后光标可能变了，重新计算纵坐标
        let top = self.y;
        let delta = top - ys[0];
        for y in &mut ys {
            *y += delta;
        }
        let left = self.left();
        let bottom = ys[ys.len() - 1];

        // 6) 表格线（含线端抖动）
        let ej = self.style.table_end_jitter.max(0.0);
        let osv = self.style.table_overshoot.max(0.0);
        let jline = |a: (f64, f64), b: (f64, f64), rng: &mut Rng| -> [(f64, f64); 2] {
            if ej <= 0.0 && osv <= 0.0 {
                return [a, b];
            }
            let horizontal = (a.1 - b.1).abs() < 1e-9;
            let (lo, hi) = if (horizontal && a.0 > b.0) || (!horizontal && a.1 < b.1) {
                (b, a)
            } else {
                (a, b)
            };
            let mut out = [lo, hi];
            for (k, p) in out.iter_mut().enumerate() {
                let o = if osv > 0.0 {
                    rng.uniform(0.0, osv)
                } else {
                    0.0
                };
                let o = if k == 0 { -o } else { o };
                let along = if ej > 0.0 { rng.gauss(ej) } else { 0.0 };
                let perp = if ej > 0.0 { rng.gauss(ej) } else { 0.0 };
                if horizontal {
                    p.0 += along + o;
                    p.1 += perp;
                } else {
                    p.0 += perp;
                    p.1 += along + o;
                }
            }
            out
        };

        let span_x = if total_w > 1e-9 { total_w } else { 1.0 };
        let span_y = if ys[0] > ys[ys.len() - 1] {
            ys[0] - ys[ys.len() - 1]
        } else {
            1.0
        };
        let mut hsegs: std::collections::HashMap<(usize, usize), ((f64, f64), (f64, f64))> =
            std::collections::HashMap::new();

        let clip = |a: (f64, f64), b: (f64, f64), t0: f64, t1: f64| -> [(f64, f64); 2] {
            [
                (a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0),
                (a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1),
            ]
        };

        // 横线
        for (i, yy) in ys.iter().enumerate() {
            let runs: Vec<(usize, usize)> = if i == 0 || i == nrow {
                vec![(0, ncol - 1)]
            } else {
                let mut runs = Vec::new();
                let mut cs: Option<usize> = None;
                for c in 0..ncol {
                    if reg_of[i - 1][c] != reg_of[i][c] {
                        if cs.is_none() {
                            cs = Some(c);
                        }
                    } else if let Some(s) = cs.take() {
                        runs.push((s, c - 1));
                    }
                }
                if let Some(s) = cs {
                    runs.push((s, ncol - 1));
                }
                runs
            };
            if runs.is_empty() {
                continue;
            }
            let line = jline((left, *yy), (left + total_w, *yy), &mut rng);
            for (cs, ce) in runs {
                let x0 = left + col_x[cs];
                let x1 = if ce + 1 < ncol {
                    left + col_x[ce + 1]
                } else {
                    left + total_w
                };
                let seg = clip(line[0], line[1], (x0 - left) / span_x, (x1 - left) / span_x);
                self.cur()
                    .push(Stroke::new(vec![seg[0], seg[1]], false, Role::TableLine));
                for c in cs..=ce {
                    hsegs.insert((i, c), (seg[0], seg[1]));
                }
            }
        }

        // 竖线
        for j in 0..=ncol {
            let runs: Vec<(usize, usize)> = if j == 0 || j == ncol {
                vec![(0, nrow - 1)]
            } else {
                let mut runs = Vec::new();
                let mut rs: Option<usize> = None;
                for r in 0..nrow {
                    if reg_of[r][j - 1] != reg_of[r][j] {
                        if rs.is_none() {
                            rs = Some(r);
                        }
                    } else if let Some(s) = rs.take() {
                        runs.push((s, r - 1));
                    }
                }
                if let Some(s) = rs {
                    runs.push((s, nrow - 1));
                }
                runs
            };
            if runs.is_empty() {
                continue;
            }
            let x = if j < ncol {
                left + col_x[j]
            } else {
                left + total_w
            };
            let line = jline((x, ys[0]), (x, ys[ys.len() - 1]), &mut rng);
            for (rs, re) in runs {
                let seg = clip(
                    line[0],
                    line[1],
                    (ys[0] - ys[rs]) / span_y,
                    (ys[0] - ys[re + 1]) / span_y,
                );
                self.cur()
                    .push(Stroke::new(vec![seg[0], seg[1]], false, Role::TableLine));
            }
        }

        // 7) 单元格文字：贴着下界线写，多行自底向上叠
        for (p, reg) in regions.iter().enumerate() {
            let c0 = reg.c0;
            if c0 >= col_x.len() {
                continue;
            }
            let align = aligns.get(c0).copied().unwrap_or(Align::Left);
            let cw: f64 = col_widths[c0..=reg.c1].iter().sum();
            let seg_r = hsegs.get(&(reg.r1 + 1, c0)).copied();
            let _seg_y0 = ys[reg.r1 + 1];
            let lines = cell_lines[p].clone();
            let last_base = ys[reg.r1 + 1] + size * 0.14 + (lines.len().max(1) - 1) as f64 * lh;
            for (k, line) in lines.iter().enumerate() {
                let mut base = last_base - k as f64 * lh;
                // 行内公式按墨迹在格内垂直居中
                if let Some(Seg::Math(m)) = line.iter().find(|s| matches!(s, Seg::Math(_))) {
                    let b = math::measure_box(
                        m,
                        size * self.style.math_size_scale,
                        self.fonts,
                        self.symbols,
                    );
                    if !b.is_empty() {
                        let row_h = ys[reg.r0] - ys[reg.r1 + 1];
                        base = ys[reg.r1 + 1] + (row_h - b.height()) / 2.0 - b.y0;
                    }
                }
                let tw = self.segment_width(line, size);
                let tx = match align {
                    Align::Right => left + col_x[c0] + cw - CELL_PAD_MM - tw,
                    Align::Center => left + col_x[c0] + (cw - tw) / 2.0,
                    Align::Left => left + col_x[c0] + CELL_PAD_MM,
                };
                let mut xx = tx;
                for seg in line {
                    match seg {
                        Seg::Text(t) => {
                            let mut strokes = Vec::new();
                            let w = layout::append_text(
                                &mut strokes,
                                t,
                                self.fonts,
                                &ts,
                                xx,
                                base,
                                &mut self.missing,
                                &mut self.missing_seen,
                                None, // 表格内不做手写抖动，保证对齐
                            );
                            self.cur().extend(strokes);
                            xx += w;
                        }
                        Seg::Math(m) => {
                            let strokes = math::render(
                                m,
                                size * self.style.math_size_scale,
                                self.fonts,
                                self.symbols,
                            );
                            let b = BBox::from_points(
                                strokes.iter().flat_map(|s| s.points.iter().copied()),
                            );
                            for mut s in strokes {
                                for pt in &mut s.points {
                                    pt.0 += xx;
                                    pt.1 += base;
                                }
                                self.cur().push(s);
                            }
                            xx += b.width() + size * 0.2;
                        }
                    }
                }
                let _ = seg_r;
            }
        }

        self.y = bottom - size * 0.6;
        self.max_x = self.max_x.max(left + total_w);
        self.tables.push((left, top, left + total_w, bottom));
    }

    fn segment_width(&mut self, segs: &[Seg], size: f64) -> f64 {
        let ts = self.text_style(size);
        let mut w = 0.0;
        for seg in segs {
            match seg {
                Seg::Text(t) => w += measure_text(t, self.fonts, &ts),
                Seg::Math(m) => {
                    let b = math::measure_box(
                        m,
                        size * self.style.math_size_scale,
                        self.fonts,
                        self.symbols,
                    );
                    w += if b.is_empty() {
                        measure_text(&format!("${m}$"), self.fonts, &ts)
                    } else {
                        b.width() + size * 0.2
                    };
                }
            }
        }
        w
    }
}

struct Region {
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
    text: String,
    need: f64,
}

impl Region {
    fn segs(&self) -> Vec<Seg> {
        vec![Seg::Text(self.text.clone())]
    }
}

fn reg_need(
    natural: &mut [f64],
    mins: &mut [f64],
    reg: &Region,
    w: f64,
    txt: &str,
    fonts: &[FontFamily],
    ts: &TextStyle,
    size: f64,
) {
    // 与实际绘制时的内宽保持一致：扣除 2×pading 与 0.2em 安全余量后仍能放下一行
    let need = w + 2.0 * CELL_PAD_MM + size * 0.2;
    let minw = widest_char_width(txt, fonts, ts).max(size * 0.6) + 2.0 * CELL_PAD_MM + size * 0.2;
    if reg.c0 == reg.c1 {
        natural[reg.c0] = natural[reg.c0].max(need);
        mins[reg.c0] = mins[reg.c0].max(minw);
    }
}

/// 把列宽调整到总宽（`_fit_widths` 的移植）。
fn fit_widths(natural: &[f64], max_total: f64, mins: &[f64], exact: bool) -> Vec<f64> {
    let total: f64 = natural.iter().sum();
    if max_total <= 0.0 {
        return natural.to_vec();
    }
    if (!exact && total <= max_total) || (total - max_total).abs() < 1e-9 {
        return natural.to_vec();
    }
    let mins_total: f64 = mins.iter().sum();
    if mins_total >= max_total {
        return mins.to_vec();
    }
    if total < max_total {
        if total <= 0.0 {
            return vec![max_total / natural.len().max(1) as f64; natural.len()];
        }
        return natural.iter().map(|n| n * max_total / total).collect();
    }
    let avail = max_total - mins_total;
    let slack: Vec<f64> = natural
        .iter()
        .zip(mins.iter())
        .map(|(n, m)| (n - m).max(0.0))
        .collect();
    let slack_total: f64 = slack.iter().sum();
    if slack_total <= 0.0 {
        return mins.to_vec();
    }
    mins.iter()
        .zip(slack.iter())
        .map(|(m, s)| m + s * avail / slack_total)
        .collect()
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------
/// 把 Markdown 渲染为分页笔画。
pub fn render_markdown(
    src: &str,
    fonts: &[FontFamily],
    symbols: &SymbolFonts,
    style: &MarkdownStyle,
    page: PageSpec,
) -> RenderedDoc {
    let blocks = parse_blocks(src);
    let mut r = Renderer::new(fonts, symbols, style, page);
    for b in &blocks {
        match b {
            Block::Heading(level, segs) => r.heading(*level, segs),
            Block::Paragraph(segs) => r.paragraph(segs, 0.0),
            Block::List {
                ordered,
                start,
                items,
            } => r.bullet_list(items, *ordered, *start),
            Block::Quote(segs) => r.quote(segs),
            Block::Code(code) => r.code_block(code),
            Block::Hr => r.hr(),
            Block::Table {
                header,
                rows,
                aligns,
            } => r.table(header, rows, aligns),
            Block::Image { alt, width, height } => r.image(alt, *width, *height),
            Block::Math(latex) => r.math_block(latex),
        }
    }
    // 去掉尾部空白页
    while r.pages.len() > 1 && r.pages.last().map(|p| p.is_empty()).unwrap_or(false) {
        r.pages.pop();
    }
    // 手写扰动：结构线（表格线/分割线/引用竖线）只加沿线起伏，不做整体位移
    if style.jitter.is_active() {
        for strokes in &mut r.pages {
            crate::sim::wobble_structural_strokes(strokes, &style.jitter);
        }
    }
    RenderedDoc {
        pages: r
            .pages
            .into_iter()
            .map(|strokes| Page { strokes })
            .collect(),
        missing: r.missing,
        tables: r.tables,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontRegistry;

    fn setup() -> (Vec<FontFamily>, SymbolFonts) {
        let mut reg = FontRegistry::builtin();
        let fonts = vec![reg.get("futural").unwrap().clone()];
        (fonts, SymbolFonts::load())
    }

    #[test]
    fn renders_heading_and_paragraph() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let doc = render_markdown("# Title\n\nHello world.", &f, &s, &st, PageSpec::a4());
        assert_eq!(doc.pages.len(), 1);
        assert!(!doc.strokes().is_empty());
    }

    #[test]
    fn wraps_long_paragraph() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let text = "word ".repeat(200);
        let doc = render_markdown(&text, &f, &s, &st, PageSpec::a4());
        let h = doc.bbox().height();
        assert!(h > 20.0, "应折成多行，实际高度 {h}");
    }

    #[test]
    fn paginates_when_page_is_full() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let text = "line of text\n\n".repeat(200);
        let doc = render_markdown(&text, &f, &s, &st, PageSpec::a4());
        assert!(doc.pages.len() > 1, "应分成多页，实际 {}", doc.pages.len());
        // 每页内容都不越出下边距
        let page = PageSpec::a4();
        for p in &doc.pages {
            for st in &p.strokes {
                for pt in &st.points {
                    assert!(
                        pt.1 >= page.margin_bottom - 1e-6,
                        "内容越出下边距：{}",
                        pt.1
                    );
                }
            }
        }
    }

    #[test]
    fn renders_table() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let doc = render_markdown(md, &f, &s, &st, PageSpec::a4());
        assert_eq!(doc.tables.len(), 1);
        assert!(!doc.strokes().is_empty());
    }

    #[test]
    fn image_reserves_space() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let doc = render_markdown(
            "before\n\n![pic](a.png){width=80,height=50}\n\nafter",
            &f,
            &s,
            &st,
            PageSpec::a4(),
        );
        // 空出 50mm 高度：前后文字距离应大于 50mm
        let h = doc.bbox().height();
        assert!(h > 55.0, "图片占位高度不足：{h}");
    }

    #[test]
    fn inline_math_renders() {
        let (f, s) = setup();
        let st = MarkdownStyle::default();
        let doc = render_markdown("energy $E=mc^2$ here", &f, &s, &st, PageSpec::a4());
        assert!(!doc.strokes().is_empty());
    }
}

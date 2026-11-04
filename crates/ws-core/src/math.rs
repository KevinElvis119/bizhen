//! LaTeX 公式排版（替代 matplotlib mathtext）。
//!
//! 原版把「排版」交给 matplotlib mathtext，只借它的字形位置与结构矩形；
//! 这里自己实现同一层：**解析 → 盒子模型排版 → 单线光栅化**。
//!
//! 支持子集（用户确认的范围）：上下标、`\frac`、`\sqrt`、`\sum`/`\prod`/`\int`
//! 的上下限、`\left(...\right)`、希腊字母、常用关系/运算符、`\text{}`。
//! 解析失败时返回空，由调用方退回显示原文（公式不丢）。

use std::collections::HashSet;

use crate::font::FontFamily;
use crate::layout::{layout_text, measure_text, TextStyle};
use crate::stroke::Stroke;
use crate::symbols::{self, SymbolFonts};

// ---------------------------------------------------------------------------
// 词法
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Char(char),
    Cmd(String),
    LBrace,
    RBrace,
    Sup,
    Sub,
    /// `\!` / `\,` / `\;` / `\quad` 之类的水平间距（em）
    Space(f64),
    LBracket,
    RBracket,
}

fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                if i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic() {
                    let start = i + 1;
                    let mut j = start;
                    while j < chars.len() && chars[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    out.push(Tok::Cmd(chars[start..j].iter().collect()));
                    i = j;
                } else if i + 1 < chars.len() {
                    // 转义字符：\, \; \! \\ \{ \} 等
                    let n = chars[i + 1];
                    match n {
                        ',' => out.push(Tok::Space(0.17)),
                        ';' => out.push(Tok::Space(0.28)),
                        ':' => out.push(Tok::Space(0.22)),
                        '!' => out.push(Tok::Space(-0.17)),
                        ' ' => out.push(Tok::Space(0.25)),
                        '\\' => {}
                        _ => out.push(Tok::Char(n)),
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '{' => {
                out.push(Tok::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Tok::RBrace);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            '^' => {
                out.push(Tok::Sup);
                i += 1;
            }
            '_' => {
                out.push(Tok::Sub);
                i += 1;
            }
            '&' => i += 1,
            '~' => {
                out.push(Tok::Space(0.25));
                i += 1;
            }
            _ => {
                out.push(Tok::Char(c));
                i += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 语法树
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
enum Node {
    Seq(Vec<Node>),
    Sym(char),
    Text(String),
    Space(f64),
    Frac(Box<Node>, Box<Node>),
    Sqrt(Option<Box<Node>>, Box<Node>),
    Scripts {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    BigOp {
        ch: char,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    Delim {
        open: char,
        close: char,
        body: Box<Node>,
    },
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Self { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn parse_seq(&mut self, stop_at_right: bool) -> Vec<Node> {
        let mut out = Vec::new();
        while let Some(t) = self.peek() {
            match t {
                Tok::RBrace => break,
                Tok::Cmd(c) if stop_at_right && c == "right" => break,
                _ => {}
            }
            let Some(node) = self.parse_atom() else {
                continue;
            };
            out.push(node);
        }
        out
    }

    fn parse_group(&mut self) -> Node {
        match self.peek() {
            Some(Tok::LBrace) => {
                self.pos += 1;
                let seq = self.parse_seq(false);
                if matches!(self.peek(), Some(Tok::RBrace)) {
                    self.pos += 1;
                }
                Node::Seq(seq)
            }
            _ => self.parse_atom().unwrap_or(Node::Seq(vec![])),
        }
    }

    fn parse_atom(&mut self) -> Option<Node> {
        let tok = self.next()?;
        let base = match tok {
            Tok::LBrace => {
                let seq = self.parse_seq(false);
                if matches!(self.peek(), Some(Tok::RBrace)) {
                    self.pos += 1;
                }
                Node::Seq(seq)
            }
            Tok::RBrace => return None,
            Tok::Space(v) => Node::Space(v),
            Tok::Sup | Tok::Sub => {
                // 游离的上下标（`x^2` 已由脚本分支处理，这里兜底当普通符号）
                return None;
            }
            Tok::LBracket => Node::Sym('['),
            Tok::RBracket => Node::Sym(']'),
            Tok::Char(c) => match c {
                '∑' | '∏' | '∫' | '∮' => Node::BigOp {
                    ch: c,
                    sup: None,
                    sub: None,
                },
                _ => Node::Sym(c),
            },
            Tok::Cmd(name) => self.parse_command(&name)?,
        };

        // 处理上标/下标
        let mut sup: Option<Box<Node>> = None;
        let mut sub: Option<Box<Node>> = None;
        loop {
            match self.peek() {
                Some(Tok::Sup) => {
                    self.pos += 1;
                    sup = Some(Box::new(self.parse_group()));
                }
                Some(Tok::Sub) => {
                    self.pos += 1;
                    sub = Some(Box::new(self.parse_group()));
                }
                _ => break,
            }
        }
        if sup.is_none() && sub.is_none() {
            return Some(base);
        }
        Some(match base {
            Node::BigOp { ch, .. } => Node::BigOp { ch, sup, sub },
            other => Node::Scripts {
                base: Box::new(other),
                sup,
                sub,
            },
        })
    }

    fn parse_command(&mut self, name: &str) -> Option<Node> {
        let greek = greek_char(name);
        if let Some(g) = greek {
            return Some(Node::Sym(g));
        }
        match name {
            "frac" | "dfrac" | "tfrac" => {
                let num = self.parse_group();
                let den = self.parse_group();
                Some(Node::Frac(Box::new(num), Box::new(den)))
            }
            "sqrt" => {
                let mut index = None;
                if matches!(self.peek(), Some(Tok::LBracket)) {
                    self.pos += 1;
                    let mut seq = Vec::new();
                    while let Some(t) = self.peek() {
                        if matches!(t, Tok::RBracket) {
                            break;
                        }
                        if let Some(n) = self.parse_atom() {
                            seq.push(n);
                        }
                    }
                    self.pos += 1; // ']'
                    index = Some(Box::new(Node::Seq(seq)));
                }
                let body = self.parse_group();
                Some(Node::Sqrt(index, Box::new(body)))
            }
            "sum" | "prod" | "int" | "oint" => {
                let ch = match name {
                    "sum" => '∑',
                    "prod" => '∏',
                    "int" => '∫',
                    _ => '∮',
                };
                Some(Node::BigOp {
                    ch,
                    sup: None,
                    sub: None,
                })
            }
            "text" | "mathrm" | "mathbf" | "mathit" | "textrm" | "operatorname" => {
                // 取花括号内的原始文字
                let mut text = String::new();
                if matches!(self.peek(), Some(Tok::LBrace)) {
                    self.pos += 1;
                    let mut depth = 1;
                    while let Some(t) = self.next() {
                        match t {
                            Tok::LBrace => {
                                depth += 1;
                                text.push('{');
                            }
                            Tok::RBrace => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                text.push('}');
                            }
                            Tok::Char(c) => text.push(c),
                            Tok::Space(_) => text.push(' '),
                            _ => {}
                        }
                    }
                }
                Some(Node::Text(text))
            }
            "left" => {
                let open = match self.next() {
                    Some(Tok::Char(c)) => c,
                    Some(Tok::Cmd(c)) if c == "{" || c == "lbrace" => '{',
                    Some(Tok::LBrace) => '{',
                    _ => '(',
                };
                let seq = self.parse_seq(true);
                // 吃掉 \right 与紧随的分隔符
                let mut close = ')';
                if let Some(Tok::Cmd(c)) = self.peek() {
                    if c == "right" {
                        self.pos += 1;
                        if let Some(t) = self.next() {
                            close = match t {
                                Tok::Char(c) => c,
                                Tok::Cmd(c) if c == "}" || c == "rbrace" => '}',
                                Tok::RBrace => '}',
                                _ => ')',
                            };
                        }
                    }
                }
                Some(Node::Delim {
                    open,
                    close,
                    body: Box::new(Node::Seq(seq)),
                })
            }
            "right" => None,
            "cdot" => Some(Node::Sym('·')),
            "times" => Some(Node::Sym('×')),
            "div" => Some(Node::Sym('÷')),
            "pm" => Some(Node::Sym('±')),
            "mp" => Some(Node::Sym('∓')),
            "leq" | "le" => Some(Node::Sym('≤')),
            "geq" | "ge" => Some(Node::Sym('≥')),
            "neq" | "ne" => Some(Node::Sym('≠')),
            "approx" => Some(Node::Sym('≈')),
            "equiv" => Some(Node::Sym('≡')),
            "sim" => Some(Node::Sym('∼')),
            "propto" => Some(Node::Sym('∝')),
            "infty" => Some(Node::Sym('∞')),
            "partial" => Some(Node::Sym('∂')),
            "nabla" => Some(Node::Sym('∇')),
            "angle" => Some(Node::Sym('∠')),
            "in" => Some(Node::Sym('∈')),
            "notin" => Some(Node::Sym('∉')),
            "subset" => Some(Node::Sym('⊂')),
            "supset" => Some(Node::Sym('⊃')),
            "subseteq" => Some(Node::Sym('⊆')),
            "supseteq" => Some(Node::Sym('⊇')),
            "cup" => Some(Node::Sym('∪')),
            "cap" => Some(Node::Sym('∩')),
            "emptyset" | "varnothing" => Some(Node::Sym('∅')),
            "forall" => Some(Node::Sym('∀')),
            "exists" => Some(Node::Sym('∃')),
            "therefore" => Some(Node::Sym('∴')),
            "because" => Some(Node::Sym('∵')),
            "to" | "rightarrow" => Some(Node::Sym('→')),
            "leftarrow" => Some(Node::Sym('←')),
            "Rightarrow" => Some(Node::Sym('⇒')),
            "Leftarrow" => Some(Node::Sym('⇐')),
            "Leftrightarrow" | "iff" => Some(Node::Sym('⇔')),
            "mapsto" => Some(Node::Sym('↦')),
            "ast" => Some(Node::Sym('∗')),
            "circ" => Some(Node::Sym('∘')),
            "triangle" => Some(Node::Sym('△')),
            "square" => Some(Node::Sym('□')),
            "diamond" => Some(Node::Sym('◇')),
            "prime" => Some(Node::Sym('′')),
            "ldots" | "dots" | "cdots" => Some(Node::Text("…".into())),
            "quad" => Some(Node::Space(1.0)),
            "qquad" => Some(Node::Space(2.0)),
            "langle" => Some(Node::Sym('〈')),
            "rangle" => Some(Node::Sym('〉')),
            "vert" | "mid" => Some(Node::Sym('|')),
            "lbrace" => Some(Node::Sym('{')),
            "rbrace" => Some(Node::Sym('}')),
            "|" => Some(Node::Sym('‖')),
            _ => {
                // 未知命令：忽略控制词，保留可读性交给调用方
                None
            }
        }
    }
}

fn greek_char(name: &str) -> Option<char> {
    let lower = [
        ("alpha", 'α'),
        ("beta", 'β'),
        ("gamma", 'γ'),
        ("delta", 'δ'),
        ("epsilon", 'ε'),
        ("varepsilon", 'ε'),
        ("zeta", 'ζ'),
        ("eta", 'η'),
        ("theta", 'θ'),
        ("vartheta", 'θ'),
        ("iota", 'ι'),
        ("kappa", 'κ'),
        ("lambda", 'λ'),
        ("mu", 'μ'),
        ("nu", 'ν'),
        ("xi", 'ξ'),
        ("omicron", 'ο'),
        ("pi", 'π'),
        ("rho", 'ρ'),
        ("sigma", 'σ'),
        ("tau", 'τ'),
        ("upsilon", 'υ'),
        ("phi", 'φ'),
        ("varphi", 'φ'),
        ("chi", 'χ'),
        ("psi", 'ψ'),
        ("omega", 'ω'),
    ];
    let upper = [
        ("Gamma", 'Γ'),
        ("Delta", 'Δ'),
        ("Theta", 'Θ'),
        ("Lambda", 'Λ'),
        ("Xi", 'Ξ'),
        ("Pi", 'Π'),
        ("Sigma", 'Σ'),
        ("Upsilon", 'Υ'),
        ("Phi", 'Φ'),
        ("Psi", 'Ψ'),
        ("Omega", 'Ω'),
    ];
    lower
        .iter()
        .chain(upper.iter())
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

// ---------------------------------------------------------------------------
// 排版（盒子模型）
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
enum Item {
    /// 字形：字符、字号、基线左端点
    Glyph { ch: char, size: f64, x: f64, y: f64 },
    /// 结构矩形（分数线/根号顶线），取中线画单笔
    Rule { x: f64, y: f64, w: f64, h: f64 },
    /// 预先算好的笔画（根号骨架）
    Custom(Stroke),
}

#[derive(Clone, Debug, Default)]
struct BoxD {
    w: f64,
    h: f64,
    d: f64,
    items: Vec<Item>,
}

impl BoxD {
    fn shift(mut self, dx: f64, dy: f64) -> BoxD {
        for it in &mut self.items {
            match it {
                Item::Glyph { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                Item::Rule { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                Item::Custom(s) => {
                    for p in &mut s.points {
                        p.0 += dx;
                        p.1 += dy;
                    }
                }
            }
        }
        self
    }
}

const SCRIPT_SCALE: f64 = 0.7;

/// 字形宽度（mm）：字体链能量到就用量出来的，否则按符号经验值。
fn glyph_width(ch: char, size: f64, fonts: &[FontFamily], symbols: &SymbolFonts) -> f64 {
    if let Some(f) = fonts.iter().find(|f| f.has(ch)) {
        return measure_text(&ch.to_string(), fonts, &TextStyle::new(size))
            .max(0.0)
            .max(f.scale_for_size(size) * f.default_advance * 0.5);
    }
    if let Some((name, key)) = symbols::symbol_fallback(ch) {
        if let Some(f) = symbols.get(name) {
            if let Some(g) = f.glyph(key) {
                return g.advance * f.scale_for_size(size);
            }
        }
    }
    if symbols::is_procedural(ch) {
        return size * 0.62;
    }
    size * 0.5
}

fn typeset(node: &Node, size: f64, fonts: &[FontFamily], symbols: &SymbolFonts) -> BoxD {
    match node {
        Node::Seq(nodes) => {
            let mut out = BoxD::default();
            for n in nodes {
                let b = typeset(n, size, fonts, symbols);
                let w = b.w;
                let mut shifted = b.shift(out.w, 0.0);
                out.h = out.h.max(shifted.h);
                out.d = out.d.max(shifted.d);
                out.items.append(&mut shifted.items);
                out.w += w;
            }
            out
        }
        Node::Sym(ch) => {
            let w = glyph_width(*ch, size, fonts, symbols);
            BoxD {
                w,
                h: size * 0.7,
                d: size * 0.2,
                items: vec![Item::Glyph {
                    ch: *ch,
                    size,
                    x: 0.0,
                    y: 0.0,
                }],
            }
        }
        Node::Text(s) => {
            let w = measure_text(s, fonts, &TextStyle::new(size));
            let mut items = Vec::new();
            let mut x = 0.0;
            for ch in s.chars() {
                let cw = glyph_width(ch, size, fonts, symbols);
                items.push(Item::Glyph {
                    ch,
                    size,
                    x,
                    y: 0.0,
                });
                x += cw;
            }
            BoxD {
                w: if w > 0.0 { w } else { x },
                h: size * 0.7,
                d: size * 0.2,
                items,
            }
        }
        Node::Space(em) => BoxD {
            w: size * em,
            ..Default::default()
        },
        Node::Frac(num, den) => {
            let s = size * SCRIPT_SCALE;
            let n = typeset(num, s, fonts, symbols);
            let d = typeset(den, s, fonts, symbols);
            let (nw, nh, nd) = (n.w, n.h, n.d);
            let (dw, dh, dd) = (d.w, d.h, d.d);
            let axis = size * 0.28;
            let gap = size * 0.18;
            let w = nw.max(dw) + size * 0.24;
            let num_dy = axis + gap + nd;
            let den_dy = axis - size * 0.06 - dh;
            let mut items = Vec::new();
            let mut nb = n.shift((w - nw) / 2.0, num_dy);
            items.append(&mut nb.items);
            let mut db = d.shift((w - dw) / 2.0, den_dy);
            items.append(&mut db.items);
            items.push(Item::Rule {
                x: 0.0,
                y: axis,
                w,
                h: size * 0.05,
            });
            BoxD {
                w,
                h: num_dy + nh,
                d: dd - den_dy,
                items,
            }
        }
        Node::Sqrt(index, body) => {
            let b = typeset(body, size, fonts, symbols);
            let (bw, bh, bd) = (b.w, b.h, b.d);
            let pad = size * 0.12;
            let top = bh + pad * 0.6;
            let bottom = -bd - pad * 0.4;
            let w_rad = size * 0.55;
            let body_x = w_rad + pad * 0.2;
            let total_w = body_x + bw + pad * 0.4;
            let mut items = Vec::new();
            let mut bb = b.shift(body_x, 0.0);
            items.append(&mut bb.items);
            // 根号骨架：小横 → 最低点 → 右上角 → 顶线（一笔画完）
            let hook_y = bottom + (top - bottom) * 0.56;
            let pts = vec![
                (0.0, hook_y),
                (w_rad * 0.25, hook_y),
                (w_rad * 0.62, bottom),
                (w_rad, top),
                (total_w, top),
            ];
            items.push(Item::Custom(Stroke::new(
                pts,
                false,
                crate::stroke::Role::Glyph,
            )));
            let mut h = top;
            let mut d = -bottom;
            if let Some(idx) = index {
                let ib = typeset(idx, size * 0.55, fonts, symbols);
                let (ih, idd) = (ib.h, ib.d);
                let mut ibs = ib.shift(-size * 0.1, top + size * 0.05);
                items.append(&mut ibs.items);
                h = h.max(top + ih);
                d = d.max(idd);
            }
            BoxD {
                w: total_w,
                h,
                d,
                items,
            }
        }
        Node::Scripts { base, sup, sub } => {
            let b = typeset(base, size, fonts, symbols);
            let ss = size * SCRIPT_SCALE;
            let mut out = b;
            let base_w = out.w;
            if let Some(sup) = sup {
                let s = typeset(sup, ss, fonts, symbols);
                let (sw, shh) = (s.w, s.h);
                let dy = size * 0.45;
                let mut sh = s.shift(base_w, dy);
                out.h = out.h.max(dy + shh);
                out.items.append(&mut sh.items);
                out.w = out.w.max(base_w + sw);
            }
            if let Some(sub) = sub {
                let s = typeset(sub, ss, fonts, symbols);
                let (sw, sdd) = (s.w, s.d);
                let dy = -size * 0.25;
                let mut sh = s.shift(base_w, dy);
                out.d = out.d.max(-(dy + sdd));
                out.items.append(&mut sh.items);
                out.w = out.w.max(base_w + sw);
            }
            out
        }
        Node::BigOp { ch, sup, sub } => {
            let op_size = size * 1.35;
            let w = if *ch == '∫' || *ch == '∮' {
                size * 0.9
            } else {
                size * 0.95
            };
            let (top, bot) = (op_size * 0.79, -op_size * 0.21);
            let mut items = vec![Item::Glyph {
                ch: *ch,
                size: op_size,
                x: (w - op_size * 0.78) / 2.0,
                y: 0.0,
            }];
            let mut h = top;
            let mut d = -bot;
            let ss = size * 0.62;
            if let Some(sup) = sup {
                let s = typeset(sup, ss, fonts, symbols);
                let (sw, shh) = (s.w, s.h);
                let dy = top + size * 0.12;
                let mut sh = s.shift((w - sw) / 2.0, dy);
                h = h.max(dy + shh);
                items.append(&mut sh.items);
            }
            if let Some(sub) = sub {
                let s = typeset(sub, ss, fonts, symbols);
                let (sw, shh, sdd) = (s.w, s.h, s.d);
                let dy = bot - size * 0.12 - shh;
                let mut sh = s.shift((w - sw) / 2.0, dy);
                d = d.max(-(dy - sdd));
                items.append(&mut sh.items);
            }
            BoxD { w, h, d, items }
        }
        Node::Delim { open, close, body } => {
            let b = typeset(body, size, fonts, symbols);
            let (bw, bh, bd) = (b.w, b.h, b.d);
            let content_h = bh + bd;
            let scale = (content_h / (size * 0.9)).clamp(1.0, 2.6);
            let dsize = size * scale;
            let ow = glyph_width(*open, dsize, fonts, symbols);
            let cw = glyph_width(*close, dsize, fonts, symbols);
            let mut items = vec![
                Item::Glyph {
                    ch: *open,
                    size: dsize,
                    x: 0.0,
                    y: 0.0,
                },
                Item::Glyph {
                    ch: *close,
                    size: dsize,
                    x: ow + bw,
                    y: 0.0,
                },
            ];
            let mut bb = b.shift(ow, 0.0);
            items.append(&mut bb.items);
            BoxD {
                w: ow + bw + cw,
                h: bh.max(dsize * 0.7),
                d: bd.max(dsize * 0.2),
                items,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 光栅化
// ---------------------------------------------------------------------------
/// 把 LaTeX 公式渲染为笔画（mm，Y 向上，基线 y=0，左端 x=0）。
///
/// `fonts` 给定时，链上画得出的字符改用字体链（用户手写体）；数学符号走
/// 内置单线字体或几何画线，绝不产生空心轮廓。
pub fn render(
    latex: &str,
    size_mm: f64,
    fonts: &[FontFamily],
    symbols: &SymbolFonts,
) -> Vec<Stroke> {
    let toks = lex(latex);
    if toks.is_empty() {
        return Vec::new();
    }
    let mut parser = Parser::new(toks);
    let seq = parser.parse_seq(false);
    if seq.is_empty() {
        return Vec::new();
    }
    let node = Node::Seq(seq);
    let b = typeset(&node, size_mm, fonts, symbols);
    if b.items.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<Stroke> = Vec::new();
    let ts = TextStyle::new(size_mm);
    for it in &b.items {
        match it {
            Item::Glyph { ch, size, x, y } => {
                let mut ch = *ch;
                // 数学减号：字体链没有 U+2212 但有 ASCII '-' 时按 '-' 走
                if ch == '\u{2212}'
                    && !fonts.iter().any(|f| f.has(ch))
                    && fonts.iter().any(|f| f.has('-'))
                {
                    ch = '-';
                }
                let (gx, gy) = (*x, *y);
                if fonts.iter().any(|f| f.has(ch)) {
                    let style = TextStyle::new(*size);
                    let lay = layout_text(&ch.to_string(), fonts, &style, (gx, gy), 0.0);
                    out.extend(lay.strokes());
                    continue;
                }
                if let Some((name, key)) = symbols::symbol_fallback(ch) {
                    if let Some((_, mut strs)) = symbols.glyph_strokes(name, key, *size, (gx, gy)) {
                        out.append(&mut strs);
                        continue;
                    }
                }
                if symbols::is_procedural(ch) {
                    out.extend(symbols::draw(ch, gx, gy, *size, None));
                    continue;
                }
                // 实在画不了：跳过（调用方会汇报缺字）
                let _ = ts;
            }
            Item::Rule { x, y, w, h } => {
                out.push(Stroke::new(
                    vec![(*x, y + h / 2.0), (*x + w, y + h / 2.0)],
                    false,
                    crate::stroke::Role::Structure,
                ));
            }
            Item::Custom(s) => out.push(s.clone()),
        }
    }

    // 归位：整体左移到 x=0
    let box_ = crate::geom::BBox::from_points(out.iter().flat_map(|s| s.points.iter().copied()));
    if !box_.is_empty() && box_.x0.abs() > 1e-9 {
        let dx = -box_.x0;
        for s in &mut out {
            for p in &mut s.points {
                p.0 += dx;
            }
        }
    }
    out
}

/// 公式的包围盒（用于行内公式的行高与列宽）。
pub fn measure_box(
    latex: &str,
    size_mm: f64,
    fonts: &[FontFamily],
    symbols: &SymbolFonts,
) -> crate::geom::BBox {
    let toks = lex(latex);
    if toks.is_empty() {
        return crate::geom::BBox::empty();
    }
    let mut parser = Parser::new(toks);
    let seq = parser.parse_seq(false);
    if seq.is_empty() {
        return crate::geom::BBox::empty();
    }
    let b = typeset(&Node::Seq(seq), size_mm, fonts, symbols);
    // 排版盒 → 包围盒（估算，足够做行内间距）
    crate::geom::BBox {
        x0: 0.0,
        y0: -b.d,
        x1: b.w,
        y1: b.h,
    }
}

/// 统计一次渲染里被跳过的字符（诊断用）。
pub fn missing_chars(
    latex: &str,
    _size_mm: f64,
    fonts: &[FontFamily],
    symbols: &SymbolFonts,
) -> Vec<char> {
    let toks = lex(latex);
    let mut parser = Parser::new(toks);
    let seq = parser.parse_seq(false);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut walk = |n: &Node| walk_chars(n, fonts, symbols, &mut seen, &mut out);
    for n in &seq {
        walk(n);
    }
    out
}

fn walk_chars(
    n: &Node,
    fonts: &[FontFamily],
    symbols: &SymbolFonts,
    seen: &mut HashSet<char>,
    out: &mut Vec<char>,
) {
    match n {
        Node::Sym(ch) | Node::BigOp { ch, .. } => {
            let ch = *ch;
            let drawable = fonts.iter().any(|f| f.has(ch))
                || symbols::symbol_fallback(ch)
                    .map(|(name, key)| symbols.get(name).map(|f| f.has(key)).unwrap_or(false))
                    .unwrap_or(false)
                || symbols::is_procedural(ch);
            if !drawable && seen.insert(ch) {
                out.push(ch);
            }
            if let Node::BigOp { sup, sub, .. } = n {
                for c in [sup, sub].into_iter().flatten() {
                    walk_chars(c, fonts, symbols, seen, out);
                }
            }
        }
        Node::Text(s) => {
            for ch in s.chars() {
                let drawable = fonts.iter().any(|f| f.has(ch))
                    || symbols::symbol_fallback(ch).is_some()
                    || symbols::is_procedural(ch);
                if !drawable && seen.insert(ch) {
                    out.push(ch);
                }
            }
        }
        Node::Seq(v) => {
            for c in v {
                walk_chars(c, fonts, symbols, seen, out);
            }
        }
        Node::Frac(a, b) => {
            walk_chars(a, fonts, symbols, seen, out);
            walk_chars(b, fonts, symbols, seen, out);
        }
        Node::Sqrt(i, b) => {
            if let Some(i) = i {
                walk_chars(i, fonts, symbols, seen, out);
            }
            walk_chars(b, fonts, symbols, seen, out);
        }
        Node::Scripts { base, sup, sub } => {
            walk_chars(base, fonts, symbols, seen, out);
            if let Some(s) = sup {
                walk_chars(s, fonts, symbols, seen, out);
            }
            if let Some(s) = sub {
                walk_chars(s, fonts, symbols, seen, out);
            }
        }
        Node::Delim { body, .. } => walk_chars(body, fonts, symbols, seen, out),
        Node::Space(_) => {}
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
    fn renders_simple_math() {
        let (f, s) = setup();
        let strokes = render("x^2 + y_1 = 3", 5.0, &f, &s);
        assert!(!strokes.is_empty());
    }

    #[test]
    fn renders_fraction_and_sqrt() {
        let (f, s) = setup();
        let a = render("\\frac{1}{2}", 5.0, &f, &s);
        assert!(!a.is_empty());
        // 分数线应是一条水平结构线
        assert!(a.iter().any(|st| {
            st.points.len() == 2
                && (st.points[0].1 - st.points[1].1).abs() < 1e-9
                && (st.points[1].0 - st.points[0].0).abs() > 1.0
        }));
        let b = render("\\sqrt{x+1}", 5.0, &f, &s);
        assert!(!b.is_empty());
    }

    #[test]
    fn renders_greek_and_bigop() {
        let (f, s) = setup();
        let a = render("\\alpha + \\beta", 5.0, &f, &s);
        assert!(!a.is_empty());
        let b = render("\\sum_{i=1}^{n} i", 5.0, &f, &s);
        assert!(!b.is_empty());
    }

    #[test]
    fn measure_box_is_positive() {
        let (f, s) = setup();
        let bx = measure_box("\\frac{a}{b}", 5.0, &f, &s);
        assert!(bx.width() > 0.0 && bx.height() > 0.0);
    }
}

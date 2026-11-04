//! 数学符号的单线绘制（`content/equation.py` 的移植）。
//!
//! 写字机只写单线，绝不能画「填充字体的轮廓」（写出来是空心字）。因此符号分三层：
//! 1. 字体链里有这个字 → 用字体链（用户的手写体）；
//! 2. 内置 Hershey `mathupp` / `greek` 有现成键位 → 用它；
//! 3. 剩下常见符号（× ≤ ≥ ∑ ∫ …）→ 在这里按几何直接画线。

use std::collections::HashMap;

use crate::font::registry::FontRegistry;
use crate::font::FontFamily;
use crate::geom::Vec2;
use crate::stroke::{Role, Stroke};

/// 内置符号字体的键位表：Unicode → (内置字体名, 键位)。
pub fn symbol_fallback(ch: char) -> Option<(&'static str, char)> {
    let table: &[(char, &str, char)] = &[
        ('∞', "mathupp", '^'),
        ('°', "mathupp", '`'),
        ('≠', "mathupp", '?'),
        ('≡', "mathupp", '@'),
        ('∈', "mathupp", 'h'),
        ('→', "mathupp", 'i'),
        ('←', "mathupp", 'j'),
        ('↓', "mathupp", 'k'),
        ('÷', "mathupp", 'x'),
        ('∥', "mathupp", 'y'),
        ('⊥', "mathupp", 'z'),
        ('·', "mathupp", '$'),
        ('⋅', "mathupp", '$'),
        ('Α', "greek", 'A'),
        ('Β', "greek", 'B'),
        ('Γ', "greek", 'G'),
        ('Δ', "greek", 'D'),
        ('Ε', "greek", 'E'),
        ('Ζ', "greek", 'Z'),
        ('Η', "greek", 'H'),
        ('Θ', "greek", 'Q'),
        ('Ι', "greek", 'I'),
        ('Κ', "greek", 'K'),
        ('Λ', "greek", 'L'),
        ('Μ', "greek", 'M'),
        ('Ν', "greek", 'N'),
        ('Ξ', "greek", 'X'),
        ('Ο', "greek", 'O'),
        ('Π', "greek", 'P'),
        ('Ρ', "greek", 'R'),
        ('Σ', "greek", 'S'),
        ('Τ', "greek", 'T'),
        ('Υ', "greek", 'U'),
        ('Φ', "greek", 'F'),
        ('Χ', "greek", 'C'),
        ('Ψ', "greek", 'Y'),
        ('Ω', "greek", 'W'),
        ('α', "greek", 'a'),
        ('β', "greek", 'b'),
        ('γ', "greek", 'g'),
        ('δ', "greek", 'd'),
        ('ε', "greek", 'e'),
        ('ζ', "greek", 'z'),
        ('η', "greek", 'h'),
        ('θ', "greek", 'q'),
        ('ι', "greek", 'i'),
        ('κ', "greek", 'k'),
        ('λ', "greek", 'l'),
        ('μ', "greek", 'm'),
        ('ν', "greek", 'n'),
        ('ξ', "greek", 'x'),
        ('ο', "greek", 'o'),
        ('π', "greek", 'p'),
        ('ρ', "greek", 'r'),
        ('σ', "greek", 's'),
        ('τ', "greek", 't'),
        ('υ', "greek", 'u'),
        ('φ', "greek", 'f'),
        ('χ', "greek", 'c'),
        ('ψ', "greek", 'y'),
        ('ω', "greek", 'w'),
    ];
    table
        .iter()
        .find(|(c, _, _)| *c == ch)
        .map(|(_, f, k)| (*f, *k))
}

/// 按几何直接画的符号集合。
pub fn is_procedural(ch: char) -> bool {
    "×≤≥≈≃≅≌∼∝≪≫≮≯⊂⊃⊆⊇⊊∪∩∅⊕⊖⊗⊘⊚⊙○∈∉∋∀∄∴∵⇒⇐⇔↔↦∓±∗∘△□◇∮′″‰≐≑∑∏∫∂∇∠¬∧∨∃⊢⊣⊧⊨⊪⟹⟸⟺•，。、：；！？（）《》「」『』【】“”’—…"
        .contains(ch)
}

const BOX_SYMBOLS: &str = "¬∧∨∃⊢⊣⊧⊨⊪";
const LONG_ARROWS: &str = "⟹⟸⟺";

/// 内置符号字体（`mathupp` / `greek`），懒加载一次。
pub struct SymbolFonts {
    fonts: HashMap<String, FontFamily>,
}

impl SymbolFonts {
    pub fn load() -> Self {
        let mut reg = FontRegistry::builtin();
        let mut fonts = HashMap::new();
        for name in ["mathupp", "mathlow", "greek"] {
            if let Some(f) = reg.get(name) {
                fonts.insert(name.to_string(), f.clone());
            }
        }
        Self { fonts }
    }

    pub fn get(&self, name: &str) -> Option<&FontFamily> {
        self.fonts.get(name)
    }

    /// 内置符号的笔画（键位存在且带笔画时）。返回 (步距, 笔画)。
    pub fn glyph_strokes(
        &self,
        name: &str,
        key: char,
        size: f64,
        origin: Vec2,
    ) -> Option<(f64, Vec<Stroke>)> {
        let fam = self.get(name)?;
        let g = fam.glyph(key)?;
        if g.strokes.is_empty() {
            return None;
        }
        let scale = fam.scale_for_size(size);
        Some((g.advance * scale, g.to_strokes(scale, origin)))
    }
}

fn stroke(pts: Vec<Vec2>, closed: bool) -> Stroke {
    Stroke::new(pts, closed, Role::Glyph)
}

fn line(a: Vec2, b: Vec2) -> Stroke {
    stroke(vec![a, b], false)
}

fn circle_pts(cx: f64, cy: f64, r: f64, n: usize) -> Vec<Vec2> {
    (0..n)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn arc_pts(cx: f64, cy: f64, r: f64, a0: f64, a1: f64, n: usize) -> Vec<Vec2> {
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let a = (a0 + (a1 - a0) * t).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn wave_pts(x0: f64, y: f64, w: f64, amp: f64, n: usize) -> Vec<Vec2> {
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            (x0 + w * t, y + amp * (2.0 * std::f64::consts::PI * t).sin())
        })
        .collect()
}

fn cubic_pts(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, n: usize) -> Vec<Vec2> {
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            (
                u * u * u * p0.0
                    + 3.0 * u * u * t * p1.0
                    + 3.0 * u * t * t * p2.0
                    + t * t * t * p3.0,
                u * u * u * p0.1
                    + 3.0 * u * u * t * p1.1
                    + 3.0 * u * t * t * p2.1
                    + t * t * t * p3.1,
            )
        })
        .collect()
}

/// 按几何画一个单线数学符号。`oy` 为基线，`em` 为字号；`box` 给定时按其定宽定位。
pub fn draw(
    ch: char,
    ox: f64,
    oy: f64,
    em: f64,
    box_: Option<(f64, f64, f64, f64)>,
) -> Vec<Stroke> {
    let mut w = em * 0.62;
    let mut x0 = ox;
    let mut cy = oy + em * 0.27;
    if let Some((bx0, by0, bx1, by1)) = box_ {
        x0 = bx0;
        w = bx1 - bx0;
        cy = (by0 + by1) / 2.0;
    } else if BOX_SYMBOLS.contains(ch) {
        let b = box_from_em(ch, ox, oy, em);
        x0 = b.0;
        w = b.2 - b.0;
        cy = (b.1 + b.3) / 2.0;
    }
    if BOX_SYMBOLS.contains(ch) {
        let b = if let Some(bb) = box_ {
            bb
        } else {
            box_from_em(ch, ox, oy, em)
        };
        return draw_box_symbol(ch, b);
    }
    let g = em * 0.13;
    let a = em * 0.05;
    let r_ring = em * 0.26;

    // 全角标点：中文字库通常没有这些字形，按几何画单笔（占一个字身）
    if is_cjk_punct(ch) {
        return draw_cjk_punct(ch, ox, oy, em);
    }

    let line_s = |p: Vec2, q: Vec2| line(p, q);
    let wave = |y: f64, amp: f64| stroke(wave_pts(x0, y, w, amp, 8), false);
    let circle = |cx: f64, cyy: f64, r: f64| stroke(circle_pts(cx, cyy, r, 16), true);
    let arc = |cx: f64, cyy: f64, r: f64, a0: f64, a1: f64| {
        stroke(arc_pts(cx, cyy, r, a0, a1, 12), false)
    };
    let dot = |p: Vec2| stroke(vec![p, p], false);
    let over_slash = |pad: f64| line((x0 + pad, cy - em * 0.34), (x0 + w - pad, cy + em * 0.34));

    match ch {
        '×' => {
            return vec![
                Stroke::new(
                    vec![(x0, cy - g), (x0 + w * 0.72, cy + g)],
                    false,
                    Role::Glyph,
                ),
                Stroke::new(
                    vec![(x0, cy + g), (x0 + w * 0.72, cy - g)],
                    false,
                    Role::Glyph,
                ),
            ]
        }
        '≤' | '≥' => {
            let s = if ch == '≥' { -1.0 } else { 1.0 };
            let tipx = x0 + w * (0.5 - s * 0.5);
            let backx = x0 + w * (0.5 + s * 0.5);
            return vec![
                Stroke::new(
                    vec![(backx, cy + g * 1.6), (tipx, cy), (backx, cy - g * 1.6)],
                    false,
                    Role::Glyph,
                ),
                line((x0, cy - g * 2.1), (x0 + w, cy - g * 2.1)),
            ];
        }
        '≈' => return vec![wave(cy + g * 0.55, a), wave(cy - g * 0.55, a)],
        '∼' => return vec![wave(cy, a)],
        '≃' => {
            return vec![
                wave(cy + g * 0.7, a),
                line((x0, cy - g * 0.8), (x0 + w, cy - g * 0.8)),
            ]
        }
        '≅' | '≌' => {
            return vec![
                wave(cy + g * 0.95, a),
                line((x0, cy - g * 0.35), (x0 + w, cy - g * 0.35)),
                line((x0, cy - g * 1.25), (x0 + w, cy - g * 1.25)),
            ]
        }
        '≐' => {
            return vec![
                line((x0, cy - g * 0.5), (x0 + w, cy - g * 0.5)),
                dot((x0 + w / 2.0, cy + g * 0.7)),
            ]
        }
        '≑' => {
            return vec![
                line((x0, cy - g * 0.9), (x0 + w, cy - g * 0.9)),
                dot((x0 + w / 2.0, cy)),
                line((x0, cy + g * 0.9), (x0 + w, cy + g * 0.9)),
            ]
        }
        '∝' => {
            let hh = em * 0.24;
            return vec![
                Stroke::new(
                    vec![(x0, cy), (x0 + w * 0.5, cy + hh * 1.2), (x0 + w, cy + hh)],
                    false,
                    Role::Glyph,
                ),
                Stroke::new(
                    vec![(x0, cy), (x0 + w * 0.5, cy - hh * 1.2), (x0 + w, cy - hh)],
                    false,
                    Role::Glyph,
                ),
            ];
        }
        '≪' | '≫' => {
            let s = if ch == '≪' { 1.0 } else { -1.0 };
            let right = s > 0.0;
            let chevron = |cx: f64| {
                Stroke::new(
                    vec![
                        (
                            cx + if right { em * 0.20 } else { -em * 0.20 },
                            cy + em * 0.22,
                        ),
                        (cx, cy),
                        (
                            cx + if right { em * 0.20 } else { -em * 0.20 },
                            cy - em * 0.22,
                        ),
                    ],
                    false,
                    Role::Glyph,
                )
            };
            return vec![
                chevron(x0 + if s > 0.0 { 0.0 } else { em * 0.26 }),
                chevron(x0 + if s > 0.0 { em * 0.26 } else { 0.0 }),
            ];
        }
        '≮' | '≯' => {
            let right = ch == '≮';
            let chevron = Stroke::new(
                vec![
                    (
                        x0 + if right { em * 0.20 } else { -em * 0.20 },
                        cy + em * 0.22,
                    ),
                    (x0, cy),
                    (
                        x0 + if right { em * 0.20 } else { -em * 0.20 },
                        cy - em * 0.22,
                    ),
                ],
                false,
                Role::Glyph,
            );
            return vec![chevron, over_slash(0.0)];
        }
        '⊂' | '⊃' | '⊆' | '⊇' | '⊊' => {
            let r = em * 0.26;
            let mut out = if matches!(ch, '⊂' | '⊆' | '⊊') {
                vec![arc(x0 + r * 0.75, cy, r, 60.0, 300.0)]
            } else {
                vec![arc(x0 + w - r * 0.75, cy, r, -120.0, 120.0)]
            };
            if matches!(ch, '⊆' | '⊇') {
                let y = cy - r * 60.0_f64.to_radians().sin() - em * 0.05;
                out.push(line((x0, y), (x0 + w, y)));
            }
            if ch == '⊊' {
                out.push(line(
                    (x0 + w * 0.80, cy - em * 0.32),
                    (x0 + w * 0.34, cy + em * 0.32),
                ));
            }
            return out;
        }
        '∪' | '∩' => {
            let r = em * 0.27;
            return if ch == '∪' {
                vec![arc(x0 + r, cy + r * 0.35, r, 200.0, 340.0)]
            } else {
                vec![arc(x0 + r, cy - r * 0.35, r, 20.0, 160.0)]
            };
        }
        '∈' | '∉' | '∋' => {
            let r = em * 0.29;
            let mut out = if ch == '∋' {
                vec![
                    arc(x0 + w - r, cy, r, -105.0, 105.0),
                    line((x0, cy), (x0 + w, cy)),
                ]
            } else {
                vec![
                    arc(x0 + r, cy, r, 75.0, 285.0),
                    line((x0, cy), (x0 + w * 0.98, cy)),
                ]
            };
            if ch == '∉' {
                out.push(over_slash(em * 0.05));
            }
            return out;
        }
        '∅' => return vec![circle(x0 + w / 2.0, cy, r_ring), over_slash(em * 0.02)],
        '○' => return vec![circle(x0 + w / 2.0, cy, r_ring)],
        '⊕' | '⊖' | '⊗' | '⊘' | '⊚' | '⊙' => {
            let cx = x0 + w / 2.0;
            let r = r_ring;
            let mut out = vec![circle(cx, cy, r)];
            match ch {
                '⊕' => {
                    out.push(line((cx - r, cy), (cx + r, cy)));
                    out.push(line((cx, cy - r), (cx, cy + r)));
                }
                '⊖' => out.push(line((cx - r, cy), (cx + r, cy))),
                '⊗' => {
                    let d = r * std::f64::consts::FRAC_1_SQRT_2;
                    out.push(line((cx - d, cy - d), (cx + d, cy + d)));
                    out.push(line((cx - d, cy + d), (cx + d, cy - d)));
                }
                '⊘' => out.push(over_slash(em * 0.02)),
                '⊚' => out.push(circle(cx, cy, r * 0.45)),
                _ => out.push(dot((cx, cy))),
            }
            return out;
        }
        '∀' | '∄' => {
            let half = w * 0.5;
            let top = cy + em * 0.26;
            let bot = cy - em * 0.26;
            if ch == '∀' {
                let mut out = vec![Stroke::new(
                    vec![(x0, top), (x0 + half, bot), (x0 + w, top)],
                    false,
                    Role::Glyph,
                )];
                let ty = cy + em * 0.04;
                let f = (ty - bot) / (top - bot);
                out.push(line(
                    (x0 + half * (1.0 - f), ty),
                    (x0 + half * (1.0 + f), ty),
                ));
                return out;
            }
            return vec![
                line((x0, top), (x0 + w, top)),
                line((x0, bot), (x0 + w, bot)),
                line((x0 + w, top), (x0 + w, bot)),
                over_slash(em * 0.05),
            ];
        }
        '∴' | '∵' => {
            let up = ch == '∴';
            let y1 = if up { cy + em * 0.15 } else { cy - em * 0.15 };
            let y2 = if up { cy - em * 0.15 } else { cy + em * 0.15 };
            return vec![
                dot((x0 + w / 2.0, y1)),
                dot((x0 + em * 0.08, y2)),
                dot((x0 + w - em * 0.08, y2)),
            ];
        }
        '⇒' | '⇐' | '⇔' | '→' | '←' | '↔' | '↦' | '⟹' | '⟸' | '⟺' => {
            let go_left = matches!(ch, '⇐' | '⇔' | '←' | '⟸' | '⟺' | '↔');
            let go_right = matches!(ch, '⇒' | '⇔' | '→' | '↦' | '⟹' | '⟺' | '↔');
            let dbl = matches!(ch, '⇒' | '⇐' | '⇔' | '⟹' | '⟸' | '⟺');
            let lng = LONG_ARROWS.contains(ch);
            let hh = em * if lng { 0.20 } else { 0.15 };
            let d1 = em * if lng { 0.30 } else { 0.17 };
            let d2 = em * if lng { 0.52 } else { 0.29 };
            let mut out = vec![line((x0, cy), (x0 + w, cy))];
            if go_right {
                out.push(Stroke::new(
                    vec![(x0 + w - d1, cy + hh), (x0 + w, cy), (x0 + w - d1, cy - hh)],
                    false,
                    Role::Glyph,
                ));
                if dbl {
                    out.push(Stroke::new(
                        vec![
                            (x0 + w - d2, cy + hh),
                            (x0 + w - d1 * 0.7, cy),
                            (x0 + w - d2, cy - hh),
                        ],
                        false,
                        Role::Glyph,
                    ));
                }
            }
            if go_left {
                out.push(Stroke::new(
                    vec![(x0 + d1, cy + hh), (x0, cy), (x0 + d1, cy - hh)],
                    false,
                    Role::Glyph,
                ));
                if dbl {
                    out.push(Stroke::new(
                        vec![(x0 + d2, cy + hh), (x0 + d1 * 0.7, cy), (x0 + d2, cy - hh)],
                        false,
                        Role::Glyph,
                    ));
                }
            }
            if ch == '↦' {
                out.push(line(
                    (x0 + w - em * 0.02, cy - em * 0.16),
                    (x0 + w - em * 0.02, cy + em * 0.16),
                ));
            }
            return out;
        }
        '∑' | '∏' => {
            let top = oy + em * 0.79;
            let bot = oy - em * 0.21;
            let ww = em * if ch == '∑' { 0.78 } else { 0.86 };
            if ch == '∏' {
                return vec![
                    line((x0, bot), (x0, top)),
                    line((x0 + ww, bot), (x0 + ww, top)),
                    line((x0, top), (x0 + ww, top)),
                ];
            }
            let mid = (top + bot) / 2.0;
            let vx = x0 + ww * 0.52;
            return vec![
                line((x0, top), (x0 + ww, top)),
                Stroke::new(
                    vec![(x0 + ww * 0.07, top), (vx, mid), (x0 + ww * 0.07, bot)],
                    false,
                    Role::Glyph,
                ),
                line((x0, bot), (x0 + ww, bot)),
            ];
        }
        '∫' => {
            let top = oy + em * 0.79;
            let bot = oy - em * 0.21;
            let cx = x0 + em * 0.31;
            let h = top - bot;
            return vec![Stroke::new(
                cubic_pts(
                    (cx - em * 0.29, bot),
                    (cx + em * 0.21, bot + h * 0.27),
                    (cx - em * 0.21, top - h * 0.27),
                    (cx + em * 0.29, top),
                    18,
                ),
                false,
                Role::Glyph,
            )];
        }
        '∂' | '∇' => {
            let ww = em * 0.62;
            if ch == '∇' {
                let top = cy + em * 0.30;
                let bot = cy - em * 0.32;
                return vec![Stroke::new(
                    vec![(x0, top), (x0 + ww, top), (x0 + ww / 2.0, bot)],
                    true,
                    Role::Glyph,
                )];
            }
            let r = em * 0.26;
            let ccx = x0 + r + em * 0.02;
            let mut out = vec![arc(ccx, cy, r, 70.0, 390.0)];
            out.push(line(
                (
                    ccx + r * 70.0_f64.to_radians().cos(),
                    cy + r * 70.0_f64.to_radians().sin(),
                ),
                (x0 + ww, cy + r * 0.92),
            ));
            return out;
        }
        '∠' => {
            let vx = x0;
            let vy = cy - em * 0.22;
            return vec![
                line((vx, vy), (x0 + w, vy)),
                line((vx, vy), (x0 + w * 0.78, cy + em * 0.26)),
                arc(vx, vy, em * 0.22, 0.0, 33.0),
            ];
        }
        '±' | '∓' => {
            let up = if ch == '±' { 1.0 } else { -1.0 };
            let yy = cy + g * 0.6 * up;
            let bar = cy - g * 1.1 * up;
            return vec![
                line((x0, yy), (x0 + w * 0.8, yy)),
                line((x0 + w * 0.4, yy - g * 0.8), (x0 + w * 0.4, yy + g * 0.8)),
                line((x0, bar), (x0 + w * 0.8, bar)),
            ];
        }
        '∗' => {
            let cx = x0 + w / 2.0;
            let d = em * 0.13;
            return vec![
                line((cx - d, cy), (cx + d, cy)),
                line((cx - d * 0.7, cy - d * 0.7), (cx + d * 0.7, cy + d * 0.7)),
                line((cx - d * 0.7, cy + d * 0.7), (cx + d * 0.7, cy - d * 0.7)),
            ];
        }
        '∘' => return vec![circle(x0 + w / 2.0, cy, em * 0.08)],
        '•' => return vec![circle(x0 + w / 2.0, cy, em * 0.11)],
        '△' | '□' | '◇' => {
            let cx = x0 + w / 2.0;
            let rr = r_ring;
            let pts = match ch {
                '△' => vec![
                    (cx, cy + rr),
                    (cx - rr * 0.95, cy - rr * 0.72),
                    (cx + rr * 0.95, cy - rr * 0.72),
                ],
                '□' => vec![
                    (cx - rr, cy - rr),
                    (cx + rr, cy - rr),
                    (cx + rr, cy + rr),
                    (cx - rr, cy + rr),
                ],
                _ => vec![(cx, cy + rr), (cx + rr, cy), (cx, cy - rr), (cx - rr, cy)],
            };
            return vec![Stroke::new(pts, true, Role::Glyph)];
        }
        '′' => {
            return vec![line(
                (x0 + em * 0.17, cy + em * 0.08),
                (x0 + em * 0.07, cy + em * 0.36),
            )]
        }
        '″' => {
            return vec![
                line(
                    (x0 + em * 0.15, cy + em * 0.08),
                    (x0 + em * 0.05, cy + em * 0.36),
                ),
                line(
                    (x0 + em * 0.33, cy + em * 0.08),
                    (x0 + em * 0.23, cy + em * 0.36),
                ),
            ]
        }
        '‰' => {
            let rr = em * 0.09;
            return vec![
                circle(x0 + rr + em * 0.05, cy + em * 0.15, rr),
                circle(x0 + rr + em * 0.05, cy - em * 0.15, rr),
                line(
                    (x0 + em * 0.05, cy - em * 0.30),
                    (x0 + em * 0.35, cy + em * 0.30),
                ),
            ];
        }
        '∮' => {
            let top = oy + em * 0.79;
            let bot = oy - em * 0.21;
            let cx = x0 + em * 0.31;
            let h = top - bot;
            return vec![
                Stroke::new(
                    cubic_pts(
                        (cx - em * 0.29, bot),
                        (cx + em * 0.21, bot + h * 0.27),
                        (cx - em * 0.21, top - h * 0.27),
                        (cx + em * 0.29, top),
                        18,
                    ),
                    false,
                    Role::Glyph,
                ),
                circle(cx, cy, em * 0.10),
            ];
        }
        _ => {}
    }
    let _ = line_s;
    Vec::new()
}

/// 没有 mathtext 墨迹框时，按字号估一个字形框（比例取自实测）。
fn box_from_em(ch: char, ox: f64, oy: f64, em: f64) -> (f64, f64, f64, f64) {
    match ch {
        '∃' => (ox, oy, ox + em * 0.48, oy + em * 0.70),
        '¬' => (ox, oy + em * 0.12, ox + em * 0.56, oy + em * 0.42),
        '∧' | '∨' => (ox, oy + em * 0.02, ox + em * 0.50, oy + em * 0.56),
        _ => (ox, oy, ox + em * 0.58, oy + em * 0.62),
    }
}

/// 按字形墨迹框画单线逻辑联结词与推出号。
pub fn draw_box_symbol(ch: char, b: (f64, f64, f64, f64)) -> Vec<Stroke> {
    let (x0, y0, x1, y1) = b;
    let cx = (x0 + x1) / 2.0;
    match ch {
        '¬' => {
            return vec![Stroke::new(
                vec![(x0, y1), (x1, y1), (x1, y0)],
                false,
                Role::Glyph,
            )]
        }
        '∧' => {
            return vec![Stroke::new(
                vec![(x0, y0), (cx, y1), (x1, y0)],
                false,
                Role::Glyph,
            )]
        }
        '∨' => {
            return vec![Stroke::new(
                vec![(x0, y1), (cx, y0), (x1, y1)],
                false,
                Role::Glyph,
            )]
        }
        _ => {}
    }
    let cy = (y0 + y1) / 2.0;
    if matches!(ch, '⊢' | '⊧' | '⊨' | '⊣' | '⊪') {
        let h = y1 - y0;
        let bar_x = if ch == '⊣' { x1 } else { x0 };
        let far_x = if ch == '⊣' { x0 } else { x1 };
        let mut out = vec![Stroke::new(
            vec![(bar_x, y0), (bar_x, y1)],
            false,
            Role::Glyph,
        )];
        let lines = if ch == '⊪' {
            3
        } else if ch == '⊨' {
            2
        } else {
            1
        };
        for i in 0..lines {
            let y = y0 + h * 0.105 * i as f64;
            out.push(Stroke::new(
                vec![(bar_x, y), (far_x, y)],
                false,
                Role::Glyph,
            ));
        }
        return out;
    }
    // ∃
    vec![
        Stroke::new(
            vec![(x0, y1), (x1, y1), (x1, y0), (x0, y0)],
            false,
            Role::Glyph,
        ),
        Stroke::new(
            vec![(x1, cy), (x0 + (x1 - x0) * 0.12, cy)],
            false,
            Role::Glyph,
        ),
    ]
}

// ---------------------------------------------------------------------------
// 全局符号字体（排版时无字体可画的字符走这里兜底）
// ---------------------------------------------------------------------------
static GLOBAL_SYMBOLS: std::sync::OnceLock<SymbolFonts> = std::sync::OnceLock::new();

/// 进程内共享的符号字体集合（首次使用时加载 `mathupp` / `greek`）。
pub fn global() -> &'static SymbolFonts {
    GLOBAL_SYMBOLS.get_or_init(SymbolFonts::load)
}

/// 字体链缺字时的兜底：返回 (步距, 笔画)，都画不了返回 None。
///
/// 顺序与公式渲染一致：先内置单线 Hershey 字体，再几何画线。
pub fn fallback(ch: char, size: f64, origin: Vec2) -> Option<(f64, Vec<Stroke>)> {
    let fonts = global();
    if let Some((name, key)) = symbol_fallback(ch) {
        if let Some((adv, strokes)) = fonts.glyph_strokes(name, key, size, origin) {
            return Some((adv, strokes));
        }
    }
    if is_procedural(ch) {
        let strokes = draw(ch, origin.0, origin.1, size, None);
        if !strokes.is_empty() {
            let advance = if is_cjk_punct(ch) { size } else { size * 0.78 };
            return Some((advance, strokes));
        }
    }
    None
}

/// 全角标点集合（中文字库普遍缺失，用几何笔画兜底）。
pub fn is_cjk_punct(ch: char) -> bool {
    "，。、：；！？（）《》「」『』【】“”‘’—…·".contains(ch)
}

/// 按几何画全角标点（em 字身，基线 y=0，左下角 x=0）。
fn draw_cjk_punct(ch: char, ox: f64, oy: f64, em: f64) -> Vec<Stroke> {
    let e = em;
    let x = |f: f64| ox + f * e;
    let y = |f: f64| oy + f * e;
    let mk = |pts: Vec<Vec2>, closed: bool| Stroke::new(pts, closed, Role::Glyph);
    let dot = |cx: f64, cy: f64, r: f64| mk(circle_pts(cx, cy, r, 10), true);
    let arc =
        |cx: f64, cy: f64, r: f64, a0: f64, a1: f64| mk(arc_pts(cx, cy, r, a0, a1, 12), false);

    match ch {
        // 逗号：左下角一小段带钩的短撇
        '，' => vec![mk(
            vec![(x(0.20), y(0.22)), (x(0.26), y(0.10)), (x(0.17), y(-0.01))],
            false,
        )],
        // 句号：左下角小圆
        '。' => vec![dot(x(0.24), y(0.16), e * 0.085)],
        // 顿号：左下角短斜笔
        '、' => vec![mk(vec![(x(0.15), y(0.20)), (x(0.33), y(0.11))], false)],
        // 冒号：上下两点
        '：' => vec![
            dot(x(0.24), y(0.60), e * 0.055),
            dot(x(0.24), y(0.20), e * 0.055),
        ],
        // 分号：上点 + 下逗号
        '；' => vec![
            dot(x(0.24), y(0.60), e * 0.055),
            mk(
                vec![(x(0.22), y(0.22)), (x(0.27), y(0.10)), (x(0.18), y(0.0))],
                false,
            ),
        ],
        // 感叹号：竖 + 点
        '！' => vec![
            mk(vec![(x(0.28), y(0.78)), (x(0.28), y(0.30))], false),
            dot(x(0.28), y(0.10), e * 0.055),
        ],
        // 问号：上半圆弧 + 竖 + 点
        '？' => vec![
            arc(x(0.30), y(0.60), e * 0.17, 200.0, 20.0),
            mk(vec![(x(0.30), y(0.42)), (x(0.30), y(0.30))], false),
            dot(x(0.30), y(0.10), e * 0.055),
        ],
        // 圆括号：左右两段弧
        '（' => vec![arc(x(0.48), y(0.40), e * 0.32, 120.0, 240.0)],
        '）' => vec![arc(x(0.14), y(0.40), e * 0.32, -60.0, 60.0)],
        // 书名号
        '《' => vec![
            mk(
                vec![(x(0.34), y(0.56)), (x(0.20), y(0.40)), (x(0.34), y(0.24))],
                false,
            ),
            mk(
                vec![(x(0.52), y(0.56)), (x(0.38), y(0.40)), (x(0.52), y(0.24))],
                false,
            ),
        ],
        '》' => vec![
            mk(
                vec![(x(0.38), y(0.56)), (x(0.52), y(0.40)), (x(0.38), y(0.24))],
                false,
            ),
            mk(
                vec![(x(0.20), y(0.56)), (x(0.34), y(0.40)), (x(0.20), y(0.24))],
                false,
            ),
        ],
        // 直角引号
        '「' => vec![mk(
            vec![(x(0.22), y(0.62)), (x(0.36), y(0.62)), (x(0.36), y(0.28))],
            false,
        )],
        '」' => vec![mk(
            vec![(x(0.34), y(0.18)), (x(0.20), y(0.18)), (x(0.20), y(0.52))],
            false,
        )],
        '『' => vec![
            mk(
                vec![(x(0.22), y(0.62)), (x(0.36), y(0.62)), (x(0.36), y(0.28))],
                false,
            ),
            mk(
                vec![(x(0.14), y(0.66)), (x(0.42), y(0.66)), (x(0.42), y(0.22))],
                false,
            ),
        ],
        '』' => vec![
            mk(
                vec![(x(0.34), y(0.18)), (x(0.20), y(0.18)), (x(0.20), y(0.52))],
                false,
            ),
            mk(
                vec![(x(0.42), y(0.14)), (x(0.14), y(0.14)), (x(0.14), y(0.58))],
                false,
            ),
        ],
        // 方头括号
        '【' => vec![
            mk(
                vec![
                    (x(0.34), y(0.62)),
                    (x(0.20), y(0.62)),
                    (x(0.20), y(0.18)),
                    (x(0.34), y(0.18)),
                ],
                false,
            ),
            mk(
                vec![
                    (x(0.46), y(0.58)),
                    (x(0.28), y(0.58)),
                    (x(0.28), y(0.22)),
                    (x(0.46), y(0.22)),
                ],
                false,
            ),
        ],
        '】' => vec![
            mk(
                vec![
                    (x(0.20), y(0.62)),
                    (x(0.34), y(0.62)),
                    (x(0.34), y(0.18)),
                    (x(0.20), y(0.18)),
                ],
                false,
            ),
            mk(
                vec![
                    (x(0.34), y(0.58)),
                    (x(0.52), y(0.58)),
                    (x(0.52), y(0.22)),
                    (x(0.34), y(0.22)),
                ],
                false,
            ),
        ],
        // 弯引号
        '“' => vec![
            mk(vec![(x(0.20), y(0.40)), (x(0.28), y(0.52))], false),
            mk(vec![(x(0.34), y(0.40)), (x(0.42), y(0.52))], false),
        ],
        '”' => vec![
            mk(vec![(x(0.20), y(0.52)), (x(0.28), y(0.40))], false),
            mk(vec![(x(0.34), y(0.52)), (x(0.42), y(0.40))], false),
        ],
        '‘' => vec![mk(vec![(x(0.22), y(0.40)), (x(0.28), y(0.55))], false)],
        '’' => vec![mk(vec![(x(0.22), y(0.55)), (x(0.28), y(0.40))], false)],
        // 破折号
        '—' => vec![mk(vec![(x(0.04), y(0.34)), (x(0.96), y(0.34))], false)],
        // 省略号
        '…' => vec![
            dot(x(0.18), y(0.10), e * 0.05),
            dot(x(0.38), y(0.10), e * 0.05),
            dot(x(0.58), y(0.10), e * 0.05),
        ],
        // 间隔号
        '·' => vec![dot(x(0.5), y(0.34), e * 0.05)],
        _ => Vec::new(),
    }
}

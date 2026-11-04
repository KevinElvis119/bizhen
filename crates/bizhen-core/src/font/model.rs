//! 字体数据模型。
//!
//! 统一约定（各解析器都必须产出此格式，与 Python 版逐条一致）：
//! * 字形笔画坐标用**字体单位**，**Y 轴向上**，**基线 y=0**，排版原点 x=0；
//! * `advance` 是字形前进宽度（字体单位），决定下一个字符的起点；
//! * `units_per_em` 定义字体单位与 em 的换算：`scale = 字号(mm) / units_per_em`；
//! * 单个字形只有笔画（单线），不含填充，可直接给写字机。
//!
//! 这样排版只需 `scale = size_mm / units_per_em`，无需关心来源是
//! Hershey、中文单线 JSON 还是将来的 TrueType 转单线。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::geom::{BBox, Vec2};
use crate::stroke::{Role, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    Hershey,
    StrokeJson,
    /// kvenjoy `.gfont` 单线字体包（ZIP + 加密头）
    Gfont,
}

impl FontKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FontKind::Hershey => "hershey",
            FontKind::StrokeJson => "stroke-json",
            FontKind::Gfont => "gfont",
        }
    }
}

/// 一个字形：若干笔画（字体单位，Y 向上，基线 y=0，左边界 x=0）。
#[derive(Clone, Debug, Default)]
pub struct Glyph {
    pub strokes: Vec<Vec<Vec2>>,
    pub advance: f64,
}

impl Glyph {
    pub fn is_empty(&self) -> bool {
        self.strokes.iter().all(|s| s.is_empty())
    }

    pub fn bbox(&self) -> BBox {
        BBox::from_points(self.strokes.iter().flat_map(|s| s.iter().copied()))
    }

    /// 按 `scale` 缩放到页面单位并平移到 `origin`，产出带 `Role::Glyph` 的笔画。
    pub fn to_strokes(&self, scale: f64, origin: Vec2) -> Vec<Stroke> {
        let (ox, oy) = origin;
        self.strokes
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                Stroke::new(
                    s.iter()
                        .map(|(x, y)| (ox + x * scale, oy + y * scale))
                        .collect(),
                    false,
                    Role::Glyph,
                )
            })
            .collect()
    }
}

/// 一套字体的字形集合与度量。
#[derive(Clone, Debug)]
pub struct FontFamily {
    pub name: String,
    pub kind: FontKind,
    pub units_per_em: f64,
    pub glyphs: HashMap<char, Glyph>,
    pub source: PathBuf,
    /// 上伸/下延/平均步距（字体单位），由 [`FontFamily::recompute_metrics`] 统计得出。
    pub ascent: f64,
    pub descent: f64,
    pub default_advance: f64,
}

/// 分位数（q∈[0,1]，先排序取 `min(len-1, int(len*q))`）——与 Python 版逐位一致。
pub fn pct(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut s = values.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((s.len() as f64) * q) as usize;
    s[idx.min(s.len() - 1)]
}

impl FontFamily {
    pub fn new(name: impl Into<String>, kind: FontKind, units_per_em: f64) -> Self {
        Self {
            name: name.into(),
            kind,
            units_per_em,
            glyphs: HashMap::new(),
            source: PathBuf::new(),
            ascent: 0.0,
            descent: 0.0,
            default_advance: 0.0,
        }
    }

    pub fn has(&self, ch: char) -> bool {
        self.glyphs.contains_key(&ch)
    }

    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.get(&ch)
    }

    pub fn advance(&self, ch: char) -> f64 {
        self.glyphs
            .get(&ch)
            .map(|g| g.advance)
            .unwrap_or(self.default_advance)
    }

    pub fn coverage(&self) -> usize {
        self.glyphs.len()
    }

    /// 从字形统计度量（Python `FontFamily.recompute_metrics` 的逐位复刻）：
    /// 上伸/下延取 95%/5% 分位而非极值——手写字库常有零星超出字身的字形，
    /// 用 max 统计会把行高撑大一截。
    pub fn recompute_metrics(&mut self) {
        let mut tops: Vec<f64> = Vec::with_capacity(self.glyphs.len());
        let mut bottoms: Vec<f64> = Vec::with_capacity(self.glyphs.len());
        let mut total_adv = 0.0;
        let mut n = 0usize;
        for g in self.glyphs.values() {
            let b = g.bbox();
            if !b.is_empty() {
                tops.push(b.y1);
                bottoms.push(b.y0);
            }
            if g.advance != 0.0 {
                total_adv += g.advance;
                n += 1;
            }
        }
        let asc = pct(&tops, 0.95);
        let desc = pct(&bottoms, 0.05);
        self.ascent = if asc > 0.0 {
            asc
        } else {
            self.units_per_em * 0.8
        };
        self.descent = if desc < 0.0 {
            desc
        } else {
            -self.units_per_em * 0.2
        };
        let adv = if n > 0 { total_adv / n as f64 } else { 0.0 };
        self.default_advance = if adv > 0.0 {
            adv
        } else {
            self.units_per_em * 0.5
        };
    }

    /// 字号(mm，em 尺寸) → 字体单位到 mm 的缩放系数。
    pub fn scale_for_size(&self, size_mm: f64) -> f64 {
        if self.units_per_em <= 0.0 {
            1.0
        } else {
            size_mm / self.units_per_em
        }
    }

    /// 行高 = (上伸 - 下延) × scale × 行距倍数。
    pub fn line_height(&self, size_mm: f64, spacing: f64) -> f64 {
        (self.ascent - self.descent) * self.scale_for_size(size_mm) * spacing
    }
}

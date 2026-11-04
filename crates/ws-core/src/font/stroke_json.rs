//! 单线笔画字库解析器（`chinese-hershey-font` 的 `STRK-*.json`）。
//!
//! JSON 结构：`{"U+4E00": [[[x,y],...], ...], ...}`
//! 坐标为 **0..1 归一化、Y 轴向下**；本解析器映射为
//! **Y 轴向上、基线 y=0（方框下沿）、左边界 x=0**，`units_per_em = 1.0`。
//!
//! 来源：<https://github.com/LingDong-/chinese-hershey-font>（MIT）。
//!
//! 与原 Python 版的差异：Python 因为对象内存开销（117 MB）做了扁平化 + 字形
//! 懒转换；Rust 的 `Vec<Vec<Vec2>>` 本身只有 ~16 MB，直接建好更简单，因此这里
//! 解析一次建成 [`Glyph`]，**懒加载放在字体注册表层**（没人用到的字体不解析）。

use std::collections::HashMap;
use std::path::Path;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::font::jhf::FontError;
use crate::font::model::{pct, FontFamily, FontKind, Glyph};
use crate::geom::Vec2;

/// 解析结果：字符 → 字形。
type GlyphMap = HashMap<char, Glyph>;

struct FontVisitor;

impl<'de> Visitor<'de> for FontVisitor {
    type Value = GlyphMap;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("一个 {\"U+XXXX\": [[[x,y],...],...]} 结构")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut out: GlyphMap = HashMap::with_capacity(21000);
        while let Some(key) = map.next_key::<String>()? {
            let raw: Vec<Vec<(f64, f64)>> = map.next_value()?;
            let Some(ch) = parse_codepoint(&key) else {
                continue;
            };
            let strokes: Vec<Vec<Vec2>> = raw
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.into_iter().map(|(x, y)| (x, 1.0 - y)).collect())
                .collect();
            out.insert(
                ch,
                Glyph {
                    strokes,
                    advance: 1.0,
                },
            );
        }
        Ok(out)
    }
}

/// `"U+4E00"` → `'一'`；也接受单字符键。
fn parse_codepoint(key: &str) -> Option<char> {
    if let Some(hex) = key.strip_prefix("U+").or_else(|| key.strip_prefix("u+")) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    let mut it = key.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

fn deserialize_glyphs<'de, D: Deserializer<'de>>(d: D) -> Result<GlyphMap, D::Error> {
    d.deserialize_map(FontVisitor)
}

/// 从字符串解析（便于测试）。
pub fn parse_stroke_json_str(text: &str, name: &str) -> Result<FontFamily, FontError> {
    #[derive(Deserialize)]
    struct Root(#[serde(deserialize_with = "deserialize_glyphs")] GlyphMap);

    let root: Root = serde_json::from_str(text)
        .map_err(|e| FontError::Parse(format!("STRK JSON 解析失败：{e}")))?;
    let glyphs = root.0;
    if glyphs.is_empty() {
        return Err(FontError::NoGlyphs(name.to_string()));
    }
    let mut fam = FontFamily::new(name, FontKind::StrokeJson, 1.0);
    fam.glyphs = glyphs;
    // 与 Python 版同一套度量：上伸/下延取 95%/5% 分位，advance 恒为 1.0
    let mut tops: Vec<f64> = Vec::with_capacity(fam.glyphs.len());
    let mut bottoms: Vec<f64> = Vec::with_capacity(fam.glyphs.len());
    for g in fam.glyphs.values() {
        let b = g.bbox();
        if !b.is_empty() {
            tops.push(b.y1);
            bottoms.push(b.y0);
        }
    }
    let asc = pct(&tops, 0.95);
    let desc = pct(&bottoms, 0.05);
    fam.ascent = if asc > 0.0 { asc } else { 0.8 };
    fam.descent = if desc < 0.0 { desc } else { -0.2 };
    fam.default_advance = 1.0;
    Ok(fam)
}

/// 从文件解析 `STRK-*.json`。
pub fn parse_stroke_json(path: &Path, name: Option<&str>) -> Result<FontFamily, FontError> {
    let text = std::fs::read_to_string(path)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let mut fam = parse_stroke_json_str(&text, name.unwrap_or(&stem))?;
    fam.source = path.to_path_buf();
    Ok(fam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepoint_parsing() {
        assert_eq!(parse_codepoint("U+4E00"), Some('一'));
        assert_eq!(parse_codepoint("u+4e00"), Some('一'));
        assert_eq!(parse_codepoint("一"), Some('一'));
        assert_eq!(parse_codepoint("U+ZZZZ"), None);
    }

    #[test]
    fn tiny_json() {
        // 一个点 (0.25, 0.25) → Y 翻转后 (0.25, 0.75)
        let text = r#"{"U+4E00": [[[0.25, 0.25], [0.5, 0.5]]]}"#;
        let fam = parse_stroke_json_str(text, "tiny").unwrap();
        let g = fam.glyph('一').unwrap();
        assert_eq!(g.strokes.len(), 1);
        assert_eq!(g.strokes[0][0], (0.25, 0.75));
        assert_eq!(g.advance, 1.0);
        assert_eq!(fam.units_per_em, 1.0);
    }

    #[test]
    fn parses_builtin_kaiti() {
        let path = crate::fonts_dir()
            .join("stroke-json")
            .join("STRK-Kaiti.json");
        let t0 = std::time::Instant::now();
        let fam = parse_stroke_json(&path, None).expect("STRK-Kaiti.json 应能解析");
        let dt = t0.elapsed();
        assert_eq!(fam.coverage(), 20976, "字形数应与原版一致");
        assert!(fam.has('永'));
        // em = 1.0，汉字几乎占满字身，95% 分位通常就是 1.0
        assert!(
            fam.ascent > 0.5 && fam.ascent <= 1.0,
            "ascent={}",
            fam.ascent
        );
        assert!(fam.descent <= 0.0, "descent={}", fam.descent);
        eprintln!(
            "STRK-Kaiti: {} 字形, ascent {:.4}, descent {:.4}, 解析 {} ms",
            fam.coverage(),
            fam.ascent,
            fam.descent,
            dt.as_millis()
        );
        // 解析应在可接受范围内（调试构造成本高，放宽到 3 s）
        assert!(dt.as_secs_f64() < 3.0, "解析耗时 {:?}", dt);
    }
}

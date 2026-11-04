//! Hershey 单线字体解析器（`.jhf`）。
//!
//! 格式（USENET / James Hurt 的 "fixed" jhf）：
//! * 每条记录 = 5 位字形编号 + 3 位字符数 + 数据；
//! * 数据 = 左边界 + 右边界 + 若干 (x,y) 点对，全部以 `'R'`(0x52) 为原点偏移；
//! * 点对 `(-50, 0)` 表示**抬笔**（笔画分隔）；
//! * 字形按**位置**映射到字符：`chr(32 + 序号)`。
//!
//! 原始坐标 **Y 轴向下**、基线在 `y=9`（大写范围 -12..9，大写高 21 单位），
//! 本解析器统一输出 **Y 轴向上、基线 y=0、左边界 x=0**。
//!
//! 许可：Hershey 字体可自由用于任何目的（含商业），须随数据保留对
//! Dr. A. V. Hershey (NBS) 与 James Hurt 的致谢，见 `assets/fonts/hershey/ACKNOWLEDGEMENT.txt`。

use std::path::Path;

use crate::font::model::{FontFamily, FontKind, Glyph};

const OFFSET: i32 = b'R' as i32; // 82
const PEN_UP: (i32, i32) = (-50, 0);
const HERSHEY_BASELINE: i32 = 9;
/// 名义 em（大写高 21 / em ≈ 0.66）
const HERSHEY_EM: f64 = 32.0;

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("读取字体文件失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("未能从 {0} 解析出任何字形")]
    NoGlyphs(String),
    #[error("{0}")]
    Parse(String),
}

/// 原始 int 单位的笔画集合（每笔一串点）。
type RawStrokes = Vec<Vec<(i32, i32)>>;

/// 把一条记录解码为 (left, right, strokes)，坐标为原始 int 单位。
fn decode_record(rec: &[u8]) -> Option<(i32, i32, RawStrokes)> {
    if rec.len() < 8 {
        return None;
    }
    let length: usize = std::str::from_utf8(&rec[5..8]).ok()?.trim().parse().ok()?;
    let body = &rec[8..];
    if body.len() != length * 2 {
        return None;
    }
    let left = body[0] as i32 - OFFSET;
    let right = body[1] as i32 - OFFSET;
    let data = &body[2..];
    let mut strokes: Vec<Vec<(i32, i32)>> = Vec::new();
    let mut cur: Vec<(i32, i32)> = Vec::new();
    for pair in data.chunks_exact(2) {
        let p = (pair[0] as i32 - OFFSET, pair[1] as i32 - OFFSET);
        if p == PEN_UP {
            if !cur.is_empty() {
                strokes.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(p);
        }
    }
    if !cur.is_empty() {
        strokes.push(cur);
    }
    Some((left, right, strokes))
}

fn is_header(line: &[u8]) -> bool {
    line.len() >= 8
        && line[..5].iter().all(|b| b.is_ascii_digit() || *b == b' ')
        && line[..5].iter().any(|b| b.is_ascii_digit())
        && line[5..8].iter().all(|b| b.is_ascii_digit() || *b == b' ')
        && line[5..8].iter().any(|b| b.is_ascii_digit())
}

/// 按记录头（5+3 位数字）切分，跨行的数据拼回同一条记录。
fn split_records(text: &[u8]) -> Vec<Vec<u8>> {
    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    for raw in text.split(|b| *b == b'\n') {
        let mut line = raw;
        if let Some(stripped) = line.strip_suffix(b"\r") {
            line = stripped;
        }
        if is_header(line) {
            if !buf.is_empty() {
                records.push(std::mem::take(&mut buf));
            }
            buf.extend_from_slice(line);
        } else if !buf.is_empty() {
            buf.extend_from_slice(line);
        }
    }
    if !buf.is_empty() {
        records.push(buf);
    }
    records
}

/// 解析 `.jhf` 字节为 [`FontFamily`]。
pub fn parse_hershey_bytes(text: &[u8], name: &str) -> Result<FontFamily, FontError> {
    let records = split_records(text);
    let mut fam = FontFamily::new(name, FontKind::Hershey, HERSHEY_EM);
    for (idx, rec) in records.iter().enumerate() {
        let Some((left, right, raw_strokes)) = decode_record(rec) else {
            continue;
        };
        let Some(ch) = char::from_u32(32 + idx as u32) else {
            continue;
        };
        let strokes: Vec<Vec<(f64, f64)>> = raw_strokes
            .iter()
            .map(|s| {
                s.iter()
                    .map(|(x, y)| ((x - left) as f64, (HERSHEY_BASELINE - y) as f64))
                    .collect()
            })
            .collect();
        fam.glyphs.insert(
            ch,
            Glyph {
                strokes,
                advance: (right - left) as f64,
            },
        );
    }
    if fam.glyphs.is_empty() {
        return Err(FontError::NoGlyphs(name.to_string()));
    }
    fam.recompute_metrics();
    Ok(fam)
}

/// 从文件解析 `.jhf`。
pub fn parse_hershey_jhf(path: &Path, name: Option<&str>) -> Result<FontFamily, FontError> {
    let bytes = std::fs::read(path)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let mut fam = parse_hershey_bytes(&bytes, name.unwrap_or(&stem))?;
    fam.source = path.to_path_buf();
    Ok(fam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_detection() {
        assert!(is_header(b"12345 10ABC"));
        assert!(!is_header(b"abcde 10ABC"));
        assert!(!is_header(b"1234"));
    }

    #[test]
    fn parse_builtin_futural() {
        let path = crate::fonts_dir().join("hershey").join("futural.jhf");
        let fam = parse_hershey_jhf(&path, None).expect("futural.jhf 应能解析");
        assert!(fam.coverage() > 90, "字形数 {}", fam.coverage());
        assert!(fam.has('A'));
        assert!(fam.ascent > 0.0);
        // 大写 A 的墨迹高度应接近 21 字体单位（Hershey 大写高）
        let b = fam.glyph('A').unwrap().bbox();
        assert!((b.height() - 21.0).abs() < 1.5, "A 高度 {}", b.height());
    }

    #[test]
    fn metrics_are_percentiles() {
        let path = crate::fonts_dir().join("hershey").join("rowmand.jhf");
        let fam = parse_hershey_jhf(&path, None).unwrap();
        assert!(fam.line_height(4.0, 1.5) > 0.0);
        assert!(fam.default_advance > 0.0);
    }
}

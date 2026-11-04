//! `.gfont` 单线字体解析器（kvenjoy 格式）。
//!
//! 文件结构（与 Python 原版逐条对应）：
//!
//! ```text
//! ┌ 文件头 ────────────────────────────────────────────┐
//! │ u32 version(5..9) │ u32 加密元数据块长度            │
//! │ 加密块：XTEA(16 循环)+CBC 变体，密钥 "1989082619920828" │
//! │   解密后（大端）：作者/类别/名称/副名/描述/默认字号/字形数， │
//! │   v≥2 版本串，v≥7 uuid，v≥9 六个度量浮点              │
//! └───────────────────────────────────────────────────┘
//! ┌ 标准 ZIP 归档 ─────────────────────────────────────┐
//! │ 条目名 = 字符的 Unicode 码点十进制串                  │
//! │ 内容（大端）：u16 码点 │ u32 n(=2×点数) │ f32[n] 坐标 │
//! │              u32 点数 │ u8[点数] 抬落笔标志           │
//! └───────────────────────────────────────────────────┘
//! ```
//!
//! 坐标系原始数据 **Y 轴向下**，墨迹各自相对原点摆放；本解析器统一转成
//! **Y 轴向上、基线 y=0、左边缘 = 类别左边距**。
//!
//! 度量推断：`.gfont` 只存墨迹、**不含任何步距/边距**，所以按字符类别推断
//! （表意文字取墨迹宽的 97 分位当字身；纯西文/符号库改用大写字高锚定
//! cap ≈ 0.70 em；标点/括号按类别收小；扁平横画「一」单独收窄）。
//!
//! 尚未移植：原版对「口/回/国」这类盒形字的分级收小（几何特征检测），
//! 当前版本这类字会略偏大，见 docs/CONTEXT.md 的待办。

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::font::jhf::FontError;
use crate::font::model::{pct, FontFamily, FontKind, Glyph};
use crate::geom::Vec2;

// ---------------------------------------------------------------------------
// 文件头（XTEA 解密的元数据）
// ---------------------------------------------------------------------------
const KEY: [u32; 16] = [1, 9, 8, 9, 0, 8, 2, 6, 1, 9, 9, 2, 0, 8, 2, 8];
const DELTA: u32 = 0x9E37_79B9;

/// 字体元数据（列表里展示作者/名称/字形数）。
#[derive(Clone, Debug, Default)]
pub struct GfontMeta {
    pub version: u32,
    pub author: String,
    pub name: String,
    pub sub: String,
    pub desc: String,
    pub default_size: i32,
    pub glyph_count: i32,
}

fn key_words() -> [u32; 4] {
    let mut kw = [0u32; 4];
    for (i, slot) in kw.iter_mut().enumerate() {
        let b = i * 4;
        *slot = (KEY[b] << 24) | (KEY[b + 1] << 16) | (KEY[b + 2] << 8) | KEY[b + 3];
    }
    kw
}

/// XTEA 两轮解密 ×16 循环（原版 `_xtea_dec`）。
fn xtea_dec(v0_in: u32, v1_in: u32) -> [u8; 8] {
    let kw = key_words();
    let mut v0 = v0_in;
    let mut v1 = v1_in;
    let mut s = DELTA << 4;
    for _ in 0..16 {
        v1 = v1.wrapping_sub(
            (((v0 << 4).wrapping_add(kw[2])) ^ (v0.wrapping_add(s)))
                ^ ((v0 >> 5).wrapping_add(kw[3])),
        );
        v0 = v0.wrapping_sub(
            (((v1 << 4).wrapping_add(kw[0])) ^ (v1.wrapping_add(s)))
                ^ ((v1 >> 5).wrapping_add(kw[1])),
        );
        s = s.wrapping_sub(DELTA);
    }
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&v0.to_be_bytes());
    out[4..].copy_from_slice(&v1.to_be_bytes());
    out
}

fn decrypt_block(mixed: &[u8]) -> [u8; 8] {
    let v0 = u32::from_be_bytes([mixed[0], mixed[1], mixed[2], mixed[3]]);
    let v1 = u32::from_be_bytes([mixed[4], mixed[5], mixed[6], mixed[7]]);
    xtea_dec(v0, v1)
}

/// 解密 v5+ 的加密元数据块（含 CBC 变体与首尾填充），返回明文。
fn decrypt_header(enc: &[u8]) -> Vec<u8> {
    let n8 = enc.len() / 8;
    if n8 < 1 {
        return Vec::new();
    }
    let data = &enc[..n8 * 8];
    let mut e = decrypt_block(&data[0..8]);
    let pad = (e[0] & 7) as usize;
    let plain_len = (n8 * 8).saturating_sub(1 + pad + 2 + 7);
    let mut prev = [0u8; 8];
    let mut off = 8usize;
    let mut idx = pad + 1;

    let step = |e: &mut [u8; 8], off: &mut usize, idx: &mut usize, prev: &mut [u8; 8]| {
        if *off + 8 <= data.len() {
            prev.copy_from_slice(&data[*off - 8..*off]);
            let mut mixed = [0u8; 8];
            for i in 0..8 {
                mixed[i] = e[i] ^ data[*off + i];
            }
            *e = decrypt_block(&mixed);
            *off += 8;
            *idx = 0;
        } else {
            *idx = 8;
        }
    };

    // 阶段 1：跳过 2 字节头部填充
    let mut cnt = 1;
    while cnt <= 2 {
        if idx < 8 {
            idx += 1;
            cnt += 1;
        } else {
            step(&mut e, &mut off, &mut idx, &mut prev);
        }
    }
    // 阶段 2：输出明文
    let mut out = Vec::with_capacity(plain_len);
    let mut remaining = plain_len;
    while remaining > 0 {
        if idx < 8 {
            out.push(e[idx] ^ prev[idx]);
            idx += 1;
            remaining -= 1;
        } else {
            step(&mut e, &mut off, &mut idx, &mut prev);
        }
    }
    out
}

/// Java `DataInput.readUTF`：u16 长度 + 修改版 UTF-8。
fn read_utf(buf: &[u8], off: &mut usize) -> String {
    if *off + 2 > buf.len() {
        *off = buf.len();
        return String::new();
    }
    let ln = u16::from_be_bytes([buf[*off], buf[*off + 1]]) as usize;
    *off += 2;
    let end = (*off + ln).min(buf.len());
    let raw = &buf[*off..end];
    *off = end;
    let fixed: Vec<u8> = raw
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| {
            if w == [0xC0, 0x80] {
                Some(0u8)
            } else if i == 0 || raw[i - 1] != 0xC0 {
                Some(raw[i])
            } else {
                None
            }
        })
        .collect();
    String::from_utf8_lossy(&fixed).into_owned()
}

/// 读取加密头里的字体元数据；失败时各字段为空（不报错）。
pub fn gfont_metadata(path: &Path) -> GfontMeta {
    let mut meta = GfontMeta::default();
    let Ok(bytes) = std::fs::read(path) else {
        return meta;
    };
    if bytes.len() < 8 {
        return meta;
    }
    let ver = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let enc_len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    meta.version = ver;
    if ver < 5 || enc_len == 0 || enc_len > bytes.len() {
        return meta;
    }
    let plain = decrypt_header(&bytes[8..8 + enc_len]);
    let mut off = 0usize;
    meta.author = read_utf(&plain, &mut off);
    if off + 4 > plain.len() {
        return meta;
    }
    off += 4; // category
    meta.name = read_utf(&plain, &mut off);
    meta.sub = read_utf(&plain, &mut off);
    meta.desc = read_utf(&plain, &mut off);
    if off + 8 <= plain.len() {
        meta.default_size =
            i32::from_be_bytes([plain[off], plain[off + 1], plain[off + 2], plain[off + 3]]);
        meta.glyph_count = i32::from_be_bytes([
            plain[off + 4],
            plain[off + 5],
            plain[off + 6],
            plain[off + 7],
        ]);
    }
    // 损坏文件解出的是乱码：含替换符/控制符就清空，让上层回退到文件名
    for s in [
        &mut meta.author,
        &mut meta.name,
        &mut meta.sub,
        &mut meta.desc,
    ] {
        if s.contains('\u{FFFD}') || s.chars().any(|c| c < ' ' || c == '\u{7F}') {
            s.clear();
        }
    }
    meta
}

// ---------------------------------------------------------------------------
// 字形记录
// ---------------------------------------------------------------------------
/// 解析一条字形记录 → (码点, 笔画[原始坐标], 包围盒[原始坐标])。
fn parse_record(data: &[u8]) -> Option<(u32, Vec<Vec<Vec2>>, (f64, f64, f64, f64))> {
    if data.len() < 10 {
        return None;
    }
    let cp = u16::from_be_bytes([data[0], data[1]]) as u32;
    let n = u32::from_be_bytes([data[2], data[3], data[4], data[5]]) as usize;
    if n == 0 || n % 2 != 0 {
        return None;
    }
    let end = 6 + n * 4;
    if end + 4 > data.len() {
        return None;
    }
    let pc = u32::from_be_bytes([data[end], data[end + 1], data[end + 2], data[end + 3]]) as usize;
    if pc != n / 2 {
        return None;
    }
    let flags = data.get(end + 4..end + 4 + pc)?;
    let mut vals: Vec<f64> = Vec::with_capacity(n);
    for i in 0..n {
        let b = 6 + i * 4;
        vals.push(f32::from_be_bytes([data[b], data[b + 1], data[b + 2], data[b + 3]]) as f64);
    }
    let mut bx0 = f64::INFINITY;
    let mut by0 = f64::INFINITY;
    let mut bx1 = f64::NEG_INFINITY;
    let mut by1 = f64::NEG_INFINITY;
    for i in 0..pc {
        let (x, y) = (vals[i * 2], vals[i * 2 + 1]);
        bx0 = bx0.min(x);
        by0 = by0.min(y);
        bx1 = bx1.max(x);
        by1 = by1.max(y);
    }
    let mut strokes: Vec<Vec<Vec2>> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for i in 0..pc {
        let p = (vals[i * 2], vals[i * 2 + 1]);
        if flags[i] == 0 && !cur.is_empty() {
            strokes.push(std::mem::take(&mut cur));
        }
        cur.push(p);
    }
    if !cur.is_empty() {
        strokes.push(cur);
    }
    if strokes.is_empty() {
        return None;
    }
    Some((cp, strokes, (bx0, by0, bx1, by1)))
}

/// ZIP 条目名 → 码点（十进制；v3+ 补增平面用 `55357_56832` 形式的代理对）。
fn entry_codepoint(name: &str) -> Option<char> {
    let name = name.trim_end_matches('/');
    if !name.contains('_') {
        let cp: u32 = name.parse().ok()?;
        return valid(cp);
    }
    let units: Vec<u16> = name
        .split('_')
        .map(|p| p.parse::<u16>().ok())
        .collect::<Option<Vec<_>>>()?;
    if units.is_empty() || units.len() > 4 {
        return None;
    }
    let mut buf: Vec<u8> = Vec::with_capacity(units.len() * 2);
    for u in units {
        buf.extend_from_slice(&u.to_be_bytes());
    }
    let s = String::from_utf16(
        &buf.chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    )
    .ok()?;
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => valid(c as u32),
        _ => None,
    }
}

fn valid(cp: u32) -> Option<char> {
    if cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
        return None;
    }
    char::from_u32(cp)
}

// ---------------------------------------------------------------------------
// 度量启发式（原版 `parse_gfont` 的移植）
// ---------------------------------------------------------------------------
fn is_fullwidth(cp: u32) -> bool {
    const RANGES: &[(u32, u32)] = &[
        (0x1100, 0x115F),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE30, 0xFE4F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x20000, 0x3FFFD),
    ];
    RANGES.iter().any(|(lo, hi)| (*lo..=*hi).contains(&cp))
}

/// 字形类别 → (左边距, 右边距)（em 单位）
fn class_bearings(class: &str) -> (f64, f64) {
    match class {
        "cjk" => (0.07, 0.07),
        "punct-small" => (0.04, 0.14),
        "punct-colon" => (0.06, 0.14),
        "punct-tall" => (0.08, 0.08),
        "fw-bracket" => (0.06, 0.06),
        "fw-latin" => (0.06, 0.06),
        "ascii" => (0.06, 0.06),
        _ => (0.05, 0.05),
    }
}

fn glyph_class(cp: u32) -> &'static str {
    if is_fullwidth(cp) {
        match cp {
            0x3001 | 0x3002 | 0xFF0C | 0xFF0E | 0xFF61 | 0xFF64 => "punct-small",
            0xFF1A | 0xFF1B => "punct-colon",
            0xFF01 | 0xFF1F => "punct-tall",
            0xFF01..=0xFF5E => "fw-latin",
            0x3008..=0x3011 | 0x300C..=0x301F | 0xFF5F | 0xFF60 => "fw-bracket",
            _ => "cjk",
        }
    } else if (0x20..=0x7E).contains(&cp) {
        "ascii"
    } else {
        "other"
    }
}

const EMPTY_FRAME_W: f64 = 0.75;
const EMPTY_FRAME_H: f64 = 0.80;
const FRAME_W: f64 = 0.86;
const FRAME_H: f64 = 0.92;
const FLAT_CJK_H: f64 = 0.30;
const FLAT_HZ_CAP: f64 = 0.60;

/// 括号/全角符号的尺寸归一类别
fn symbol_kind(cp: u32) -> Option<&'static str> {
    let ch = char::from_u32(cp)?;
    let kind = match ch {
        '(' | ')' => "round",
        '[' | ']' => "square",
        '{' | '}' => "curl",
        '<' | '>' => "angle",
        _ => {
            const WIDE: &str = "（）［］｛｝｟｠〈〉《》「」『』【】〔〕〖〗〘〙〚〛﹙﹚﹛﹜﹝﹞";
            if WIDE.contains(ch) || (0xFFE0..=0xFFE6).contains(&cp) {
                "wide"
            } else {
                return None;
            }
        }
    };
    Some(kind)
}

fn symbol_size(kind: &str) -> (f64, f64) {
    match kind {
        "round" => (1.22, 1.42),
        "square" => (1.12, 1.32),
        "curl" => (1.35, 1.50),
        "angle" => (1.02, 1.25),
        _ => (1.15, 1.22),
    }
}

fn punct_size(class: &str) -> Option<(f64, f64)> {
    match class {
        "punct-small" => Some((0.24, 0.34)),
        "punct-colon" => Some((0.55, 0.72)),
        "punct-tall" => Some((1.02, 1.15)),
        _ => None,
    }
}

/// 个别「写大了」的字形收小系数（只缩不放）。
fn size_norm_scale(cp: u32, ink_w: f64, ink_h: f64, cap: f64, em: f64, cjk_h: f64) -> f64 {
    if let Some(kind) = symbol_kind(cp) {
        let mut reference = cap;
        if kind == "wide" && cjk_h > 0.0 {
            reference = cjk_h;
        }
        if reference <= 0.0 {
            reference = 0.70 * em;
        }
        let (target, limit) = symbol_size(kind);
        if ink_h > limit * reference {
            return target * reference / ink_h;
        }
        return 1.0;
    }
    let class = glyph_class(cp);
    if class == "cjk" {
        if ink_h < FLAT_CJK_H * em {
            if ink_w > FLAT_HZ_CAP * em {
                return FLAT_HZ_CAP * em / ink_w;
            }
            return 1.0;
        }
        if ink_w > 0.90 * em {
            return 0.90 * em / ink_w;
        }
        return 1.0;
    }
    if let Some((target, limit)) = punct_size(class) {
        let cap = if cap > 0.0 { cap } else { 0.70 * em };
        if ink_h > limit * cap {
            return target * cap / ink_h;
        }
    }
    1.0
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------
/// 解析 `.gfont` 为 [`FontFamily`]。
pub fn parse_gfont(path: &Path, name: Option<&str>) -> Result<FontFamily, FontError> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| FontError::Parse(format!(".gfont ZIP 结构无效：{e}")))?;

    // 1) 读全部字形（码点 → 笔画 / 包围盒）
    let mut records: Vec<(u32, Vec<Vec<Vec2>>, (f64, f64, f64, f64))> = Vec::new();
    let mut seen: HashMap<char, usize> = HashMap::new();
    for i in 0..zip.len() {
        let Ok(mut entry) = zip.by_index(i) else {
            continue;
        };
        let entry_name = entry.name().to_string();
        let Some(ch) = entry_codepoint(&entry_name) else {
            continue;
        };
        if seen.contains_key(&ch) {
            continue;
        }
        let mut data = Vec::new();
        if entry.read_to_end(&mut data).is_err() {
            continue;
        }
        let Some((cp, strokes, bbox)) = parse_record(&data) else {
            continue;
        };
        seen.insert(ch, records.len());
        records.push((cp, strokes, bbox));
    }
    if records.is_empty() {
        return Err(FontError::NoGlyphs(path.display().to_string()));
    }

    // 2) 统计字身（表意文字优先）
    let full_w: Vec<f64> = records
        .iter()
        .filter(|(cp, _, _)| is_fullwidth(*cp))
        .map(|(_, _, b)| b.2 - b.0)
        .collect();
    let full_top: Vec<f64> = records
        .iter()
        .filter(|(cp, _, _)| is_fullwidth(*cp))
        .map(|(_, _, b)| b.1)
        .collect();
    let full_bottom: Vec<f64> = records
        .iter()
        .filter(|(cp, _, _)| is_fullwidth(*cp))
        .map(|(_, _, b)| b.3)
        .collect();
    let all_w: Vec<f64> = records.iter().map(|(_, _, b)| b.2 - b.0).collect();
    let all_top: Vec<f64> = records.iter().map(|(_, _, b)| b.1).collect();
    let all_bottom: Vec<f64> = records.iter().map(|(_, _, b)| b.3).collect();

    let (pitch, baseline_raw, cell_h) = if !full_w.is_empty() {
        let pitch = pct(&full_w, 0.97);
        let baseline = pct(&full_bottom, 0.90);
        (pitch, baseline, baseline - pct(&full_top, 0.05))
    } else {
        let pitch = pct(&all_w, 0.95);
        let baseline = pct(&all_bottom, 0.90);
        (pitch, baseline, baseline - pct(&all_top, 0.05))
    };
    let mut upem = if pitch > 0.0 { pitch } else { cell_h };

    let cap_h = {
        let hs: Vec<f64> = records
            .iter()
            .filter(|(cp, _, b)| (0x41..=0x5A).contains(cp) && b.3 > b.1)
            .map(|(_, _, b)| b.3 - b.1)
            .collect();
        if hs.len() >= 4 {
            median(hs)
        } else {
            0.0
        }
    };
    let _med_w = {
        let ws: Vec<f64> = records
            .iter()
            .filter(|(cp, _, b)| glyph_class(*cp) == "cjk" && b.2 > b.0)
            .map(|(_, _, b)| b.2 - b.0)
            .collect();
        if ws.len() >= 2 {
            median(ws)
        } else {
            0.0
        }
    };
    let med_h = {
        let hs: Vec<f64> = records
            .iter()
            .filter(|(cp, _, b)| glyph_class(*cp) == "cjk" && b.3 > b.1)
            .map(|(_, _, b)| b.3 - b.1)
            .collect();
        if hs.len() >= 2 {
            median(hs)
        } else {
            0.0
        }
    };
    if med_h <= 0.0 && cap_h > 0.0 {
        upem = cap_h / 0.70;
    }
    if upem <= 0.0 {
        upem = 100.0;
    }

    // 3) 逐字形定型：收缩系数 / 左对齐偏移 / 步距
    let display = match name {
        Some(n) => n.to_string(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    };
    let mut fam = FontFamily::new(display, FontKind::Gfont, 1.0);
    fam.units_per_em = upem;
    fam.source = path.to_path_buf();

    let mut tops: Vec<f64> = Vec::new();
    let mut bottoms: Vec<f64> = Vec::new();
    let mut total_adv = 0.0;
    let mut count = 0usize;

    for (ch, idx) in &seen {
        let (cp, strokes, b) = &records[*idx];
        let (x0, y0, x1, y1) = *b;
        let ink_w = x1 - x0;
        let ink_h = y1 - y0;
        let is_frame = false; // TODO：盒形字分级（口/回/国）尚未移植
        let _ = (EMPTY_FRAME_W, EMPTY_FRAME_H, FRAME_W, FRAME_H, is_frame);

        let s = size_norm_scale(*cp, ink_w, ink_h, cap_h, upem, med_h);
        let class = glyph_class(*cp);
        let (lb, rb) = class_bearings(class);

        let advance = if *cp == 0x20 {
            upem * 0.48
        } else if *cp == 0x3000 {
            upem
        } else {
            let mut a = ink_w * s + (lb + rb) * upem;
            if matches!(class, "ascii" | "fw-latin" | "other") {
                a = a.max(upem * 0.30);
            }
            a
        };
        let dx = lb * upem - x0 * s;

        // 转到项目坐标系：Y 向上、基线 y=0、左缘 = 左边距
        let new_strokes: Vec<Vec<Vec2>> = strokes
            .iter()
            .map(|st| {
                st.iter()
                    .map(|(x, y)| (x * s + dx, (baseline_raw - y) * s))
                    .collect()
            })
            .collect();

        let gb = Glyph {
            strokes: new_strokes,
            advance,
        };
        // 度量统计用字形墨迹上下缘
        let bb = gb.bbox();
        if !bb.is_empty() {
            tops.push(bb.y1);
            bottoms.push(bb.y0);
        }
        fam.glyphs.insert(*ch, gb);
        total_adv += advance;
        count += 1;
    }

    // 缺空格时合成（半角宽）
    if let std::collections::hash_map::Entry::Vacant(e) = fam.glyphs.entry(' ') {
        e.insert(Glyph {
            strokes: Vec::new(),
            advance: upem * 0.48,
        });
        total_adv += upem * 0.48;
        count += 1;
    }

    fam.ascent = {
        let a = pct(&tops, 0.95);
        if a > 0.0 {
            a
        } else {
            upem * 0.8
        }
    };
    fam.descent = {
        let d = pct(&bottoms, 0.05);
        if d < 0.0 {
            d
        } else {
            -upem * 0.2
        }
    };
    fam.default_advance = if count > 0 && total_adv > 0.0 {
        total_adv / count as f64
    } else {
        upem * 0.5
    };
    Ok(fam)
}

/// 只读 ZIP 目录统计字形数（不解压任何条目）。
pub fn count_gfont_glyphs(path: &Path) -> i64 {
    let Ok(file) = std::fs::File::open(path) else {
        return -1;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return -1;
    };
    let mut seen: std::collections::HashSet<char> = std::collections::HashSet::new();
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index(i) {
            if let Some(ch) = entry_codepoint(entry.name()) {
                seen.insert(ch);
            }
        }
    }
    if !seen.contains(&' ') {
        seen.insert(' ');
    }
    seen.len() as i64
}

/// 显示名：优先加密头里的真实名称。
pub fn display_name(path: &Path) -> String {
    let meta = gfont_metadata(path);
    if !meta.name.is_empty() {
        return meta.name;
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepoint_from_entry_names() {
        assert_eq!(entry_codepoint("20013"), Some('中'));
        assert_eq!(entry_codepoint("55357_56832"), Some('😀'));
        assert_eq!(entry_codepoint("not-a-number"), None);
        assert_eq!(entry_codepoint("55296"), None); // 孤立代理区
    }

    #[test]
    fn class_and_bearings() {
        assert_eq!(glyph_class(0x4E2D), "cjk");
        assert_eq!(glyph_class(0x41), "ascii");
        assert_eq!(glyph_class(0xFF0C), "punct-small");
        assert_eq!(class_bearings("cjk"), (0.07, 0.07));
    }

    #[test]
    fn parses_real_gfont_if_present() {
        // 本机若放了 .gfont 才跑（仓库里没有该格式的授权样本）
        let dir = crate::font::registry::user_font_dir();
        let Some(path) = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.extension().map(|x| x == "gfont").unwrap_or(false))
        }) else {
            eprintln!("跳过：{} 里没有 .gfont 样本", dir.display());
            return;
        };
        let fam = parse_gfont(&path, None).expect(".gfont 应能解析");
        assert!(fam.coverage() > 10, "字形数 {}", fam.coverage());
        assert!(fam.units_per_em > 0.0);
        eprintln!(
            "{}: {} 字形, upem {:.2}, ascent {:.2}, descent {:.2}",
            path.display(),
            fam.coverage(),
            fam.units_per_em,
            fam.ascent,
            fam.descent
        );
    }
}

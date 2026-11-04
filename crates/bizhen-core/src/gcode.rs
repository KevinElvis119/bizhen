//! G-code 生成：笔画 → GRBL 可执行指令。
//!
//! 流程：笔画 →（可选）抽稀 → 笔序规划 → 抬落笔 + 直线插补 → G-code 行。
//!
//! 坐标：页面 mm、**Y 轴向上**，与 GRBL 机械坐标一致，**不做 Y 翻转**；
//! 机器坐标 = `page_to_machine`（机械原点 = 纸张左上角）+ 起点偏移。
//!
//! 这是 `machine/gcode_gen.py` + `machine/config.py` 的移植，默认参数保持一致。

use std::fmt::Write as _;

use crate::geom::{rdp_simplify, Vec2};
use crate::stroke::Stroke;

/// 抬落笔方式（按驱动形式分类）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenMode {
    /// 步进 Z 轴
    Z,
    /// 舵机角度（M3 S<角度>）
    ServoAngle,
    /// 激光（M3 S<功率> / M5）
    Laser,
    /// 自定义指令
    Custom,
    /// 主轴 M3/M5
    M3M5,
    /// 冷却 M7/M9
    M7M9,
}

impl PenMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PenMode::Z => "z",
            PenMode::ServoAngle => "servo-angle",
            PenMode::Laser => "laser",
            PenMode::Custom => "custom",
            PenMode::M3M5 => "m3m5",
            PenMode::M7M9 => "m7m9",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "servo-angle" => PenMode::ServoAngle,
            "laser" => PenMode::Laser,
            "custom" => PenMode::Custom,
            "m3m5" => PenMode::M3M5,
            "m7m9" => PenMode::M7M9,
            _ => PenMode::Z,
        }
    }

    pub const ALL: [PenMode; 6] = [
        PenMode::Z,
        PenMode::ServoAngle,
        PenMode::Laser,
        PenMode::Custom,
        PenMode::M3M5,
        PenMode::M7M9,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PenMode::Z => "步进 Z 轴（Stepper）",
            PenMode::ServoAngle => "舵机角度（Servo）",
            PenMode::Laser => "激光（Laser）",
            PenMode::Custom => "自定义指令（Custom）",
            PenMode::M3M5 => "主轴 M3/M5（S 值）",
            PenMode::M7M9 => "冷却 M7/M9",
        }
    }
}

/// 内容哪个角对齐机器原点
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginCorner {
    None,
    LeftTop,
    RightTop,
    LeftBottom,
    RightBottom,
}

impl OriginCorner {
    pub fn as_str(&self) -> &'static str {
        match self {
            OriginCorner::None => "none",
            OriginCorner::LeftTop => "left-top",
            OriginCorner::RightTop => "right-top",
            OriginCorner::LeftBottom => "left-bottom",
            OriginCorner::RightBottom => "right-bottom",
        }
    }
    pub const ALL: [OriginCorner; 5] = [
        OriginCorner::None,
        OriginCorner::LeftTop,
        OriginCorner::RightTop,
        OriginCorner::LeftBottom,
        OriginCorner::RightBottom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            OriginCorner::None => "不用（手动起点）",
            OriginCorner::LeftTop => "左上角 = 原点",
            OriginCorner::RightTop => "右上角 = 原点",
            OriginCorner::LeftBottom => "左下角 = 原点",
            OriginCorner::RightBottom => "右下角 = 原点",
        }
    }
}

/// G-code 输出配置（默认值与原版一致）。
#[derive(Clone, Debug)]
pub struct GCodeConfig {
    pub pen_mode: PenMode,
    pub pen_up_z: f64,
    pub pen_down_z: f64,
    pub pen_down_s: i32,
    pub pen_up_s: i32,
    pub pen_down_angle: i32,
    pub laser_power: i32,
    pub pen_up_command: String,
    pub pen_down_command: String,
    pub pen_on_delay: f64,
    pub pen_off_delay: f64,
    pub draw_feed: f64,
    pub travel_feed: f64,
    pub pen_z_feed: f64,
    pub custom_start_gcode: String,
    pub custom_end_gcode: String,
    pub return_to_start: bool,
    pub swap_xy: bool,
    pub invert_x: bool,
    pub invert_y: bool,
    pub origin_corner: OriginCorner,
    pub simplify_mm: f64,
    pub stroke_taper_mm: f64,
    pub entry_taper_mm: f64,
    pub decimals: usize,
    pub feed_decimals: usize,
    pub header: Vec<String>,
    pub footer: Vec<String>,
    pub include_comments: bool,
    /// 书写顺序：true = 阅读顺序（按分组），false = 最短空程
    pub reading_order: bool,
}

impl Default for GCodeConfig {
    fn default() -> Self {
        Self {
            pen_mode: PenMode::Z,
            pen_up_z: 0.0,
            pen_down_z: 6.0,
            pen_down_s: 1000,
            pen_up_s: 0,
            pen_down_angle: 0,
            laser_power: 1000,
            pen_up_command: String::new(),
            pen_down_command: String::new(),
            pen_on_delay: 0.0,
            pen_off_delay: 0.0,
            draw_feed: 4400.0,
            travel_feed: 9000.0,
            pen_z_feed: 12000.0,
            custom_start_gcode: String::new(),
            custom_end_gcode: String::new(),
            return_to_start: true,
            swap_xy: false,
            invert_x: false,
            invert_y: false,
            origin_corner: OriginCorner::None,
            simplify_mm: 0.0,
            stroke_taper_mm: 1.5,
            entry_taper_mm: 0.5,
            decimals: 3,
            feed_decimals: 0,
            header: vec!["G21".into(), "G90".into(), "G94".into(), "G17".into()],
            footer: vec!["M5".into()],
            include_comments: true,
            reading_order: true,
        }
    }
}

impl GCodeConfig {
    fn f(&self, v: f64) -> String {
        let mut s = format!("{:.*}", self.decimals, v);
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        s
    }

    fn ff(&self, v: f64) -> String {
        let mut s = format!("{:.*}", self.feed_decimals, v);
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        if s.is_empty() {
            "0".into()
        } else {
            s
        }
    }

    pub fn fmt_xy(&self, x: f64, y: f64) -> String {
        format!("X{} Y{}", self.f(x), self.f(y))
    }

    fn split_custom(text: &str) -> Vec<String> {
        text.split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn pen_up_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.pen_mode {
            PenMode::Z => lines.push(format!(
                "G1 Z{} F{}",
                self.f(self.pen_up_z),
                self.ff(self.pen_z_feed)
            )),
            PenMode::M3M5 => lines.push("M5".into()),
            PenMode::ServoAngle => lines.push(format!("M3 S{}", self.pen_up_s)),
            PenMode::Laser => lines.push("M5".into()),
            PenMode::M7M9 => lines.push("M9".into()),
            PenMode::Custom => lines.push(if self.pen_up_command.is_empty() {
                "M5".into()
            } else {
                self.pen_up_command.clone()
            }),
        }
        if self.pen_off_delay > 0.0 {
            lines.push(format!("G4 P{}", self.f(self.pen_off_delay)));
        }
        lines
    }

    pub fn pen_down_lines(&self, to_z: Option<f64>) -> Vec<String> {
        let mut lines = Vec::new();
        match self.pen_mode {
            PenMode::Z => {
                let z = to_z.unwrap_or(self.pen_down_z);
                lines.push(format!("G1 Z{} F{}", self.f(z), self.ff(self.pen_z_feed)));
            }
            PenMode::M3M5 => lines.push(format!("M3 S{}", self.pen_down_s)),
            PenMode::ServoAngle => lines.push(format!("M3 S{}", self.pen_down_angle)),
            PenMode::Laser => lines.push(format!("M3 S{}", self.laser_power)),
            PenMode::M7M9 => lines.push("M7".into()),
            PenMode::Custom => lines.push(if self.pen_down_command.is_empty() {
                "M3".into()
            } else {
                self.pen_down_command.clone()
            }),
        }
        if self.pen_on_delay > 0.0 {
            lines.push(format!("G4 P{}", self.f(self.pen_on_delay)));
        }
        lines
    }
}

/// 页面坐标 → 机器坐标（机械原点 = 纸张左上角）。
pub fn page_to_machine(p: Vec2, page_h: f64, swap: bool, ix: bool, iy: bool) -> Vec2 {
    let mut x = p.0;
    let mut y = page_h - p.1;
    if swap {
        std::mem::swap(&mut x, &mut y);
    }
    if ix {
        x = if x == 0.0 { 0.0 } else { -x };
    }
    if iy {
        y = if y == 0.0 { 0.0 } else { -y };
    }
    (x, y)
}

#[derive(Clone, Debug, Default)]
pub struct GCodeResult {
    pub lines: Vec<String>,
    pub draw_length: f64,
    pub travel_length: f64,
    pub estimated_seconds: f64,
    pub stroke_count: usize,
    pub point_count: usize,
}

impl GCodeResult {
    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// 阅读顺序：按分组，组内保持原顺序，组间先上后下、先左后右。
fn order_reading(strokes: &[Stroke], groups: &[i32]) -> Vec<Stroke> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<i32, Vec<Stroke>> = BTreeMap::new();
    for (s, g) in strokes.iter().zip(groups.iter()) {
        buckets.entry(*g).or_default().push(s.clone());
    }
    let mut keyed: Vec<(f64, f64, Vec<Stroke>)> = buckets
        .into_values()
        .map(|group| {
            let xs0 = group
                .iter()
                .map(|s| s.points[0].0)
                .fold(f64::INFINITY, f64::min);
            let ys1 = group
                .iter()
                .map(|s| {
                    s.points
                        .iter()
                        .map(|p| p.1)
                        .fold(f64::NEG_INFINITY, f64::max)
                })
                .fold(f64::NEG_INFINITY, f64::max);
            (-(ys1 / 2.0).round() * 2.0, xs0, group)
        })
        .collect();
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    keyed.into_iter().flat_map(|(_, _, g)| g).collect()
}

/// 最短空程：贪心最近邻，允许反向。
fn order_shortest(strokes: &[Stroke], start: Option<Vec2>) -> Vec<Stroke> {
    let mut remaining: Vec<Option<Stroke>> = strokes.iter().cloned().map(Some).collect();
    let mut cur =
        start.unwrap_or_else(|| strokes.first().map(|s| s.start_pt()).unwrap_or((0.0, 0.0)));
    let mut out = Vec::with_capacity(strokes.len());
    while remaining.iter().any(|s| s.is_some()) {
        let mut best: Option<(usize, bool, f64)> = None;
        for (i, s) in remaining.iter().enumerate() {
            let Some(s) = s else { continue };
            let d0 = dist(cur, s.start_pt());
            if best.map(|b| d0 < b.2).unwrap_or(true) {
                best = Some((i, false, d0));
            }
            let d1 = dist(cur, s.end_pt());
            if best.map(|b| d1 < b.2).unwrap_or(true) {
                best = Some((i, true, d1));
            }
        }
        let Some((i, rev, _)) = best else { break };
        let mut s = remaining[i].take().unwrap();
        if rev {
            s.points.reverse();
        }
        cur = s.end_pt();
        out.push(s);
    }
    out
}

/// 把笔画生成为 G-code。
pub fn generate(
    strokes: &[Stroke],
    cfg: &GCodeConfig,
    page_h: f64,
    start_point: Option<Vec2>,
) -> GCodeResult {
    let use_strokes: Vec<Stroke> = strokes
        .iter()
        .filter(|s| s.points.len() >= 2)
        .cloned()
        .collect();
    let mut result = GCodeResult::default();
    if use_strokes.is_empty() {
        result.lines.extend(cfg.header.iter().cloned());
        result
            .lines
            .extend(GCodeConfig::split_custom(&cfg.custom_start_gcode));
        result
            .lines
            .extend(GCodeConfig::split_custom(&cfg.custom_end_gcode));
        result.lines.extend(cfg.footer.iter().cloned());
        return result;
    }

    // 抽稀
    let mut use_strokes = if cfg.simplify_mm > 0.0 {
        use_strokes
            .into_iter()
            .filter_map(|s| {
                let pts = if s.closed {
                    s.points.clone()
                } else {
                    rdp_simplify(&s.points, cfg.simplify_mm)
                };
                (pts.len() >= 2).then_some(Stroke { points: pts, ..s })
            })
            .collect::<Vec<_>>()
    } else {
        use_strokes
    };

    // 笔序
    if cfg.reading_order {
        let groups: Vec<i32> = use_strokes.iter().map(|s| s.group).collect();
        if groups.iter().any(|g| *g >= 0) {
            use_strokes = order_reading(&use_strokes, &groups);
        } else {
            use_strokes = order_shortest(&use_strokes, start_point);
        }
    } else {
        use_strokes = order_shortest(&use_strokes, start_point);
    }

    let map =
        |p: Vec2| -> Vec2 { page_to_machine(p, page_h, cfg.swap_xy, cfg.invert_x, cfg.invert_y) };
    let mapped: Vec<Vec<Vec2>> = use_strokes
        .iter()
        .map(|s| s.points.iter().map(|p| map(*p)).collect())
        .collect();

    let mut lines: Vec<String> = Vec::new();
    lines.extend(cfg.header.iter().cloned());
    lines.extend(GCodeConfig::split_custom(&cfg.custom_start_gcode));

    let (sx, sy) = match start_point {
        Some(p) => map(p),
        None => mapped[0][0],
    };

    // 斜落/斜抬（仅 Z 模式）
    let do_taper = cfg.pen_mode == PenMode::Z
        && (cfg.stroke_taper_mm > 0.0 || cfg.entry_taper_mm > 0.0)
        && cfg.draw_feed > 0.0
        && cfg.pen_z_feed > 0.0
        && cfg.pen_down_z > cfg.pen_up_z;
    let slope_cap = if do_taper {
        cfg.pen_z_feed / cfg.draw_feed
    } else {
        0.0
    };

    lines.extend(cfg.pen_up_lines());
    lines.push(format!("G0 {}", cfg.fmt_xy(sx, sy)));

    let mut draw_len = 0.0;
    let mut travel_len = 0.0;
    let mut point_count = 0usize;
    let mut cur = (sx, sy);
    let mut pen_down = false;

    for pts in &mapped {
        if pen_down {
            lines.extend(cfg.pen_up_lines());
        }
        let s0 = pts[0];
        if dist(cur, s0) > 0.0 {
            lines.push(format!("G0 {}", cfg.fmt_xy(s0.0, s0.1)));
            travel_len += dist(cur, s0);
            cur = s0;
        }

        // 单笔的斜坡计划
        let total: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
        let mut e_len = 0.0;
        let mut dz_e = 0.0;
        let mut x_len = 0.0;
        let mut dz_x = 0.0;
        if do_taper && total > 1e-9 {
            e_len = if cfg.entry_taper_mm > 0.0 {
                cfg.entry_taper_mm.min(total / 2.0)
            } else {
                0.0
            };
            x_len = if cfg.stroke_taper_mm > 0.0 {
                cfg.stroke_taper_mm.min(total / 2.0)
            } else {
                0.0
            };
            let full = cfg.pen_down_z - cfg.pen_up_z;
            dz_e = if e_len > 0.0 {
                full.min(slope_cap * e_len)
            } else {
                0.0
            };
            dz_x = if x_len > 0.0 {
                full.min(slope_cap * x_len)
            } else {
                0.0
            };
        }
        let has_taper = dz_e > 1e-9 || dz_x > 1e-9;
        let pts_v = if has_taper {
            split_taper_boundaries(pts, e_len, x_len)
        } else {
            pts.clone()
        };

        let to_z = if dz_e > 0.0 {
            Some(cfg.pen_down_z - dz_e)
        } else {
            None
        };
        lines.extend(cfg.pen_down_lines(to_z));
        pen_down = true;

        let mut cum = 0.0;
        for p in &pts_v[1..] {
            if *p == cur {
                continue;
            }
            let seg = dist(cur, *p);
            cum += seg;
            let z_word = if dz_e > 0.0 && cum <= e_len {
                Some(cfg.pen_down_z - dz_e + dz_e * (cum / e_len))
            } else if dz_x > 0.0 && (total - cum) < x_len {
                let frac = (x_len - (total - cum)) / x_len;
                Some(cfg.pen_down_z - dz_x * frac)
            } else {
                None
            };
            match z_word {
                Some(z) => lines.push(format!(
                    "G1 {} Z{} F{}",
                    cfg.fmt_xy(p.0, p.1),
                    cfg.f(z),
                    cfg.ff(cfg.draw_feed)
                )),
                None => lines.push(format!(
                    "G1 {} F{}",
                    cfg.fmt_xy(p.0, p.1),
                    cfg.ff(cfg.draw_feed)
                )),
            }
            draw_len += seg;
            cur = *p;
            point_count += 1;
        }
        point_count += 1;
    }

    if pen_down {
        lines.extend(cfg.pen_up_lines());
    }
    lines.extend(GCodeConfig::split_custom(&cfg.custom_end_gcode));
    if cfg.return_to_start {
        lines.push(format!("G90 G0 X{} Y{}", cfg.f(sx), cfg.f(sy)));
    }
    lines.extend(cfg.footer.iter().cloned());
    if cfg.include_comments {
        let mut s = String::new();
        let _ = write!(s, "(draw {draw_len:.1}mm, travel {travel_len:.1}mm)");
        lines.push(s);
    }

    result.estimated_seconds =
        draw_len / cfg.draw_feed.max(1.0) * 60.0 + travel_len / cfg.travel_feed.max(1.0) * 60.0;
    result.lines = lines;
    result.draw_length = draw_len;
    result.travel_length = travel_len;
    result.stroke_count = use_strokes.len();
    result.point_count = point_count;
    result
}

/// 在斜落/斜抬的弧长边界处插点，使斜坡与顶点采样密度无关。
fn split_taper_boundaries(pts: &[Vec2], e_len: f64, x_len: f64) -> Vec<Vec2> {
    let total: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    let mut bounds: Vec<f64> = Vec::new();
    if e_len > 1e-9 && e_len < total - 1e-9 {
        bounds.push(e_len);
    }
    if x_len > 0.0 {
        let b = total - x_len;
        if b > 1e-9 && b < total - 1e-9 {
            bounds.push(b);
        }
    }
    if bounds.is_empty() {
        return pts.to_vec();
    }
    bounds.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut out = vec![pts[0]];
    let mut cum = 0.0;
    let mut bi = 0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = dist(a, b);
        let seg_end = cum + seg;
        while bi < bounds.len() && bounds[bi] > cum + 1e-9 && bounds[bi] < seg_end - 1e-9 {
            let t = (bounds[bi] - cum) / seg;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            bi += 1;
        }
        out.push(b);
        cum = seg_end;
    }
    out
}

/// 估算时长（长度/进给，不含加速度模型）。
pub fn format_duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, rem) = (s / 3600, s % 3600);
    let (m, sec) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}小时{m}分")
    } else if m > 0 {
        format!("{m}分{sec}秒")
    } else {
        format!("{sec}秒")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::Role;

    fn sample() -> Vec<Stroke> {
        vec![
            Stroke::new(vec![(10.0, 100.0), (20.0, 100.0)], false, Role::Glyph).with_group(0),
            Stroke::new(vec![(10.0, 90.0), (20.0, 90.0)], false, Role::Glyph).with_group(1),
        ]
    }

    #[test]
    fn emits_header_and_footer() {
        let cfg = GCodeConfig::default();
        let r = generate(&sample(), &cfg, 297.0, None);
        assert_eq!(r.lines[0], "G21");
        assert!(r.lines.contains(&"M5".to_string()));
        assert_eq!(r.stroke_count, 2);
        assert!(r.draw_length > 19.9 && r.draw_length < 20.1);
    }

    #[test]
    fn maps_page_y_to_machine() {
        // 页面 y=100，纸高 297 → 机器 y=197
        let cfg = GCodeConfig::default();
        let r = generate(&sample(), &cfg, 297.0, None);
        let g0 = r
            .lines
            .iter()
            .find(|l| l.starts_with("G0 X10 Y197"))
            .unwrap_or_else(|| panic!("缺少起点指令：{:?}", r.lines));
        assert!(g0.contains("G0 X10 Y197"));
    }

    #[test]
    fn taper_adds_z_words() {
        let cfg = GCodeConfig::default();
        let r = generate(&sample(), &cfg, 297.0, None);
        assert!(
            r.lines
                .iter()
                .any(|l| l.starts_with("G1 X") && l.contains(" Z")),
            "斜落应在 G1 里带 Z 词"
        );
    }

    #[test]
    fn reading_order_follows_groups() {
        // 第二笔在页面上方，应被先写
        let strokes = vec![
            Stroke::new(vec![(10.0, 10.0), (20.0, 10.0)], false, Role::Glyph).with_group(0),
            Stroke::new(vec![(10.0, 200.0), (20.0, 200.0)], false, Role::Glyph).with_group(1),
        ];
        let cfg = GCodeConfig::default();
        let r = generate(&strokes, &cfg, 297.0, None);
        let idx_top = r
            .lines
            .iter()
            .position(|l| l.starts_with("G0 X10 Y97"))
            .unwrap();
        let idx_bottom = r
            .lines
            .iter()
            .position(|l| l.starts_with("G0 X10 Y287"))
            .unwrap();
        assert!(idx_top < idx_bottom, "应先写上方那一笔");
    }
}

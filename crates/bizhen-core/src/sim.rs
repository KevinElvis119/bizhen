//! 手写随机扰动（对应 WriterStudio 的 `perturb/params.py` + `engine.py` 的排版路径）。
//!
//! 扰动分三级：
//! * **字符级**：字号缩放、水平/垂直位移、整体旋转、体态斜切；
//! * **行级**：行基线抖动、行首偏移、行尾落差（行不水平）；
//! * **笔画级**：单笔画绕自身中心旋转/位移/沿轴伸缩/两端随机修剪；
//! * **结构线**：表格线、分割线这类 `Role::Structure` 笔画只加沿线起伏（wobble）
//!   与微颤（tremor），不做整体位移——否则相接的线会在接缝处撕开。
//!
//! 所有参数以 σ（标准差）表达，`N(0, σ)`；固定 `seed` 结果可复现。

use crate::geom::Vec2;
use crate::stroke::{Role, Stroke};

/// 手写扰动参数（默认全 0 = 关闭；单位见各字段注释）。
#[derive(Clone, Debug, PartialEq)]
pub struct JitterParams {
    pub enabled: bool,
    pub seed: u64,
    /// 总强度系数，统一放大/缩小下面所有量
    pub intensity: f64,

    // 字符级
    /// 字号比例抖动 σ（0.035 = ±3.5%）
    pub size_sigma: f64,
    /// 字符水平位移 σ (mm)
    pub char_x_sigma: f64,
    /// 字符垂直位移 σ (mm)
    pub char_y_sigma: f64,
    /// 字符整体旋转 σ (°)
    pub char_rot_sigma: f64,
    /// 体态斜切 σ (°)
    pub char_shear_sigma: f64,

    // 行级
    pub line_y_sigma: f64,
    pub line_start_sigma: f64,
    /// 行尾相对行首的高度漂移 σ (cm→mm)
    pub line_tilt_sigma: f64,
    /// 字距抖动 σ (mm)
    pub char_spacing_sigma: f64,
    /// 词间额外留空（× 字号）
    pub word_gap_em: f64,

    // 笔画级
    pub stroke_x_sigma: f64,
    pub stroke_y_sigma: f64,
    pub stroke_theta_sigma: f64,
    /// 沿自身轴向伸缩 σ（0.05 = ±5%）
    pub stroke_stretch_sigma: f64,
    /// 两端各随机修剪 0..该值 (mm)
    pub stroke_trim_mm: f64,

    // 结构线起伏
    pub line_wobble: f64,
    pub line_wobble_wavelength: f64,
    pub line_tremor: f64,

    pub smoothing: f64,
}

impl Default for JitterParams {
    fn default() -> Self {
        Self {
            enabled: false,
            seed: 20240910,
            intensity: 1.0,
            size_sigma: 0.0,
            char_x_sigma: 0.0,
            char_y_sigma: 0.0,
            char_rot_sigma: 0.0,
            char_shear_sigma: 0.0,
            line_y_sigma: 0.0,
            line_start_sigma: 0.0,
            line_tilt_sigma: 0.0,
            char_spacing_sigma: 0.0,
            word_gap_em: 0.0,
            stroke_x_sigma: 0.0,
            stroke_y_sigma: 0.0,
            stroke_theta_sigma: 0.0,
            stroke_stretch_sigma: 0.0,
            stroke_trim_mm: 0.0,
            line_wobble: 0.0,
            line_wobble_wavelength: 20.0,
            line_tremor: 0.0,
            smoothing: 0.0,
        }
    }
}

impl JitterParams {
    fn base(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    /// 「轻微」：几乎端正，只消除机器感
    pub fn subtle(size_mm: f64) -> Self {
        let mut p = Self::base(true);
        p.size_sigma = 0.012;
        p.char_x_sigma = size_mm * 0.015;
        p.char_y_sigma = size_mm * 0.012;
        p.char_rot_sigma = 0.25;
        p.char_shear_sigma = 0.8;
        p.line_y_sigma = size_mm * 0.02;
        p.line_start_sigma = size_mm * 0.04;
        p.line_tilt_sigma = size_mm * 0.04;
        p.char_spacing_sigma = size_mm * 0.008;
        p.word_gap_em = 0.06;
        p.stroke_theta_sigma = 1.2;
        p.stroke_stretch_sigma = 0.02;
        p.stroke_trim_mm = size_mm * 0.012;
        p.line_wobble = 0.18;
        p.line_wobble_wavelength = 16.0;
        p.line_tremor = 0.02;
        p.smoothing = 0.1;
        p
    }

    /// 「自然」：接近真人执笔
    pub fn natural(size_mm: f64) -> Self {
        let mut p = Self::base(true);
        p.size_sigma = 0.035;
        p.char_x_sigma = size_mm * 0.05;
        p.char_y_sigma = size_mm * 0.04;
        p.char_rot_sigma = 0.8;
        p.char_shear_sigma = 2.0;
        p.line_y_sigma = size_mm * 0.12;
        p.line_start_sigma = size_mm * 0.15;
        p.line_tilt_sigma = size_mm * 0.18;
        p.char_spacing_sigma = size_mm * 0.025;
        p.word_gap_em = 0.10;
        p.stroke_x_sigma = size_mm * 0.006;
        p.stroke_y_sigma = size_mm * 0.006;
        p.stroke_theta_sigma = 4.0;
        p.stroke_stretch_sigma = 0.05;
        p.stroke_trim_mm = size_mm * 0.03;
        p.line_wobble = 0.35;
        p.line_wobble_wavelength = 13.0;
        p.line_tremor = 0.035;
        p.smoothing = 0.15;
        p
    }

    /// 「强烈」：潦草、随性
    pub fn strong(size_mm: f64) -> Self {
        let mut p = Self::base(true);
        p.size_sigma = 0.07;
        p.char_x_sigma = size_mm * 0.10;
        p.char_y_sigma = size_mm * 0.09;
        p.char_rot_sigma = 2.2;
        p.char_shear_sigma = 4.5;
        p.line_y_sigma = size_mm * 0.25;
        p.line_start_sigma = size_mm * 0.30;
        p.line_tilt_sigma = size_mm * 0.35;
        p.char_spacing_sigma = size_mm * 0.06;
        p.word_gap_em = 0.14;
        p.stroke_x_sigma = size_mm * 0.012;
        p.stroke_y_sigma = size_mm * 0.012;
        p.stroke_theta_sigma = 7.0;
        p.stroke_stretch_sigma = 0.11;
        p.stroke_trim_mm = size_mm * 0.06;
        p.line_wobble = 0.75;
        p.line_wobble_wavelength = 9.0;
        p.line_tremor = 0.06;
        p.smoothing = 0.2;
        p
    }

    /// 是否需要扰动（关闭或全 0 时跳过，省 CPU）。
    pub fn is_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let f = |v: f64| v.abs() > 1e-12;
        f(self.size_sigma)
            || f(self.char_x_sigma)
            || f(self.char_y_sigma)
            || f(self.char_rot_sigma)
            || f(self.char_shear_sigma)
            || f(self.line_y_sigma)
            || f(self.line_start_sigma)
            || f(self.line_tilt_sigma)
            || f(self.char_spacing_sigma)
            || f(self.word_gap_em)
            || f(self.stroke_x_sigma)
            || f(self.stroke_y_sigma)
            || f(self.stroke_theta_sigma)
            || f(self.stroke_stretch_sigma)
            || f(self.stroke_trim_mm)
            || f(self.line_wobble)
            || f(self.line_tremor)
            || f(self.smoothing)
    }

    pub fn factor(&self) -> f64 {
        self.intensity.clamp(0.0, 10.0)
    }
}

// ---------------------------------------------------------------------------
// 随机数
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
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn gauss(&mut self, sigma: f64) -> f64 {
        if sigma <= 0.0 {
            return 0.0;
        }
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos() * sigma
    }
}

/// 一行开始时的随机量。
#[derive(Clone, Copy, Debug)]
pub struct LineJitter {
    pub dy: f64,
    pub start_dx: f64,
    /// 行尾落差（mm），沿行位置线性施加
    pub tilt: f64,
}

/// 排版时逐字调用的扰动器。
pub struct Simulator {
    params: JitterParams,
    rng: Rng,
    line: LineJitter,
    line_len: f64,
    spacing_state: f64,
}

impl Simulator {
    pub fn new(params: &JitterParams) -> Self {
        let mut rng = Rng::new(params.seed);
        // 预抽一个行参数，begin_line 会覆盖
        let _ = rng.next_u64();
        Self {
            params: params.clone(),
            rng,
            line: LineJitter {
                dy: 0.0,
                start_dx: 0.0,
                tilt: 0.0,
            },
            line_len: 0.0,
            spacing_state: 0.0,
        }
    }

    /// 开始新的一行；`line_len` 用于把行尾落差按位置摊开。
    pub fn begin_line(&mut self, line_len: f64) {
        let f = self.params.factor();
        self.line = LineJitter {
            dy: self.rng.gauss(self.params.line_y_sigma * f),
            start_dx: self.rng.gauss(self.params.line_start_sigma * f),
            tilt: self.rng.gauss(self.params.line_tilt_sigma * f),
        };
        self.line_len = line_len.max(1e-6);
        self.spacing_state = 0.0;
    }

    /// 一个字的位置扰动：返回 (dx, dy)。
    pub fn char_offset(
        &mut self,
        x_on_line: f64,
        is_first: bool,
        is_word_start: bool,
    ) -> (f64, f64) {
        let f = self.params.factor();
        let mut dx = self.rng.gauss(self.params.char_x_sigma * f);
        let mut dy = self.rng.gauss(self.params.char_y_sigma * f) + self.line.dy;
        dy += self.line.tilt * (x_on_line / self.line_len);
        if is_first {
            dx += self.line.start_dx;
        }
        if is_word_start && self.params.word_gap_em > 1e-12 {
            dx += self.params.word_gap_em * f * self.rng.uniform(0.75, 1.25);
        }
        // 字距节奏：均值回归的随机游走（不随行长漂移）
        self.spacing_state =
            0.6 * self.spacing_state + self.rng.gauss(0.5 * self.params.char_spacing_sigma * f);
        dx += self.spacing_state;
        (dx, dy)
    }

    /// 字号缩放与体态参数。
    pub fn char_shape(&mut self) -> (f64, f64, f64) {
        let f = self.params.factor();
        let size = (1.0 + self.rng.gauss(self.params.size_sigma * f)).clamp(0.2, 5.0);
        let rot = self.rng.gauss(self.params.char_rot_sigma * f);
        let shear = self.rng.gauss(self.params.char_shear_sigma * f);
        (size, rot, shear)
    }

    /// 笔画级扰动（单笔画绕中心旋转/位移/伸缩/修剪）。
    pub fn perturb_stroke(&mut self, s: &mut Stroke) {
        let f = self.params.factor();
        if s.points.len() < 2 {
            return;
        }
        let dx = self.rng.gauss(self.params.stroke_x_sigma * f);
        let dy = self.rng.gauss(self.params.stroke_y_sigma * f);
        for p in &mut s.points {
            p.0 += dx;
            p.1 += dy;
        }

        // 绕笔画中心旋转
        let theta = self
            .rng
            .gauss(self.params.stroke_theta_sigma * f)
            .to_radians();
        if theta.abs() > 1e-9 {
            let (cx, cy) = centroid(&s.points);
            let (sin, cos) = theta.sin_cos();
            for p in &mut s.points {
                let (x, y) = (p.0 - cx, p.1 - cy);
                p.0 = cx + x * cos - y * sin;
                p.1 = cy + x * sin + y * cos;
            }
        }

        // 沿自身轴向伸缩
        let stretch = self.rng.gauss(self.params.stroke_stretch_sigma * f);
        if stretch.abs() > 1e-9 {
            let a = s.points[0];
            let b = s.points[s.points.len() - 1];
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-9 {
                let (ux, uy) = (dx / len, dy / len);
                let (cx, cy) = centroid(&s.points);
                for p in &mut s.points {
                    let t = (p.0 - cx) * ux + (p.1 - cy) * uy;
                    let t2 = t * (1.0 + stretch);
                    let (along_x, along_y) = (ux * t, uy * t);
                    p.0 += ux * (t2 - t);
                    p.1 += uy * (t2 - t);
                    let _ = (along_x, along_y);
                }
            }
        }

        // 两端随机修剪
        let trim = self.params.stroke_trim_mm * f;
        if trim > 1e-9 {
            let t0 = self.rng.uniform(0.0, trim);
            let t1 = self.rng.uniform(0.0, trim);
            if !s.closed {
                s.points = trim_ends(&s.points, t0, t1);
            }
        }
    }

    /// 结构线：沿线起伏 + 微颤（不做整体位移）。
    pub fn wobble_structural(&mut self, s: &mut Stroke) {
        let f = self.params.factor();
        let amp = self.params.line_wobble * f;
        let tremor = self.params.line_tremor * f;
        if (amp <= 1e-9 && tremor <= 1e-9) || s.points.len() < 2 {
            return;
        }
        let wavelength = self.params.line_wobble_wavelength.max(1.0);
        // 沿线累加弧长 → 法向偏移
        let mut acc = 0.0;
        let mut phase = self.rng.uniform(0.0, wavelength);
        for i in 0..s.points.len() {
            if i > 0 {
                let a = s.points[i - 1];
                let b = s.points[i];
                acc += ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            }
            let (dir_x, dir_y) = direction_at(&s.points, i);
            let t = acc / wavelength + phase / wavelength;
            let off = amp * (t * 2.0 * std::f64::consts::PI).sin()
                + tremor * (t * 7.0 * std::f64::consts::PI).sin();
            s.points[i].0 += -dir_y * off;
            s.points[i].1 += dir_x * off;
        }
        phase = 0.0;
        let _ = phase;
    }
}

fn centroid(pts: &[Vec2]) -> Vec2 {
    let n = pts.len().max(1) as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for p in pts {
        x += p.0;
        y += p.1;
    }
    (x / n, y / n)
}

fn direction_at(pts: &[Vec2], i: usize) -> (f64, f64) {
    let (a, b) = if i == 0 {
        (pts[0], pts[1])
    } else if i + 1 >= pts.len() {
        (pts[pts.len() - 2], pts[pts.len() - 1])
    } else {
        (pts[i - 1], pts[i + 1])
    };
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= 1e-12 {
        (1.0, 0.0)
    } else {
        (dx / len, dy / len)
    }
}

/// 从两端各剪掉 `t0` / `t1` 弧长（至少保留两个点）。
fn trim_ends(pts: &[Vec2], t0: f64, t1: f64) -> Vec<Vec2> {
    let mut out = trim_front(pts.to_vec(), t0);
    out.reverse();
    out = trim_front(out, t1);
    out.reverse();
    out
}

fn trim_front(mut pts: Vec<Vec2>, mut cut: f64) -> Vec<Vec2> {
    while cut > 1e-9 && pts.len() >= 2 {
        let (a, b) = (pts[0], pts[1]);
        let seg = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if seg <= 1e-12 {
            pts.remove(0);
            continue;
        }
        if seg > cut {
            let t = cut / seg;
            pts[0] = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            return pts;
        }
        if pts.len() > 2 {
            pts.remove(0);
            cut -= seg;
        } else {
            // 整段被剪光：退化成中点（不产生空笔画）
            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            pts[0] = mid;
            pts[1] = mid;
            return pts;
        }
    }
    pts
}

/// 对一个字符的全部笔画施加字符级变换（缩放/旋转/斜切/平移）。
pub fn transform_char(
    strokes: &mut [Stroke],
    origin: Vec2,
    size_factor: f64,
    rotation_deg: f64,
    shear_deg: f64,
    dx: f64,
    dy: f64,
) {
    let theta = rotation_deg.to_radians();
    let (sin, cos) = theta.sin_cos();
    let shear = shear_deg.to_radians().tan();
    for s in strokes.iter_mut() {
        for p in &mut s.points {
            let (mut x, mut y) = (p.0 - origin.0, p.1 - origin.1);
            // 先缩放
            x *= size_factor;
            y *= size_factor;
            // 再斜切（x 随 y 线性偏移）
            x += shear * y;
            // 再旋转
            let (rx, ry) = (x * cos - y * sin, x * sin + y * cos);
            p.0 = origin.0 + rx + dx;
            p.1 = origin.1 + ry + dy;
        }
    }
}

/// 对整页笔画做收尾处理：结构线起伏。
pub fn wobble_structural_strokes(strokes: &mut [Stroke], params: &JitterParams) {
    if !params.is_active() {
        return;
    }
    let mut sim = Simulator::new(params);
    for s in strokes.iter_mut() {
        if s.role == Role::Structure {
            sim.wobble_structural(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_active_and_scaled() {
        let p = JitterParams::natural(4.0);
        assert!(p.is_active());
        assert!((p.char_x_sigma - 0.2).abs() < 1e-9);
        assert!(!JitterParams::default().is_active());
    }

    #[test]
    fn seed_reproducible() {
        let p = JitterParams::natural(4.0);
        let mut a = Simulator::new(&p);
        let mut b = Simulator::new(&p);
        a.begin_line(50.0);
        b.begin_line(50.0);
        let x1 = a.char_offset(10.0, false, false);
        let x2 = b.char_offset(10.0, false, false);
        assert!((x1.0 - x2.0).abs() < 1e-12);
        assert!((x1.1 - x2.1).abs() < 1e-12);
        // 换种子结果不同
        let mut c = Simulator::new(&JitterParams {
            seed: 7,
            ..p.clone()
        });
        c.begin_line(50.0);
        let x3 = c.char_offset(10.0, false, false);
        assert!((x1.0 - x3.0).abs() > 1e-9);
    }

    #[test]
    fn transform_moves_points() {
        let mut s = vec![Stroke::new(
            vec![(0.0, 0.0), (1.0, 0.0)],
            false,
            Role::Glyph,
        )];
        transform_char(&mut s, (0.0, 0.0), 2.0, 0.0, 0.0, 1.0, 0.5);
        assert!((s[0].points[1].0 - 3.0).abs() < 1e-9);
        assert!((s[0].points[1].1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn trim_shortens() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0)];
        let out = trim_ends(&pts, 1.0, 2.0);
        assert!((out[0].0 - 1.0).abs() < 1e-9);
        assert!((out[1].0 - 8.0).abs() < 1e-9);
    }
}

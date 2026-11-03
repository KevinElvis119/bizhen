//! 几何基础：点、包围盒、折线抽稀。
//!
//! 坐标约定（与原 Python 版一致，全程不能改）：
//! * 单位 **毫米(mm)**，**Y 轴向上**（与机械/G-code 坐标一致，导出时不做翻转）；
//! * 页面原点在**左下角**，纸张顶部为 `y = page_height`。

/// 二维点 / 向量。
pub type Vec2 = (f64, f64);

/// 轴对齐包围盒。默认是「空盒」（`x0 > x1`），便于逐步并集。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Default for BBox {
    fn default() -> Self {
        Self::empty()
    }
}

impl BBox {
    pub const fn empty() -> Self {
        Self {
            x0: f64::INFINITY,
            y0: f64::INFINITY,
            x1: f64::NEG_INFINITY,
            y1: f64::NEG_INFINITY,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 > self.x1 || self.y0 > self.y1
    }

    pub fn width(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.x1 - self.x0
        }
    }

    pub fn height(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.y1 - self.y0
        }
    }

    pub fn center(&self) -> Vec2 {
        if self.is_empty() {
            (0.0, 0.0)
        } else {
            ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
        }
    }

    pub fn expand(&mut self, p: Vec2) {
        let (x, y) = p;
        if x < self.x0 {
            self.x0 = x;
        }
        if y < self.y0 {
            self.y0 = y;
        }
        if x > self.x1 {
            self.x1 = x;
        }
        if y > self.y1 {
            self.y1 = y;
        }
    }

    pub fn from_points<I>(points: I) -> Self
    where
        I: IntoIterator<Item = Vec2>,
    {
        let mut b = Self::empty();
        for p in points {
            b.expand(p);
        }
        b
    }

    pub fn union(self, other: BBox) -> BBox {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        BBox {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

/// Douglas–Peucker 折线抽稀（迭代实现，保首尾点）。
///
/// `tol <= 0` 或点数 < 3 时原样返回。G-code 导出抽稀与手写扰动共用。
pub fn rdp_simplify(points: &[Vec2], tol: f64) -> Vec<Vec2> {
    if tol <= 0.0 || points.len() < 3 {
        return points.to_vec();
    }
    let n = points.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (ax, ay) = points[i];
        let (bx, by) = points[j];
        let (dx, dy) = (bx - ax, by - ay);
        let seg2 = dx * dx + dy * dy;
        let mut dmax = -1.0;
        let mut imax = i;
        for (m, p) in points.iter().enumerate().take(j).skip(i + 1) {
            let (px, py) = *p;
            let d = if seg2 <= 1e-18 {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            } else {
                let t = (((px - ax) * dx + (py - ay) * dy) / seg2).clamp(0.0, 1.0);
                ((px - (ax + t * dx)).powi(2) + (py - (ay + t * dy)).powi(2)).sqrt()
            };
            if d > dmax {
                dmax = d;
                imax = m;
            }
        }
        if dmax > tol {
            keep[imax] = true;
            stack.push((i, imax));
            stack.push((imax, j));
        }
    }
    points
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bbox_ignores_points_until_first() {
        let mut b = BBox::empty();
        assert!(b.is_empty());
        assert_eq!(b.width(), 0.0);
        b.expand((1.0, 2.0));
        assert!(!b.is_empty());
        assert_eq!(b.width(), 0.0);
        b.expand((3.0, 5.0));
        assert_eq!(b.width(), 2.0);
        assert_eq!(b.height(), 3.0);
    }

    #[test]
    fn rdp_keeps_endpoints() {
        let pts = vec![(0.0, 0.0), (1.0, 0.01), (2.0, 0.0), (3.0, 5.0)];
        let out = rdp_simplify(&pts, 0.1);
        assert_eq!(out.first(), Some(&(0.0, 0.0)));
        assert_eq!(out.last(), Some(&(3.0, 5.0)));
        assert!(out.len() < pts.len());
    }
}

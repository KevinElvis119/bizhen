//! 笔画数据模型。
//!
//! 一条 [`Stroke`] = 一次落笔到抬笔之间的连续轨迹（折线，单位 mm）。
//! 排版产物、导入的矢量图、G-code 输入都是这个结构。

use crate::geom::Vec2;

/// 笔画来源：决定手写扰动的作用面（原 Python 用字符串 `role`，这里用枚举）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// 未标注：自由手绘/矢量图，按手写笔画对待。
    #[default]
    Free,
    /// 字体字形笔画：线条起伏/弯折不作用于文字。
    Glyph,
    /// 结构线：分隔线、引述竖线、导入的矢量图形。
    /// 扰动时不做整笔刚性变换与端点修饰，只叠线条起伏。
    Structure,
    /// 表格网格线：抖动时保持规整（格线歪斜会让文字错格）。
    TableLine,
}

/// 一条连续笔迹（折线）。`closed` 表示首尾闭合。
#[derive(Clone, Debug)]
pub struct Stroke {
    pub points: Vec<Vec2>,
    pub closed: bool,
    pub role: Role,
    /// 书写分组：一个字符/一个对象一组，笔序规划「写完一组再写下一组」。`-1` = 未分组。
    pub group: i32,
}

impl Stroke {
    pub fn new(points: Vec<Vec2>, closed: bool, role: Role) -> Self {
        Self {
            points,
            closed,
            role,
            group: -1,
        }
    }

    pub fn with_group(mut self, group: i32) -> Self {
        self.group = group;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn start(&self) -> Option<Vec2> {
        self.points.first().copied()
    }

    pub fn end(&self) -> Option<Vec2> {
        self.points.last().copied()
    }

    pub fn start_pt(&self) -> Vec2 {
        self.points.first().copied().unwrap_or((0.0, 0.0))
    }

    pub fn end_pt(&self) -> Vec2 {
        self.points.last().copied().unwrap_or((0.0, 0.0))
    }

    /// 折线总长度(mm)，闭合笔画含收尾那一段。
    pub fn length(&self) -> f64 {
        let mut total = 0.0;
        for w in self.points.windows(2) {
            total += ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        }
        if self.closed && self.points.len() >= 2 {
            let a = self.points[self.points.len() - 1];
            let b = self.points[0];
            total += ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        }
        total
    }

    /// 整体平移（返回新笔画）。
    pub fn translated(&self, dx: f64, dy: f64) -> Stroke {
        let mut s = self.clone();
        for p in &mut s.points {
            p.0 += dx;
            p.1 += dy;
        }
        s
    }
}

pub fn total_length<'a, I: IntoIterator<Item = &'a Stroke>>(strokes: I) -> f64 {
    strokes.into_iter().map(|s| s.length()).sum()
}

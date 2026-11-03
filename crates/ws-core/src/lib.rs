//! WriterStudio 的 Rust 重写 —— 纯逻辑核心（无 UI 依赖）。
//!
//! 目标：保留原 Python 版的 **Markdown 排版能力**（标题/公式/图片占位/表格、
//! 按行宽自动折行、字体与度量对齐），并新增**分页**；去掉笔画编辑、扰动、
//! TikZ、SVG、MCP 等外围功能。
//!
//! 模块规划见 `docs/DEVELOPMENT_PLAN.md`：
//! * [`geom`] / [`stroke`]      几何与笔画模型
//! * [`font`]                   字库解析、度量、回退链
//! * `layout`（M2）             文本排版与折行
//! * `markdown`（M2/M3）        Markdown → 笔画
//! * `math`（M3）               LaTeX 公式 → 笔画（自研排版引擎）
//! * `paginate`（M3）           分页流
//! * `gcode`（M4）              笔画 → G-code

pub mod font;
pub mod geom;
pub mod stroke;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use font::registry::fonts_dir;
pub use geom::{rdp_simplify, BBox, Vec2};
pub use stroke::{Role, Stroke};

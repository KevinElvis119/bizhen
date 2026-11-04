//! WriterStudio 的 Rust 重写 —— 纯逻辑核心（无 UI 依赖）。
//!
//! 目标：保留原 Python 版的 **Markdown 排版能力**（标题/公式/图片占位/表格、
//! 按行宽自动折行、字体与度量对齐），并新增**分页**；去掉笔画编辑、扰动、
//! TikZ、SVG、MCP 等外围功能。
//!
//! 模块：
//! * [`geom`] / [`stroke`]   几何与笔画模型
//! * [`font`]                字库解析（`.jhf` / `STRK-*.json`）、度量、回退链
//! * [`layout`]              文本排版与折行
//! * [`markdown`]            Markdown → 分页笔画
//! * [`math`] / [`symbols`]  LaTeX 公式排版（自研，替代 matplotlib mathtext）
//! * [`gcode`]               笔画 → G-code
//! * [`serial`]              GRBL 串口联机
//! * [`render`]              SVG 预览导出

#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod font;
pub mod gcode;
pub mod geom;
pub mod layout;
pub mod markdown;
pub mod math;
pub mod render;
#[cfg(feature = "serial")]
pub mod serial;
pub mod sim;
pub mod stroke;
pub mod symbols;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use font::registry::fonts_dir;
pub use geom::{rdp_simplify, BBox, Vec2};
pub use layout::{
    append_text, layout_text, measure_text, pick_font, widest_char_width, wrap_text, Align,
    CharPlacement, LineLayout, TextLayout, TextStyle,
};
pub use stroke::{Role, Stroke};

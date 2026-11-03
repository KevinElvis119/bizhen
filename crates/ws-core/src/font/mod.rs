//! 字体层：数据模型、解析器与注册表。
//!
//! 目前实现：Hershey `.jhf`（单线拉丁/希腊/符号）。
//! 待实现（见 docs/DEVELOPMENT_PLAN.md）：`STRK-*.json` 中文单线笔画字库。

pub mod jhf;
pub mod model;
pub mod registry;

pub use jhf::{parse_hershey_bytes, parse_hershey_jhf, FontError};
pub use model::{pct, FontFamily, FontKind, Glyph};
pub use registry::FontRegistry;

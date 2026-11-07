//! 字体层：数据模型、解析器与注册表。

pub mod jhf;
pub mod model;
pub mod registry;
pub mod stroke_json;

pub use jhf::{parse_hershey_bytes, parse_hershey_jhf, FontError};
pub use model::{pct, FontFamily, FontKind, Glyph};
pub use registry::FontRegistry;
pub use stroke_json::{parse_stroke_json, parse_stroke_json_str};

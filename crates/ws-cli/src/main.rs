//! `ws-cli` —— 无界面的命令行入口，用来验证核心层、生成 G-code、做基准对比。
//!
//! 用法：
//! ```text
//! ws-cli fonts              列出内置字体
//! ws-cli info <font>        打印一款字体的度量信息
//! ws-cli md <file.md>       排版并导出 G-code（M2/M4 实现）
//! ```

use std::process::ExitCode;

use ws_core::font::FontRegistry;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("fonts") => {
            let reg = FontRegistry::builtin();
            let items: Vec<_> = reg
                .entries()
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "kind": e.kind,
                        "path": e.path.display().to_string(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&items).unwrap());
            ExitCode::SUCCESS
        }
        Some("info") => {
            let Some(name) = args.get(1) else {
                eprintln!("用法：ws-cli info <font>");
                return ExitCode::from(2);
            };
            let mut reg = FontRegistry::builtin();
            if reg.get_entry(name).map(|e| e.kind) == Some("stroke-json") {
                eprintln!("{name} 是中文单线字库，解析尚未实现（计划 M1：font/stroke_json.rs）");
                return ExitCode::from(3);
            }
            match reg.get(name) {
                Some(f) => {
                    let g = f.glyph('A').map(|g| g.bbox());
                    let info = serde_json::json!({
                        "name": f.name,
                        "kind": f.kind.as_str(),
                        "units_per_em": f.units_per_em,
                        "glyphs": f.coverage(),
                        "ascent": f.ascent,
                        "descent": f.descent,
                        "default_advance": f.default_advance,
                        "line_height_at_4mm_1.5x": f.line_height(4.0, 1.5),
                        "glyph_A_height": g.map(|b| b.height()),
                    });
                    println!("{}", serde_json::to_string_pretty(&info).unwrap());
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("没有这款字体：{name}（先用 ws-cli fonts 查看）");
                    ExitCode::from(1)
                }
            }
        }
        Some("md") => {
            eprintln!("Markdown 排版尚未实现（计划见 docs/DEVELOPMENT_PLAN.md 的 M2/M4）");
            ExitCode::from(3)
        }
        _ => {
            eprintln!(
                "用法：ws-cli fonts | info <font> | md <file.md>   (v{})",
                ws_core::VERSION
            );
            ExitCode::from(2)
        }
    }
}

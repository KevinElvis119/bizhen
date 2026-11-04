//! `bizhen-cli` —— 无界面的命令行入口：排版、导出、串口诊断。
//!
//! ```text
//! bizhen-cli fonts                     列出内置字体
//! bizhen-cli info <font>               打印一款字体的度量信息
//! bizhen-cli ports                     列出串口
//! bizhen-cli md <file.md> [选项]        排版并导出
//! ```
//!
//! `md` 的选项：
//! * `-o <path>`      输出文件（按扩展名：`.gcode` / `.svg`；默认打印统计）
//! * `--page A4`      纸张（A4/A5/B5/Letter/...，见 `page_preset`）
//! * `--landscape`    横向
//! * `--margin 10`    页边距（mm）
//! * `--size 4`       正文字号（mm）
//! * `--font kai,futural` 字体链（逗号分隔；缺省 = 中文楷体 + futural）

use std::process::ExitCode;

use bizhen_core::font::FontRegistry;
use bizhen_core::gcode::{self, GCodeConfig};
use bizhen_core::markdown::{self, MarkdownStyle, PageSpec};
use bizhen_core::render;
#[cfg(feature = "serial")]
use bizhen_core::serial;
use bizhen_core::symbols::SymbolFonts;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("fonts") => cmd_fonts(),
        Some("info") => cmd_info(&args),
        #[cfg(feature = "serial")]
        Some("ports") => cmd_ports(),
        Some("md") => cmd_md(&args[1..]),
        _ => {
            eprintln!(
                "Bizhen v{}\n\n\
                 用法：\n  \
                 bizhen-cli fonts                      列出内置字体\n  \
                 bizhen-cli info <font>                字体度量\n  \
                 bizhen-cli ports                      列出串口\n  \
                 bizhen-cli md <file.md> [-o out.gcode|out.svg] [--page A4] [--landscape]\n      \
                 [--margin 10] [--size 4] [--font kai,futural]",
                bizhen_core::VERSION
            );
            ExitCode::from(2)
        }
    }
}

fn cmd_fonts() -> ExitCode {
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

fn cmd_info(args: &[String]) -> ExitCode {
    let Some(name) = args.get(1) else {
        eprintln!("用法：bizhen-cli info <font>");
        return ExitCode::from(2);
    };
    let mut reg = FontRegistry::builtin();
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
            eprintln!("没有这款字体：{name}（先用 bizhen-cli fonts 查看）");
            ExitCode::from(1)
        }
    }
}

#[cfg(feature = "serial")]
fn cmd_ports() -> ExitCode {
    let ports = serial::list_ports();
    if ports.is_empty() {
        println!("[]");
        return ExitCode::SUCCESS;
    }
    let items: Vec<_> = ports
        .iter()
        .map(|p| serde_json::json!({"name": p.name, "description": p.description}))
        .collect();
    println!("{}", serde_json::to_string_pretty(&items).unwrap());
    ExitCode::SUCCESS
}

/// 纸张预设（与 `page_presets.py` 的内置表一致）。
fn page_preset(name: &str) -> Option<PageSpec> {
    let (w, h, m) = match name.to_ascii_lowercase().as_str() {
        "a0" => (841.0, 1189.0, 10.0),
        "a1" => (594.0, 841.0, 10.0),
        "a2" => (420.0, 594.0, 10.0),
        "a3" => (297.0, 420.0, 10.0),
        "a4" => (210.0, 297.0, 10.0),
        "a5" => (148.0, 210.0, 10.0),
        "a6" => (105.0, 148.0, 10.0),
        "b0" => (1000.0, 1414.0, 10.0),
        "b1" => (707.0, 1000.0, 10.0),
        "b2" => (500.0, 707.0, 10.0),
        "b3" => (353.0, 500.0, 10.0),
        "b4" => (250.0, 353.0, 10.0),
        "b5" => (176.0, 250.0, 10.0),
        "letter" => (215.9, 279.4, 12.7),
        "legal" => (215.9, 355.6, 12.7),
        "tabloid" => (279.4, 431.8, 12.7),
        _ => return None,
    };
    Some(PageSpec {
        width: w,
        height: h,
        margin: m,
    })
}

fn cmd_md(args: &[String]) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut page = PageSpec::a4();
    let mut landscape = false;
    let mut style = MarkdownStyle::default();
    let mut font_names: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-o" | "--out" => {
                i += 1;
                output = args.get(i).cloned();
            }
            "--page" => {
                i += 1;
                if let Some(p) = args.get(i).and_then(|s| page_preset(s)) {
                    page = p;
                }
            }
            "--landscape" => landscape = true,
            "--margin" => {
                i += 1;
                if let Some(m) = args.get(i).and_then(|s| s.parse::<f64>().ok()) {
                    page.margin = m;
                }
            }
            "--size" => {
                i += 1;
                if let Some(s) = args.get(i).and_then(|s| s.parse::<f64>().ok()) {
                    style.size = s;
                }
            }
            "--font" => {
                i += 1;
                if let Some(list) = args.get(i) {
                    font_names = list
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
            other => {
                if !other.starts_with('-') && input.is_none() {
                    input = Some(other.to_string());
                }
            }
        }
        i += 1;
    }

    let Some(input_path) = input else {
        eprintln!("用法：bizhen-cli md <file.md> [-o out.gcode|out.svg]");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(&input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("读取 {input_path} 失败：{e}");
            return ExitCode::from(1);
        }
    };

    if landscape {
        page = page.landscape();
    }

    let mut reg = FontRegistry::builtin();
    if font_names.is_empty() {
        font_names = vec!["STRK-Kaiti".into(), "futural".into()];
    }
    let mut fonts = Vec::new();
    for name in &font_names {
        match reg.get(name) {
            Some(f) => fonts.push(f.clone()),
            None => eprintln!("警告：字体 {name} 不存在，已跳过"),
        }
    }
    if fonts.is_empty() {
        eprintln!("没有任何可用字体");
        return ExitCode::from(1);
    }

    let symbols = SymbolFonts::load();
    let doc = markdown::render_markdown(&src, &fonts, &symbols, &style, page);

    let out_ext = output
        .as_deref()
        .and_then(|p| std::path::Path::new(p).extension())
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match out_ext.as_str() {
        "gcode" | "nc" | "ngc" => {
            let strokes = doc.strokes();
            let cfg = GCodeConfig::default();
            let result = gcode::generate(&strokes, &cfg, page.height, None);
            if let Some(path) = &output {
                if let Err(e) = std::fs::write(path, result.text()) {
                    eprintln!("写入 {path} 失败：{e}");
                    return ExitCode::from(1);
                }
            }
            println!(
                "页数 {} · 笔画 {} · 绘制 {:.1} mm · 空程 {:.1} mm · 预计 {}",
                doc.pages.len(),
                result.stroke_count,
                result.draw_length,
                result.travel_length,
                gcode::format_duration(result.estimated_seconds)
            );
        }
        "json" => {
            let pages: Vec<_> = doc
                .pages
                .iter()
                .map(|p| {
                    let strokes: Vec<_> = p
                        .strokes
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "closed": s.closed,
                                "points": s.points.iter().map(|q| [q.0, q.1]).collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    serde_json::json!({ "strokes": strokes })
                })
                .collect();
            let payload = serde_json::json!({
                "page": { "width": page.width, "height": page.height, "margin": page.margin },
                "pages": pages,
                "missing": doc.missing.iter().collect::<String>(),
            });
            if let Some(path) = &output {
                if let Err(e) = std::fs::write(path, serde_json::to_string(&payload).unwrap()) {
                    eprintln!("写入 {path} 失败：{e}");
                    return ExitCode::from(1);
                }
            }
            println!("页数 {} · 输出 JSON", doc.pages.len());
        }
        "svg" => {
            let svg = render::doc_svg(&doc, page);
            if let Some(path) = &output {
                if let Err(e) = std::fs::write(path, svg) {
                    eprintln!("写入 {path} 失败：{e}");
                    return ExitCode::from(1);
                }
            }
            println!(
                "页数 {} · 笔画 {} · 输出 SVG",
                doc.pages.len(),
                doc.strokes().len()
            );
        }
        _ => {
            let b = doc.bbox();
            println!(
                "页数 {} · 笔画 {} · 包围盒 {:.1}×{:.1} mm",
                doc.pages.len(),
                doc.strokes().len(),
                b.width(),
                b.height()
            );
        }
    }

    if !doc.missing.is_empty() {
        let chars: String = doc.missing.iter().collect();
        eprintln!("警告：字体链画不出的字符（{chars}）");
    }
    ExitCode::SUCCESS
}

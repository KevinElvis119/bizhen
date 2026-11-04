//! 输出辅助：把页面笔画导成 SVG（1 单位 = 1 mm），便于不开界面时核对排版。

use crate::geom::BBox;
use crate::markdown::{PageSpec, RenderedDoc};
use crate::stroke::Stroke;

/// 一页内容的 SVG 片段（不含外层 `<svg>`）：页边距参考线 + 全部笔画。
fn page_body(strokes: &[Stroke], page: PageSpec, margin_line: bool) -> String {
    let mut out = String::new();
    if margin_line {
        out.push_str(&format!(
            "<rect x=\"{m}\" y=\"{m}\" width=\"{w}\" height=\"{h}\" fill=\"none\" stroke=\"#dddddd\" stroke-width=\"0.2\"/>\n",
            m = page.margin,
            w = page.width - 2.0 * page.margin,
            h = page.height - 2.0 * page.margin
        ));
    }
    out.push_str("<g stroke=\"black\" stroke-width=\"0.35\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n");
    for s in strokes {
        if s.points.is_empty() {
            continue;
        }
        out.push_str("<path d=\"");
        for (i, p) in s.points.iter().enumerate() {
            // 屏幕坐标 Y 向下
            let (x, y) = (p.0, page.height - p.1);
            if i == 0 {
                out.push_str(&format!("M{x:.3} {y:.3}"));
            } else {
                out.push_str(&format!(" L{x:.3} {y:.3}"));
            }
        }
        if s.closed {
            out.push_str(" Z");
        }
        out.push_str("\"/>\n");
    }
    out.push_str("</g>\n");
    out
}

/// 单页 SVG。
pub fn page_svg(strokes: &[Stroke], page: PageSpec) -> String {
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n",
        w = page.width,
        h = page.height
    );
    out.push_str("<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n");
    out.push_str(&page_body(strokes, page, true));
    out.push_str("</svg>\n");
    out
}

/// 整份文档的 SVG：每页一张纸，纵向排列，便于一眼看分页效果。
pub fn doc_svg(doc: &RenderedDoc, page: PageSpec) -> String {
    let n = doc.pages.len().max(1);
    let gap = 10.0;
    let total_h = page.height * n as f64 + gap * (n as f64 - 1.0);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n",
        w = page.width,
        h = total_h
    );
    out.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#f4f4f4\"/>\n");
    for (i, p) in doc.pages.iter().enumerate() {
        let dy = i as f64 * (page.height + gap);
        out.push_str(&format!("<g transform=\"translate(0,{dy:.3})\">\n"));
        out.push_str(&format!(
            "<rect width=\"{w}\" height=\"{h}\" fill=\"white\" stroke=\"#cccccc\" stroke-width=\"0.2\"/>\n",
            w = page.width,
            h = page.height
        ));
        out.push_str(&page_body(&p.strokes, page, true));
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

/// 内容包围盒（调试用）。
pub fn content_bbox(strokes: &[Stroke]) -> BBox {
    BBox::from_points(strokes.iter().flat_map(|s| s.points.iter().copied()))
}

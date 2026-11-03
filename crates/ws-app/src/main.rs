//! `ws-app` —— GPUI 桌面端（M4 实现）。
//!
//! 界面规划：
//! * 左侧：Markdown 源码编辑区（多行文本）
//! * 右侧：分页预览（GPUI 自绘：笔画折线 + 页框/页边距）
//! * 顶部：纸张预设、字号、行距、字体链、表格宽度
//! * 底部：导出 G-code / 打印进度

fn main() {
    eprintln!(
        "ws-app 尚未实现（v{}）。\n\
         当前可用：cargo run -p ws-cli -- fonts\n\
         开发计划见 docs/DEVELOPMENT_PLAN.md",
        ws_core::VERSION
    );
}

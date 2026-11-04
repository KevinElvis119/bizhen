//! `bizhen` —— 笔阵的 GPUI 桌面端。
//!
//! 布局：
//! ```text
//! ┌──────────────────── 顶部：打开 / 导出 / 字体与抖动 / 翻页 ────────────────────┐
//! │ 左侧栏（Markdown | 页面 两个标签） │ 中间：分页预览 │ 右侧：机器（串口/Jog/状态）│
//! └──────────────────────────────── 底部状态栏 ──────────────────────────────────┘
//! ```
//!
//! 编辑器说明：gpui 0.2 的核心 crate 没有多行文本输入控件（Zed 的编辑器在未发布的
//! `ui` crate 里），这里是自绘的：字符输入、退格/删除、方向键、Home/End、回车、
//! Ctrl+V 粘贴、滚轮滚动。中文输入法需要平台的组合输入支持，当前靠「打开文件/粘贴」。

mod machine;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Application, Bounds, ContentMask,
    Context, KeyDownEvent, PathBuilder, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    Window, WindowBounds, WindowOptions,
};

use bizhen_core::font::FontRegistry;
use bizhen_core::gcode::{self, GCodeConfig};
use bizhen_core::markdown::{self, MarkdownStyle, PageSpec, RenderedDoc};
use bizhen_core::serial::{self, PortInfo};
use bizhen_core::sim::JitterParams;
use bizhen_core::symbols::SymbolFonts;
use bizhen_core::Stroke;

use machine::Machine;

/// 纸张预设（mm，纵向：名称、宽、高、页边距）
const PRESETS: &[(&str, f64, f64, f64)] = &[
    ("A4", 210.0, 297.0, 10.0),
    ("A5", 148.0, 210.0, 10.0),
    ("B5", 176.0, 250.0, 10.0),
    ("A3", 297.0, 420.0, 10.0),
    ("Letter", 215.9, 279.4, 12.7),
    ("明信片", 100.0, 148.0, 5.0),
];

const BAUDS: &[u32] = &[9600, 19200, 38400, 57600, 115200, 250000];

/// 编辑器一行的高度（px），滚轮与光标跟随都用它换算
const LINE_H: f32 = 18.0;
/// 编辑器可见行数（估算，用于光标跟随时保底）
const EDITOR_VISIBLE_LINES: usize = 26;

const SAMPLE: &str = "# 静夜思\n\n床前明月光 疑是地上霜\n举头望明月 低头思故乡\n\n## 公式\n\n质能方程 $E = mc^2$，行内公式随正文重排。\n\n## 表格\n\n| 项目 | 数量 | 备注 |\n|:---|---:|:---:|\n| 笔 | 2 | 黑色 |\n| 纸 | 10 | A4 |\n\n![示意图](none.png){width=60,height=30}\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Markdown,
    Page,
}

struct WriterApp {
    source: String,
    /// 光标位置（按字符计）
    cursor: usize,
    /// 编辑器滚动偏移（px）
    editor_scroll: f32,
    tab: Tab,

    preset: usize,
    landscape: bool,
    margin: f64,
    size_mm: f64,
    wrap: f64,
    table_width: f64,
    page: PageSpec,

    registry: FontRegistry,
    font_names: Vec<String>,
    jitter: JitterParams,
    dialog: bool,
    font_page: usize,

    symbols: SymbolFonts,
    doc: RenderedDoc,
    page_index: usize,
    zoom: f32,
    status: SharedString,

    machine: Machine,
    ports: Vec<PortInfo>,
    port_idx: usize,
    baud_idx: usize,
    port_menu: bool,
    baud_menu: bool,
    jog_step: f64,
    jog_feed: f64,

    focus: gpui::FocusHandle,
}

impl WriterApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let registry = FontRegistry::builtin();
        let symbols = SymbolFonts::load();
        let page = PageSpec {
            width: PRESETS[0].1,
            height: PRESETS[0].2,
            margin: PRESETS[0].3,
        };
        let mut font_names: Vec<String> = Vec::new();
        for want in ["STRK-Kaiti", "futural"] {
            if registry.get_entry(want).is_some() {
                font_names.push(want.to_string());
            }
        }
        if font_names.is_empty() {
            font_names = registry.names().into_iter().take(1).collect();
        }
        let mut app = Self {
            source: std::env::var("BIZHEN_DEMO_SOURCE")
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_else(|| SAMPLE.to_string()),
            cursor: 0,
            // 冒烟/演示：BIZHEN_DEMO_SCROLL=px 指定初始滚动位置
            editor_scroll: std::env::var("BIZHEN_DEMO_SCROLL")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0),
            // 冒烟/演示：BIZHEN_DEMO_TAB=page 启动即显示「页面」标签
            tab: if std::env::var("BIZHEN_DEMO_TAB").as_deref() == Ok("page") {
                Tab::Page
            } else {
                Tab::Markdown
            },
            preset: 0,
            landscape: false,
            margin: page.margin,
            size_mm: 4.0,
            wrap: 160.0,
            table_width: 0.0,
            page,
            registry,
            font_names,
            jitter: JitterParams::default(),
            // 演示/冒烟：BIZHEN_DEMO_DIALOG=1 启动即打开弹窗
            dialog: std::env::var("BIZHEN_DEMO_DIALOG").is_ok(),
            font_page: 0,
            symbols,
            doc: RenderedDoc::default(),
            page_index: 0,
            zoom: std::env::var("BIZHEN_DEMO_ZOOM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            status: "就绪".into(),
            machine: Machine::new(),
            ports: serial::list_ports(),
            port_idx: 0,
            baud_idx: 4, // 115200
            port_menu: false,
            baud_menu: false,
            jog_step: 10.0,
            jog_feed: 2500.0,
            focus: cx.focus_handle(),
        };
        app.cursor = app.source.chars().count();
        app.relayout();
        let handle = app.focus.clone();
        window.focus(&handle);
        app
    }

    fn effective_page(&self) -> PageSpec {
        let (w, h) = if self.landscape {
            (self.page.height, self.page.width)
        } else {
            (self.page.width, self.page.height)
        };
        PageSpec {
            width: w,
            height: h,
            margin: self.margin,
        }
    }

    fn chain_fonts(&mut self) -> Vec<bizhen_core::font::FontFamily> {
        let names = self.font_names.clone();
        let mut out = Vec::new();
        for n in names {
            if let Some(f) = self.registry.get(&n) {
                out.push(f.clone());
            }
        }
        out
    }

    fn relayout(&mut self) {
        let page = self.effective_page();
        let style = MarkdownStyle {
            size: self.size_mm,
            wrap_width: self.wrap.min(page.content_width()),
            table_width: self.table_width,
            jitter: self.jitter.clone(),
            ..Default::default()
        };
        let fonts = self.chain_fonts();
        self.doc = markdown::render_markdown(&self.source, &fonts, &self.symbols, &style, page);
        if self.page_index >= self.doc.pages.len() {
            self.page_index = self.doc.pages.len().saturating_sub(1);
        }
        let stroke_count: usize = self.doc.pages.iter().map(|p| p.strokes.len()).sum();
        self.status = format!(
            "{} 页 · {} 笔画{}",
            self.doc.pages.len(),
            stroke_count,
            if self.doc.missing.is_empty() {
                String::new()
            } else {
                format!(" · 缺字：{}", self.doc.missing.iter().collect::<String>())
            }
        )
        .into();
    }

    fn import_font(&mut self) -> String {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("单线字体", &["gfont", "jhf", "json"])
            .pick_file()
        else {
            return "已取消".into();
        };
        let dir = bizhen_core::font::registry::user_font_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return format!("创建字库目录失败：{e}");
        }
        let Some(file_name) = path.file_name() else {
            return "文件名无效".into();
        };
        let dest = dir.join(file_name);
        if let Err(e) = std::fs::copy(&path, &dest) {
            return format!("复制失败：{e}");
        }
        self.registry.refresh();
        self.relayout();
        format!("已导入 {}", file_name.to_string_lossy())
    }

    // -- 文本编辑 -----------------------------------------------------------
    fn byte_of_char(&self, char_idx: usize) -> usize {
        self.source
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.source.len())
    }

    fn insert(&mut self, text: &str) {
        let b = self.byte_of_char(self.cursor);
        self.source.insert_str(b, text);
        self.cursor += text.chars().count();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_of_char(self.cursor);
        let start = self.byte_of_char(self.cursor - 1);
        self.source.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.source.chars().count() {
            return;
        }
        let start = self.byte_of_char(self.cursor);
        let end = self.byte_of_char(self.cursor + 1);
        self.source.replace_range(start..end, "");
    }

    fn cursor_line_col(&self) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for (i, ch) in self.source.chars().enumerate() {
            if i >= self.cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn move_vertical(&mut self, down: bool) {
        let lines: Vec<&str> = self.source.split('\n').collect();
        let (line, col) = self.cursor_line_col();
        let target = if down {
            if line + 1 >= lines.len() {
                return;
            }
            line + 1
        } else {
            if line == 0 {
                return;
            }
            line - 1
        };
        let target_len = lines[target].chars().count();
        let offset: usize = lines[..target].iter().map(|l| l.chars().count() + 1).sum();
        self.cursor = offset + col.min(target_len);
    }

    /// 让光标所在行保持在可视范围内。
    fn follow_cursor(&mut self) {
        let (line, _) = self.cursor_line_col();
        let first = (self.editor_scroll / LINE_H) as usize;
        if line < first {
            self.editor_scroll = line as f32 * LINE_H;
        } else if line >= first + EDITOR_VISIBLE_LINES {
            self.editor_scroll = (line + 1 - EDITOR_VISIBLE_LINES) as f32 * LINE_H;
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let mut changed = false;
        match ks.key.as_str() {
            "backspace" => {
                self.backspace();
                changed = true;
            }
            "delete" => {
                self.delete();
                changed = true;
            }
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => {
                if self.cursor < self.source.chars().count() {
                    self.cursor += 1;
                }
            }
            "up" => self.move_vertical(false),
            "down" => self.move_vertical(true),
            "home" => {
                let (line, _) = self.cursor_line_col();
                let offset: usize = self
                    .source
                    .split('\n')
                    .take(line)
                    .map(|l| l.chars().count() + 1)
                    .sum();
                self.cursor = offset;
            }
            "end" => {
                let lines: Vec<&str> = self.source.split('\n').collect();
                let (line, _) = self.cursor_line_col();
                let offset: usize = lines[..line].iter().map(|l| l.chars().count() + 1).sum();
                self.cursor = offset + lines[line].chars().count();
            }
            "enter" => {
                self.insert("\n");
                changed = true;
            }
            "v" if ks.modifiers.control => {
                if let Some(item) = cx.read_from_clipboard() {
                    if let Some(text) = item.text() {
                        self.insert(&text);
                        changed = true;
                    }
                }
            }
            _ => {
                if let Some(ch) = ks.key_char.as_deref() {
                    if !ks.modifiers.control && !ch.is_empty() {
                        self.insert(ch);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.relayout();
        }
        self.follow_cursor();
        cx.notify();
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let lines = self.source.split('\n').count() as f32;
        let max = ((lines - 1.0) * LINE_H).max(0.0);
        let dy = match ev.delta {
            ScrollDelta::Lines(p) => p.y * LINE_H,
            ScrollDelta::Pixels(p) => f32::from(p.y),
        };
        self.editor_scroll = (self.editor_scroll - dy).clamp(0.0, max);
        cx.notify();
    }

    // -- 操作 ---------------------------------------------------------------
    fn open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "txt"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.source = text;
                    self.cursor = self.source.chars().count();
                    self.editor_scroll = 0.0;
                    self.relayout();
                    self.status = format!("已打开 {}", path.display()).into();
                }
                Err(e) => self.status = format!("打开失败：{e}").into(),
            }
        }
    }

    fn export_gcode(&mut self) {
        let page = self.effective_page();
        let cfg = GCodeConfig::default();
        let result = gcode::generate(&self.doc.strokes(), &cfg, page.height, None);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("bizhen.gcode")
            .save_file()
        {
            match std::fs::write(&path, result.text()) {
                Ok(_) => {
                    self.status = format!(
                        "已导出 {} · 绘制 {:.1} mm · 预计 {}",
                        path.display(),
                        result.draw_length,
                        gcode::format_duration(result.estimated_seconds)
                    )
                    .into();
                }
                Err(e) => self.status = format!("导出失败：{e}").into(),
            }
        }
    }

    fn refresh_ports(&mut self) {
        self.ports = serial::list_ports();
        self.port_idx = 0;
        self.port_menu = false;
        self.status = if self.ports.is_empty() {
            "没有找到串口".into()
        } else {
            format!("{} 个串口", self.ports.len()).into()
        };
    }

    fn connect_or_disconnect(&mut self, cx: &mut Context<Self>) {
        let st = self.machine.snapshot();
        if st.connected {
            self.machine.send(machine::Cmd::Disconnect);
        } else {
            let Some(port) = self.ports.get(self.port_idx).map(|p| p.name.clone()) else {
                self.status = "没有可用串口，先刷新".into();
                cx.notify();
                return;
            };
            let baud = BAUDS[self.baud_idx.min(BAUDS.len() - 1)];
            self.machine.send(machine::Cmd::Connect { port, baud });
        }
        cx.notify();
    }

    fn send_document(&mut self, cx: &mut Context<Self>) {
        let st = self.machine.snapshot();
        if !st.connected {
            self.status = "未连接串口".into();
            cx.notify();
            return;
        }
        let page = self.effective_page();
        let cfg = GCodeConfig::default();
        let lines = gcode::generate(&self.doc.strokes(), &cfg, page.height, None).lines;
        self.machine.send(machine::Cmd::Send(lines));
        cx.notify();
    }

    /// 每帧刷新机器状态（仅在已连接时保持循环）。
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let st = self.machine.snapshot();
        if !st.connected && !st.message.is_empty() {
            self.status = st.message.clone().into();
        }
        cx.notify();
        if st.connected {
            cx.on_next_frame(window, |this, window, cx| Self::tick(this, window, cx));
        }
    }
}

/// 工具栏按钮。
fn button(
    id: &str,
    label: impl Into<SharedString>,
    cx: &mut Context<WriterApp>,
    on_click: impl Fn(&mut WriterApp, &mut Window, &mut Context<WriterApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0x3a3a3a))
        .text_color(rgb(0xe0e0e0))
        .text_xs()
        .hover(|s| s.bg(rgb(0x4a4a4a)))
        .child(label.into())
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

/// 一行设置：标签 + 「−」+ 值 + 「＋」。
fn setting_row(
    label: &str,
    value: String,
    id: &str,
    cx: &mut Context<WriterApp>,
    dec: impl Fn(&mut WriterApp) + 'static,
    inc: impl Fn(&mut WriterApp) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .py_0p5()
        .child(
            div()
                .w(px(86.))
                .text_xs()
                .text_color(rgb(0xb0b0b0))
                .child(label.to_string()),
        )
        .child(button(
            &format!("{id}_dec"),
            "−",
            cx,
            move |this, _w, cx| {
                dec(this);
                cx.notify();
            },
        ))
        .child(
            div()
                .w(px(58.))
                .text_xs()
                .text_color(rgb(0xffffff))
                .child(value),
        )
        .child(button(
            &format!("{id}_inc"),
            "＋",
            cx,
            move |this, _w, cx| {
                inc(this);
                cx.notify();
            },
        ))
}

/// 抖动参数行（弹窗用，改动后立即重排）。
fn param_row(
    label: &str,
    value: String,
    id: &str,
    cx: &mut Context<WriterApp>,
    dec: impl Fn(&mut WriterApp) + 'static,
    inc: impl Fn(&mut WriterApp) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(150.))
                .text_xs()
                .text_color(rgb(0xb0b0b0))
                .child(label.to_string()),
        )
        .child(button(
            &format!("{id}_dec"),
            "−",
            cx,
            move |this, _w, cx| {
                dec(this);
                this.relayout();
                cx.notify();
            },
        ))
        .child(
            div()
                .w(px(64.))
                .text_xs()
                .text_color(rgb(0xffffff))
                .child(value),
        )
        .child(button(
            &format!("{id}_inc"),
            "＋",
            cx,
            move |this, _w, cx| {
                inc(this);
                this.relayout();
                cx.notify();
            },
        ))
}

impl Render for WriterApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.effective_page();
        let current: Vec<Stroke> = self
            .doc
            .pages
            .get(self.page_index)
            .map(|p| p.strokes.clone())
            .unwrap_or_default();
        let zoom = self.zoom;
        let (cursor_line, cursor_col) = self.cursor_line_col();
        let all_lines: Vec<String> = self.source.split('\n').map(|s| s.to_string()).collect();
        let total_pages = self.doc.pages.len().max(1);
        let status = self.status.clone();
        let focus = self.focus.clone();
        let preset_name = PRESETS[self.preset].0;
        let landscape = self.landscape;
        let tab = self.tab;
        let machine_state = self.machine.snapshot();
        let port_name = self
            .ports
            .get(self.port_idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "无串口".into());
        let baud = BAUDS[self.baud_idx.min(BAUDS.len() - 1)];
        let port_menu = self.port_menu;
        let baud_menu = self.baud_menu;

        // ---------------- 顶部 ----------------
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(rgb(0x252525))
            .border_b_1()
            .border_color(rgb(0x383838))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child("笔阵 · Bizhen"),
            )
            .child(div().flex_1())
            .child(button("open", "打开…", cx, |this, _w, cx| {
                this.open_file();
                cx.notify();
            }))
            .child(button("export", "导出 G-code", cx, |this, _w, cx| {
                this.export_gcode();
                cx.notify();
            }))
            .child(button("style", "字体与抖动", cx, |this, _w, cx| {
                this.dialog = !this.dialog;
                cx.notify();
            }))
            .child(button("prev_page", "◀", cx, |this, _w, cx| {
                this.page_index = this.page_index.saturating_sub(1);
                cx.notify();
            }))
            .child(div().text_xs().text_color(rgb(0xaaaaaa)).child(format!(
                "{} / {}",
                self.page_index + 1,
                total_pages
            )))
            .child(button("next_page", "▶", cx, |this, _w, cx| {
                if this.page_index + 1 < this.doc.pages.len().max(1) {
                    this.page_index += 1;
                }
                cx.notify();
            }));

        // ---------------- 左侧栏 ----------------
        let tab_style = |active: bool| {
            if active {
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0xffffff))
                    .bg(rgb(0x3a3a3a))
            } else {
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x999999))
                    .bg(rgb(0x2a2a2a))
            }
        };

        let tabs = div()
            .flex()
            .items_center()
            .bg(rgb(0x2a2a2a))
            .border_b_1()
            .border_color(rgb(0x383838))
            .child(
                div()
                    .id("tab_md")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tab = Tab::Markdown;
                        cx.notify();
                    }))
                    .child(tab_style(tab == Tab::Markdown).child("Markdown")),
            )
            .child(
                div()
                    .id("tab_page")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tab = Tab::Page;
                        cx.notify();
                    }))
                    .child(tab_style(tab == Tab::Page).child("页面")),
            );

        let left_content = if tab == Tab::Markdown {
            let first = (self.editor_scroll / LINE_H) as usize;
            let last = (first + EDITOR_VISIBLE_LINES + 8).min(all_lines.len());
            let mut editor = div().flex().flex_col();
            for (idx, line) in all_lines.iter().enumerate().take(last).skip(first) {
                let row = if idx == cursor_line {
                    let before: String = line.chars().take(cursor_col).collect();
                    let after: String = line.chars().skip(cursor_col).collect();
                    div()
                        .h(px(LINE_H))
                        .flex()
                        .items_center()
                        .child(div().text_xs().text_color(rgb(0xdddddd)).child(before))
                        .child(div().w(px(1.5)).h(px(13.)).bg(rgb(0x3b82f6)))
                        .child(div().text_xs().text_color(rgb(0xdddddd)).child(after))
                } else {
                    div()
                        .h(px(LINE_H))
                        .child(div().text_xs().text_color(rgb(0xbbbbbb)).child(
                            if line.is_empty() {
                                " ".to_string()
                            } else {
                                line.clone()
                            },
                        ))
                };
                editor = editor.child(row);
            }
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_hidden()
                .p_3()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_key))
                .on_scroll_wheel(cx.listener(Self::on_wheel))
                .child(editor)
        } else {
            let mut panel = div().flex().flex_col().gap_1().p_3();
            panel = panel
                .child(div().text_xs().text_color(rgb(0x888888)).child("纸张"))
                .child(button(
                    "preset",
                    format!("纸张 {preset_name}（点击切换）"),
                    cx,
                    move |this, _w, cx| {
                        this.preset = (this.preset + 1) % PRESETS.len();
                        let (_, w, h, m) = PRESETS[this.preset];
                        this.page = PageSpec {
                            width: w,
                            height: h,
                            margin: m,
                        };
                        this.margin = m;
                        this.relayout();
                        cx.notify();
                    },
                ))
                .child(button(
                    "orient",
                    if landscape {
                        "方向：横向"
                    } else {
                        "方向：纵向"
                    },
                    cx,
                    |this, _w, cx| {
                        this.landscape = !this.landscape;
                        this.relayout();
                        cx.notify();
                    },
                ))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("排版"),
                )
                .child(setting_row(
                    "字号 (mm)",
                    format!("{:.1}", self.size_mm),
                    "sz",
                    cx,
                    |a| {
                        a.size_mm = (a.size_mm - 0.5).max(1.5);
                        a.relayout();
                    },
                    |a| {
                        a.size_mm = (a.size_mm + 0.5).min(20.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "页边距 (mm)",
                    format!("{:.1}", self.margin),
                    "mg",
                    cx,
                    |a| {
                        a.margin = (a.margin - 1.0).max(0.0);
                        a.relayout();
                    },
                    |a| {
                        a.margin = (a.margin + 1.0).min(40.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "折行宽 (mm)",
                    format!("{:.0}", self.wrap),
                    "wr",
                    cx,
                    |a| {
                        a.wrap = (a.wrap - 10.0).max(20.0);
                        a.relayout();
                    },
                    |a| {
                        a.wrap = (a.wrap + 10.0).min(400.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "表格宽 (mm)",
                    if self.table_width <= 0.0 {
                        "自适应".to_string()
                    } else {
                        format!("{:.0}", self.table_width)
                    },
                    "tw",
                    cx,
                    |a| {
                        a.table_width = if a.table_width <= 0.0 {
                            0.0
                        } else {
                            (a.table_width - 10.0).max(0.0)
                        };
                        a.relayout();
                    },
                    |a| {
                        a.table_width = (a.table_width + 10.0).min(400.0);
                        a.relayout();
                    },
                ))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("预览"),
                )
                .child(setting_row(
                    "缩放",
                    format!("{:.1}×", self.zoom),
                    "zm",
                    cx,
                    |a| a.zoom = (a.zoom - 0.1).max(0.3),
                    |a| a.zoom = (a.zoom + 0.1).min(4.0),
                ));
            div().flex_1().min_h(px(0.)).overflow_hidden().child(panel)
        };

        let left = div()
            .w(px(400.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(0x333333))
            .child(tabs)
            .child(left_content);

        // ---------------- 中间预览 ----------------
        let page_w = page.width;
        let page_h = page.height;
        let margin = page.margin;

        let preview = div().flex_1().min_w(px(0.)).h_full().p_3().child(
            canvas(
                move |_, _, _| {},
                move |bounds, _state, window, _cx| {
                    // 裁剪到画布范围：缩放后纸张不会盖住工具栏
                    let clip = Bounds {
                        origin: bounds.origin,
                        size: bounds.size,
                    };
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let avail_w = f32::from(bounds.size.width) - 8.0;
                        let avail_h = f32::from(bounds.size.height) - 8.0;
                        let scale = ((avail_w / page_w as f32).min(avail_h / page_h as f32))
                            .max(0.05)
                            * zoom;
                        let paper_w = page_w as f32 * scale;
                        let paper_h = page_h as f32 * scale;
                        let ox = f32::from(bounds.origin.x) + (avail_w - paper_w) / 2.0;
                        let oy = f32::from(bounds.origin.y) + (avail_h - paper_h) / 2.0;

                        window.paint_quad(quad(
                            Bounds {
                                origin: point(px(ox), px(oy)),
                                size: size(px(paper_w), px(paper_h)),
                            },
                            px(0.),
                            gpui::white(),
                            px(1.),
                            rgb(0x666666),
                            Default::default(),
                        ));

                        let m = margin as f32 * scale;
                        window.paint_quad(quad(
                            Bounds {
                                origin: point(px(ox + m), px(oy + m)),
                                size: size(px(paper_w - 2.0 * m), px(paper_h - 2.0 * m)),
                            },
                            px(0.),
                            gpui::transparent_black(),
                            px(0.5),
                            rgb(0xdddddd),
                            Default::default(),
                        ));

                        let line_width = (0.35_f64 * scale as f64).max(0.9) as f32;
                        for stroke in &current {
                            if stroke.points.len() < 2 {
                                continue;
                            }
                            let mut builder = PathBuilder::stroke(px(line_width));
                            for (i, p) in stroke.points.iter().enumerate() {
                                let x = ox + p.0 as f32 * scale;
                                let y = oy + (page_h - p.1) as f32 * scale;
                                if i == 0 {
                                    builder.move_to(point(px(x), px(y)));
                                } else {
                                    builder.line_to(point(px(x), px(y)));
                                }
                            }
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, rgb(0x111111));
                            }
                        }
                    });
                },
            )
            .size_full(),
        );

        // ---------------- 右侧机器栏 ----------------
        let mut machine_panel = div().flex().flex_col().gap_1().p_3();
        machine_panel = machine_panel
            .child(div().text_xs().text_color(rgb(0x888888)).child("串口"))
            .child(button(
                "port",
                format!("{port_name} ▾"),
                cx,
                |this, _w, cx| {
                    this.port_menu = !this.port_menu;
                    cx.notify();
                },
            ));
        if port_menu {
            let mut list = div().flex().flex_col().gap_1();
            if self.ports.is_empty() {
                list = list.child(div().text_xs().text_color(rgb(0x888888)).child("没有串口"));
            }
            for (i, p) in self.ports.iter().enumerate() {
                let label = format!("{} {}", p.name, p.description);
                list = list.child(button(
                    &format!("port_{i}"),
                    label,
                    cx,
                    move |this, _w, cx| {
                        this.port_idx = i;
                        this.port_menu = false;
                        cx.notify();
                    },
                ));
            }
            machine_panel = machine_panel.child(list);
        }
        machine_panel = machine_panel
            .child(button(
                "refresh_ports",
                "刷新端口",
                cx,
                |this, _w, cx| {
                    this.refresh_ports();
                    cx.notify();
                },
            ))
            .child(button(
                "baud",
                format!("{baud} 波特 ▾"),
                cx,
                |this, _w, cx| {
                    this.baud_menu = !this.baud_menu;
                    cx.notify();
                },
            ));
        if baud_menu {
            let mut list = div().flex().flex_col().gap_1();
            for (i, b) in BAUDS.iter().enumerate() {
                list = list.child(button(
                    &format!("baud_{i}"),
                    format!("{b}"),
                    cx,
                    move |this, _w, cx| {
                        this.baud_idx = i;
                        this.baud_menu = false;
                        cx.notify();
                    },
                ));
            }
            machine_panel = machine_panel.child(list);
        }
        machine_panel =
            machine_panel
                .child(button(
                    "connect",
                    if machine_state.connected {
                        "断开"
                    } else {
                        "连接"
                    },
                    cx,
                    |this, _w, cx| this.connect_or_disconnect(cx),
                ))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("状态"),
                )
                .child(div().text_xs().text_color(rgb(0xdddddd)).child(
                    if machine_state.connected {
                        let pos = machine_state
                            .mpos
                            .map(|(x, y, z)| format!("X{x:.2} Y{y:.2} Z{z:.2}"))
                            .unwrap_or_default();
                        format!(
                            "{}  {}",
                            if machine_state.grbl_state.is_empty() {
                                "—"
                            } else {
                                machine_state.grbl_state.as_str()
                            },
                            pos
                        )
                    } else {
                        "未连接".to_string()
                    },
                ))
                .child(div().text_xs().text_color(rgb(0x888888)).child(
                    if machine_state.message.is_empty() {
                        String::new()
                    } else {
                        machine_state.message.clone()
                    },
                ))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(button("unlock", "解锁", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Unlock);
                            cx.notify();
                        }))
                        .child(button("home", "回零", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Home);
                            cx.notify();
                        }))
                        .child(button("reset", "复位", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Reset);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("手动控制"),
                )
                .child(setting_row(
                    "步长 (mm)",
                    format!("{:.1}", self.jog_step),
                    "jog_step",
                    cx,
                    |a| a.jog_step = (a.jog_step / 2.0).max(0.1),
                    |a| a.jog_step = (a.jog_step * 2.0).min(100.0),
                ))
                .child(setting_row(
                    "速度 (mm/min)",
                    format!("{:.0}", self.jog_feed),
                    "jog_feed",
                    cx,
                    |a| a.jog_feed = (a.jog_feed - 250.0).max(10.0),
                    |a| a.jog_feed = (a.jog_feed + 250.0).min(20000.0),
                ));

        // 方向键（Y+ / X− X+ / Y−）
        let jog_button = |id: &str, label: &str, dx: f64, dy: f64| {
            div()
                .id(SharedString::from(id.to_string()))
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(0x3a3a3a))
                .text_xs()
                .text_color(rgb(0xe0e0e0))
                .hover(|s| s.bg(rgb(0x4a4a4a)))
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.machine.send(machine::Cmd::Jog {
                        dx,
                        dy,
                        step: this.jog_step,
                        feed: this.jog_feed,
                    });
                    cx.notify();
                }))
        };
        machine_panel = machine_panel
            .child(
                div()
                    .mt_1()
                    .flex()
                    .gap_1()
                    .child(div().w(px(44.)).h(px(28.)))
                    .child(jog_button("jog_yp", "Y+", 0.0, 1.0))
                    .child(div().w(px(44.)).h(px(28.))),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(jog_button("jog_xm", "X−", -1.0, 0.0))
                    .child(jog_button("jog_xp", "X+", 1.0, 0.0))
                    .child(jog_button("jog_ym", "Y−", 0.0, -1.0)),
            )
            .child(div().mt_2())
            .child(button(
                "print",
                "发送作业（打印）",
                cx,
                |this, _w, cx| {
                    this.send_document(cx);
                },
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(if machine_state.busy {
                        format!(
                            "发送中 {}/{}",
                            machine_state.progress.0, machine_state.progress.1
                        )
                    } else {
                        String::new()
                    }),
            );

        let right = div()
            .w(px(300.))
            .h_full()
            .border_l_1()
            .border_color(rgb(0x333333))
            .child(machine_panel);

        // ---------------- 底部 ----------------
        let footer = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(rgb(0x252525))
            .border_t_1()
            .border_color(rgb(0x383838))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(0xa0a0a0))
                    .child(status),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x777777))
                    .child(format!("第 {} 行", cursor_line + 1)),
            );

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1b1b1b));
        root = root.child(header);
        root = root.child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.))
                .child(left)
                .child(preview)
                .child(right),
        );
        root = root.child(footer);

        if self.dialog {
            let entries: Vec<(String, String, String)> = self
                .registry
                .entries()
                .iter()
                .map(|e| (e.name.clone(), e.display.clone(), e.kind.to_string()))
                .collect();
            let chain = self.font_names.clone();
            let jitter = self.jitter.clone();
            let font_page = self.font_page;
            root = root.child(build_dialog(&chain, &entries, font_page, &jitter, cx));
        }

        // 已连接时持续刷新机器状态
        if machine_state.connected {
            cx.on_next_frame(window, |this, window, cx| Self::tick(this, window, cx));
        }

        root
    }
}

/// 「字体与手写抖动」弹窗。
fn build_dialog(
    chain: &[String],
    entries: &[(String, String, String)],
    page: usize,
    jitter: &JitterParams,
    cx: &mut Context<WriterApp>,
) -> impl IntoElement {
    const PER_PAGE: usize = 12;
    let total = entries.len();
    let pages = total.div_ceil(PER_PAGE).max(1);
    let page = page.min(pages - 1);
    let start = page * PER_PAGE;
    let end = (start + PER_PAGE).min(total);

    let display_of = |name: &str| {
        entries
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, d, _)| d.clone())
            .unwrap_or_else(|| name.to_string())
    };

    let mut chips = div().flex().flex_wrap().gap_1();
    if chain.is_empty() {
        chips = chips.child(
            div()
                .text_xs()
                .text_color(rgb(0xff8888))
                .child("字体链为空，选一款字体"),
        );
    }
    for (i, name) in chain.iter().enumerate() {
        let up = name.clone();
        let del = name.clone();
        chips = chips.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .rounded_md()
                .bg(rgb(0x3a3a3a))
                .child(div().text_xs().text_color(rgb(0xe8e8e8)).child(format!(
                    "{}. {}",
                    i + 1,
                    display_of(name)
                )))
                .child(button(
                    &format!("up_{i}"),
                    "↑",
                    cx,
                    move |this, _w, cx| {
                        if let Some(pos) = this.font_names.iter().position(|c| c == &up) {
                            if pos > 0 {
                                this.font_names.swap(pos - 1, pos);
                            }
                        }
                        this.relayout();
                        cx.notify();
                    },
                ))
                .child(button(
                    &format!("del_{i}"),
                    "×",
                    cx,
                    move |this, _w, cx| {
                        if this.font_names.len() <= 1 {
                            return;
                        }
                        if let Some(pos) = this.font_names.iter().position(|c| c == &del) {
                            this.font_names.remove(pos);
                        }
                        this.relayout();
                        cx.notify();
                    },
                )),
        );
    }

    let mut font_grid = div().flex().flex_wrap().gap_1();
    for (name, display, kind) in &entries[start..end] {
        let chosen = chain.iter().any(|c| c == name);
        let label = if chosen {
            format!("✓ {display}")
        } else {
            format!("+ {display}")
        };
        let n = name.clone();
        let tip = match kind.as_str() {
            "gfont" => "gfont",
            "stroke-json" => "中文单线",
            _ => "Hershey",
        };
        font_grid = font_grid.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(button(
                    &format!("pick_{name}"),
                    label,
                    cx,
                    move |this, _w, cx| {
                        if let Some(pos) = this.font_names.iter().position(|c| c == &n) {
                            this.font_names.remove(pos);
                        } else {
                            this.font_names.push(n.clone());
                        }
                        this.relayout();
                        cx.notify();
                    },
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x777777))
                        .child(tip.to_string()),
                ),
        );
    }

    let page_row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(button("font_prev", "◀", cx, |this, _w, cx| {
            this.font_page = this.font_page.saturating_sub(1);
            cx.notify();
        }))
        .child(div().text_xs().text_color(rgb(0x999999)).child(format!(
            "字体 {}/{}（共 {} 款）",
            page + 1,
            pages,
            total
        )))
        .child(button("font_next", "▶", cx, |this, _w, cx| {
            this.font_page += 1;
            cx.notify();
        }))
        .child(button(
            "import_font",
            "导入字体…",
            cx,
            |this, _w, cx| {
                this.status = this.import_font().into();
                cx.notify();
            },
        ));

    let preset_row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(60.))
                .text_xs()
                .text_color(rgb(0xb0b0b0))
                .child("预设"),
        )
        .child(button("j_off", "关闭", cx, |this, _w, cx| {
            this.jitter = JitterParams::default();
            this.relayout();
            cx.notify();
        }))
        .child(button("j_subtle", "轻微", cx, |this, _w, cx| {
            this.jitter = JitterParams::subtle(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(button("j_natural", "自然", cx, |this, _w, cx| {
            this.jitter = JitterParams::natural(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(button("j_strong", "强烈", cx, |this, _w, cx| {
            this.jitter = JitterParams::strong(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(button(
            "j_reroll",
            "换一套（重摇种子）",
            cx,
            |this, _w, cx| {
                this.jitter.seed = this.jitter.seed.wrapping_add(1);
                this.relayout();
                cx.notify();
            },
        ));

    let mut rows_a = div().flex().flex_col().gap_1();
    let mut rows_b = div().flex().flex_col().gap_1();
    rows_a = rows_a
        .child(param_row(
            "强度",
            format!("{:.2}", jitter.intensity),
            "ji",
            cx,
            |a| a.jitter.intensity = (a.jitter.intensity - 0.1).max(0.1),
            |a| a.jitter.intensity = (a.jitter.intensity + 0.1).min(5.0),
        ))
        .child(param_row(
            "字号波动 σ",
            format!("{:.3}", jitter.size_sigma),
            "js",
            cx,
            |a| a.jitter.size_sigma = (a.jitter.size_sigma - 0.01).max(0.0),
            |a| a.jitter.size_sigma = (a.jitter.size_sigma + 0.01).min(0.5),
        ))
        .child(param_row(
            "字符位移 σ(mm)",
            format!("{:.2}", jitter.char_x_sigma),
            "jx",
            cx,
            |a| {
                a.jitter.char_x_sigma = (a.jitter.char_x_sigma - 0.05).max(0.0);
                a.jitter.char_y_sigma = (a.jitter.char_y_sigma - 0.04).max(0.0);
            },
            |a| {
                a.jitter.char_x_sigma = (a.jitter.char_x_sigma + 0.05).min(3.0);
                a.jitter.char_y_sigma = (a.jitter.char_y_sigma + 0.04).min(3.0);
            },
        ))
        .child(param_row(
            "字符旋转 σ(°)",
            format!("{:.1}", jitter.char_rot_sigma),
            "jr",
            cx,
            |a| a.jitter.char_rot_sigma = (a.jitter.char_rot_sigma - 0.2).max(0.0),
            |a| a.jitter.char_rot_sigma = (a.jitter.char_rot_sigma + 0.2).min(15.0),
        ))
        .child(param_row(
            "行基线 σ(mm)",
            format!("{:.2}", jitter.line_y_sigma),
            "jy",
            cx,
            |a| a.jitter.line_y_sigma = (a.jitter.line_y_sigma - 0.05).max(0.0),
            |a| a.jitter.line_y_sigma = (a.jitter.line_y_sigma + 0.05).min(3.0),
        ));
    rows_b = rows_b
        .child(param_row(
            "行斜 σ(mm)",
            format!("{:.2}", jitter.line_tilt_sigma),
            "jt",
            cx,
            |a| a.jitter.line_tilt_sigma = (a.jitter.line_tilt_sigma - 0.05).max(0.0),
            |a| a.jitter.line_tilt_sigma = (a.jitter.line_tilt_sigma + 0.05).min(4.0),
        ))
        .child(param_row(
            "笔画旋转 σ(°)",
            format!("{:.1}", jitter.stroke_theta_sigma),
            "jst",
            cx,
            |a| a.jitter.stroke_theta_sigma = (a.jitter.stroke_theta_sigma - 0.5).max(0.0),
            |a| a.jitter.stroke_theta_sigma = (a.jitter.stroke_theta_sigma + 0.5).min(20.0),
        ))
        .child(param_row(
            "笔画修剪(mm)",
            format!("{:.2}", jitter.stroke_trim_mm),
            "jtr",
            cx,
            |a| a.jitter.stroke_trim_mm = (a.jitter.stroke_trim_mm - 0.01).max(0.0),
            |a| a.jitter.stroke_trim_mm = (a.jitter.stroke_trim_mm + 0.01).min(1.0),
        ))
        .child(param_row(
            "结构线起伏(mm)",
            format!("{:.2}", jitter.line_wobble),
            "jw",
            cx,
            |a| a.jitter.line_wobble = (a.jitter.line_wobble - 0.05).max(0.0),
            |a| a.jitter.line_wobble = (a.jitter.line_wobble + 0.05).min(2.0),
        ));

    div()
        .absolute()
        .top(px(52.))
        .right(px(12.))
        .w(px(620.))
        .bg(rgb(0x2b2b2b))
        .border_1()
        .border_color(rgb(0x555555))
        .rounded_lg()
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(rgb(0xffffff))
                        .child("字体与手写抖动"),
                )
                .child(button("dlg_close", "关闭", cx, |this, _w, cx| {
                    this.dialog = false;
                    cx.notify();
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x999999))
                .child("字体链按优先级匹配：靠前者负责绘制它能画的字符"),
        )
        .child(chips)
        .child(font_grid)
        .child(page_row)
        .child(div().h(px(1.)).bg(rgb(0x444444)))
        .child(preset_row)
        .child(div().flex().gap_3().child(rows_a).child(rows_b))
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.), px(730.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("笔阵 Bizhen".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| WriterApp::new(window, cx)),
        )
        .unwrap();
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}

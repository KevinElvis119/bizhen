//! `bizhen-app` —— Bizhen 的 GPUI 桌面端。
//!
//! 布局：顶部参数条 / 左侧 Markdown 编辑区 / 右侧分页预览 / 底部状态与机器栏。
//!
//! 说明：gpui 0.2 的核心 crate 没有多行文本输入控件（Zed 的编辑器在未发布的
//! `ui` crate 里），这里自绘了一个简易编辑器：字符输入、退格/删除、方向键、
//! Home/End、回车、Ctrl+V 粘贴、打开文件。中文输入法（IME）需要平台的组合
//! 输入支持，当前版本通过「打开文件 / 粘贴」录入中文。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Application, Bounds, Context,
    KeyDownEvent, PathBuilder, Render, SharedString, Window, WindowBounds, WindowOptions,
};

use bizhen_core::font::FontRegistry;
use bizhen_core::gcode::{self, GCodeConfig};
use bizhen_core::markdown::{self, MarkdownStyle, PageSpec, RenderedDoc};
use bizhen_core::serial::{self, PortInfo};
use bizhen_core::sim::JitterParams;
use bizhen_core::symbols::SymbolFonts;
use bizhen_core::Stroke;

/// 纸张预设（mm，纵向：名称、宽、高、页边距）
const PRESETS: &[(&str, f64, f64, f64)] = &[
    ("A4", 210.0, 297.0, 10.0),
    ("A5", 148.0, 210.0, 10.0),
    ("B5", 176.0, 250.0, 10.0),
    ("A3", 297.0, 420.0, 10.0),
    ("Letter", 215.9, 279.4, 12.7),
    ("明信片", 100.0, 148.0, 5.0),
];

const SAMPLE: &str = "# 静夜思\n\n床前明月光 疑是地上霜\n举头望明月 低头思故乡\n\n## 公式\n\n质能方程 $E = mc^2$，行内公式随正文重排。\n\n## 表格\n\n| 项目 | 数量 | 备注 |\n|:---|---:|:---:|\n| 笔 | 2 | 黑色 |\n| 纸 | 10 | A4 |\n\n![示意图](none.png){width=60,height=30}\n";

struct WriterApp {
    source: String,
    /// 光标位置（按字符计）
    cursor: usize,
    preset: usize,
    landscape: bool,
    margin: f64,
    size_mm: f64,
    wrap: f64,
    page: PageSpec,
    /// 字体注册表（含内置与用户导入的 `.gfont`）
    registry: FontRegistry,
    /// 字体链（按优先级排列的注册表键）
    font_names: Vec<String>,
    /// 手写抖动参数
    jitter: JitterParams,
    /// 「字体与抖动」弹窗是否打开
    dialog: bool,
    /// 弹窗里字体列表的翻页
    font_page: usize,
    symbols: SymbolFonts,
    doc: RenderedDoc,
    page_index: usize,
    zoom: f32,
    status: SharedString,
    ports: Vec<PortInfo>,
    port_idx: usize,
    sending: Arc<AtomicBool>,
    progress: Arc<Mutex<(usize, usize, String)>>,
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
        // 默认字体链：中文单线 + Hershey 拉丁
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
            source: SAMPLE.to_string(),
            cursor: SAMPLE.chars().count(),
            preset: 0,
            landscape: false,
            margin: page.margin,
            size_mm: 4.0,
            wrap: 160.0,
            page,
            registry,
            font_names,
            jitter: JitterParams::default(),
            // 冒烟/演示用：BIZHEN_DEMO_DIALOG=1 启动即打开弹窗
            dialog: std::env::var("BIZHEN_DEMO_DIALOG").is_ok(),
            font_page: 0,
            symbols,
            doc: RenderedDoc::default(),
            page_index: 0,
            zoom: 1.0,
            status: "就绪".into(),
            ports: serial::list_ports(),
            port_idx: 0,
            sending: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new((0, 0, String::new()))),
            focus: cx.focus_handle(),
        };
        app.relayout();
        let handle = app.focus.clone();
        window.focus(&handle);
        app
    }

    /// 按字体链加载字体（懒加载，首次用到才解析）。
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

    fn relayout(&mut self) {
        let page = self.effective_page();
        let style = MarkdownStyle {
            size: self.size_mm,
            wrap_width: self.wrap.min(page.content_width()),
            jitter: self.jitter.clone(),
            ..Default::default()
        };
        let fonts = self.chain_fonts();
        self.doc = markdown::render_markdown(&self.source, &fonts, &self.symbols, &style, page);
        if self.page_index >= self.doc.pages.len() {
            self.page_index = self.doc.pages.len().saturating_sub(1);
        }
        let stroke_count: usize = self.doc.pages.iter().map(|p| p.strokes.len()).sum();
        let jitter_note = if self.jitter.is_active() {
            format!(" · 抖动×{:.1}", self.jitter.intensity)
        } else {
            String::new()
        };
        self.status = format!(
            "{} 页 · {} 笔画{}{}",
            self.doc.pages.len(),
            stroke_count,
            jitter_note,
            if self.doc.missing.is_empty() {
                String::new()
            } else {
                format!(" · 缺字：{}", self.doc.missing.iter().collect::<String>())
            }
        )
        .into();
    }

    /// 导入字体：把选中的 `.gfont` / `.jhf` / `.json` 拷进用户字库目录并刷新。
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
        let total = self.source.chars().count();
        if self.cursor >= total {
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
            .set_file_name("writerstudio.gcode")
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
        self.status = if self.ports.is_empty() {
            "没有找到串口".into()
        } else {
            format!("{} 个串口", self.ports.len()).into()
        };
    }

    fn send_to_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending.load(Ordering::SeqCst) {
            return;
        }
        if self.ports.is_empty() {
            self.status = "没有可用串口，先刷新".into();
            cx.notify();
            return;
        }
        let port = self.ports[self.port_idx].name.clone();
        let page = self.effective_page();
        let cfg = GCodeConfig::default();
        let lines = gcode::generate(&self.doc.strokes(), &cfg, page.height, None).lines;
        let total = lines.len();
        let sending = self.sending.clone();
        let progress = self.progress.clone();
        sending.store(true, Ordering::SeqCst);
        {
            let mut p = progress.lock().unwrap();
            *p = (0, total, format!("正在发送到 {port}…"));
        }
        std::thread::spawn(move || {
            let outcome = match serial::GrblLink::open(&port, 115200) {
                Ok(mut link) => {
                    let _ = link.unlock();
                    link.send_program(&lines, |done, total| {
                        let mut p = progress.lock().unwrap();
                        *p = (done, total, format!("发送中 {done}/{total}"));
                    })
                }
                Err(e) => Err(e),
            };
            let mut p = progress.lock().unwrap();
            *p = match outcome {
                Ok(()) => (total, total, "发送完成".into()),
                Err(e) => (0, total, format!("发送失败：{e}")),
            };
            sending.store(false, Ordering::SeqCst);
        });
        cx.notify();
        Self::poll_send(self, window, cx);
    }

    /// 发送期间逐帧刷新进度。
    fn poll_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (done, total, msg) = {
            let p = self.progress.lock().unwrap();
            (p.0, p.1, p.2.clone())
        };
        self.status = if total > 0 {
            format!("{msg}（{done}/{total}）").into()
        } else {
            msg.into()
        };
        if self.sending.load(Ordering::SeqCst) {
            cx.notify();
            cx.on_next_frame(window, |this, window, cx| Self::poll_send(this, window, cx));
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

impl Render for WriterApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.effective_page();
        let current: Vec<Stroke> = self
            .doc
            .pages
            .get(self.page_index)
            .map(|p| p.strokes.clone())
            .unwrap_or_default();
        let zoom = self.zoom;
        let (cursor_line, cursor_col) = self.cursor_line_col();
        let lines: Vec<String> = self.source.split('\n').map(|s| s.to_string()).collect();
        let total_pages = self.doc.pages.len().max(1);
        let status = self.status.clone();
        let focus = self.focus.clone();
        let preset_name = PRESETS[self.preset].0;
        let port_name = self
            .ports
            .get(self.port_idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "无串口".into());
        let landscape = self.landscape;
        let page_text = format!("第 {} / {} 页", self.page_index + 1, total_pages);

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
            .child(button(
                "preset",
                format!("纸张 {preset_name}"),
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
                if landscape { "横向" } else { "纵向" },
                cx,
                |this, _w, cx| {
                    this.landscape = !this.landscape;
                    this.relayout();
                    cx.notify();
                },
            ))
            .child(button("size_down", "字号−", cx, |this, _w, cx| {
                this.size_mm = (this.size_mm - 0.5).max(1.5);
                this.relayout();
                cx.notify();
            }))
            .child(button("size_up", "字号＋", cx, |this, _w, cx| {
                this.size_mm = (this.size_mm + 0.5).min(20.0);
                this.relayout();
                cx.notify();
            }))
            .child(button("zoom_out", "缩放−", cx, |this, _w, cx| {
                this.zoom = (this.zoom - 0.1).max(0.3);
                cx.notify();
            }))
            .child(button("zoom_in", "缩放＋", cx, |this, _w, cx| {
                this.zoom = (this.zoom + 0.1).min(4.0);
                cx.notify();
            }))
            .child(button("open", "打开…", cx, |this, _w, cx| {
                this.open_file();
                cx.notify();
            }))
            .child(button("export", "导出", cx, |this, _w, cx| {
                this.export_gcode();
                cx.notify();
            }))
            .child(button("style", "字体与抖动", cx, |this, _w, cx| {
                this.dialog = !this.dialog;
                cx.notify();
            }));

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
            .child(div().text_xs().text_color(rgb(0x808080)).child(page_text))
            .child(button("prev", "◀", cx, |this, _w, cx| {
                this.page_index = this.page_index.saturating_sub(1);
                cx.notify();
            }))
            .child(button("next", "▶", cx, |this, _w, cx| {
                if this.page_index + 1 < this.doc.pages.len().max(1) {
                    this.page_index += 1;
                }
                cx.notify();
            }))
            .child(button(
                "port",
                format!("串口 {port_name}"),
                cx,
                |this, _w, cx| {
                    if !this.ports.is_empty() {
                        this.port_idx = (this.port_idx + 1) % this.ports.len();
                    }
                    cx.notify();
                },
            ))
            .child(button("refresh", "刷新端口", cx, |this, _w, cx| {
                this.refresh_ports();
                cx.notify();
            }))
            .child(button("print", "打印", cx, |this, w, cx| {
                this.send_to_machine(w, cx);
            }));

        // 编辑区：逐行显示，光标处画一条竖线
        let mut editor = div().flex().flex_col();
        for (idx, line) in lines.iter().enumerate() {
            let row =
                if idx == cursor_line {
                    let before: String = line.chars().take(cursor_col).collect();
                    let after: String = line.chars().skip(cursor_col).collect();
                    div()
                        .flex()
                        .items_center()
                        .child(div().text_xs().text_color(rgb(0xdddddd)).child(before))
                        .child(div().w(px(1.5)).h(px(13.)).bg(rgb(0x3b82f6)))
                        .child(div().text_xs().text_color(rgb(0xdddddd)).child(after))
                } else {
                    div().child(div().text_xs().text_color(rgb(0xbbbbbb)).child(
                        if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.clone()
                        },
                    ))
                };
            editor = editor.child(row);
        }

        let page_w = page.width;
        let page_h = page.height;
        let margin = page.margin;

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
                .child(
                    div()
                        .w(px(380.))
                        .h_full()
                        .overflow_hidden()
                        .p_3()
                        .border_r_1()
                        .border_color(rgb(0x333333))
                        .track_focus(&focus)
                        .on_key_down(cx.listener(Self::on_key))
                        .child(editor),
                )
                .child(
                    div().flex_1().h_full().p_4().child(
                        canvas(
                            move |_, _, _| {},
                            move |bounds, _state, window, _cx| {
                                let avail_w = f32::from(bounds.size.width) - 8.0;
                                let avail_h = f32::from(bounds.size.height) - 8.0;
                                let scale = ((avail_w / page_w as f32)
                                    .min(avail_h / page_h as f32))
                                .max(0.05)
                                    * zoom;
                                let paper_w = page_w as f32 * scale;
                                let paper_h = page_h as f32 * scale;
                                let ox = f32::from(bounds.origin.x) + (avail_w - paper_w) / 2.0;
                                let oy = f32::from(bounds.origin.y) + (avail_h - paper_h) / 2.0;

                                // 纸张
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

                                // 页边距参考线
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

                                // 笔画
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
                            },
                        )
                        .size_full(),
                    ),
                ),
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
        root
    }
}

/// 一行参数：标签 + 「−」按钮 + 当前值 + 「＋」按钮。
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

    // 字体链
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

    // 可加入的字体（分页）
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

    // 抖动预设
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
        .top(px(56.))
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
        let bounds = Bounds::centered(None, size(px(940.), px(700.)), cx);
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

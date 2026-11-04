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
    fonts: Vec<bizhen_core::font::FontFamily>,
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
        let mut reg = FontRegistry::builtin();
        let mut fonts = Vec::new();
        if let Some(f) = reg.get("STRK-Kaiti") {
            fonts.push(f.clone());
        }
        if let Some(f) = reg.get("futural") {
            fonts.push(f.clone());
        }
        let symbols = SymbolFonts::load();
        let page = PageSpec {
            width: PRESETS[0].1,
            height: PRESETS[0].2,
            margin: PRESETS[0].3,
        };
        let mut app = Self {
            source: SAMPLE.to_string(),
            cursor: SAMPLE.chars().count(),
            preset: 0,
            landscape: false,
            margin: page.margin,
            size_mm: 4.0,
            wrap: 160.0,
            page,
            fonts,
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
            ..Default::default()
        };
        self.doc =
            markdown::render_markdown(&self.source, &self.fonts, &self.symbols, &style, page);
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
            .child(button("export", "导出 G-code", cx, |this, _w, cx| {
                this.export_gcode();
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

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1b1b1b))
            .child(header)
            .child(
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
                                            size: size(
                                                px(paper_w - 2.0 * m),
                                                px(paper_h - 2.0 * m),
                                            ),
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
            )
            .child(footer)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1120.), px(740.)), cx);
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


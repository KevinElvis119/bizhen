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
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, ContentMask, Context, Entity,
    PathBuilder, Render, SharedString, Subscription, WeakEntity, Window, WindowBounds,
    WindowOptions,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;

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

/// 编辑器字号（pt）
const EDITOR_TEXT_SIZE: f32 = 13.0;

const SAMPLE: &str = "# 静夜思\n\n床前明月光 疑是地上霜\n举头望明月 低头思故乡\n\n## 公式\n\n质能方程 $E = mc^2$，行内公式随正文重排。\n\n## 表格\n\n| 项目 | 数量 | 备注 |\n|:---|---:|:---:|\n| 笔 | 2 | 黑色 |\n| 纸 | 10 | A4 |\n\n![示意图](none.png){width=60,height=30}\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Markdown,
    Page,
}

struct WriterApp {
    source: String,
    /// Kit 的多行输入状态：自带输入法组合、选区、撤销、鼠标定位
    editor: Entity<EditorState>,
    _editor_sub: Option<Subscription>,
    tab: Tab,

    /// 当前纸张预设；None 表示自定义尺寸
    preset: Option<usize>,
    landscape: bool,
    size_mm: f64,
    wrap: f64,
    table_width: f64,
    page: PageSpec,

    registry: FontRegistry,
    font_names: Vec<String>,
    jitter: JitterParams,
    /// 冒烟/演示：BIZHEN_DEMO_DIALOG=1 时首帧自动打开弹窗
    auto_dialog: bool,
    /// 异步对话框选好的待打开文件（文本, 显示名）
    pending_open: Option<(String, String)>,
    /// 异步对话框选好的待导入字体
    pending_font: Option<std::path::PathBuf>,
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
    jog_step: f64,
    jog_feed: f64,
    /// 发出机器命令后保持刷新若干帧，等后台线程回报状态
    poll_frames: u32,
    /// 是否已经挂了 on_next_frame 链，避免重复挂
    ticking: bool,
}

impl WriterApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let registry = FontRegistry::builtin();
        let symbols = SymbolFonts::load();
        let page = PageSpec::with_margin(PRESETS[0].1, PRESETS[0].2, PRESETS[0].3);
        let mut font_names: Vec<String> = Vec::new();
        for want in ["STRK-Kaiti", "futural"] {
            if registry.get_entry(want).is_some() {
                font_names.push(want.to_string());
            }
        }
        if font_names.is_empty() {
            font_names = registry.names().into_iter().take(1).collect();
        }
        let source = std::env::var("BIZHEN_DEMO_SOURCE")
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_else(|| SAMPLE.to_string());
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .default_value(source.clone())
                .language("markdown")
        });
        let mut app = Self {
            source,
            editor,
            _editor_sub: None,
            // 冒烟/演示：BIZHEN_DEMO_TAB=page 启动即显示「页面」标签
            tab: if std::env::var("BIZHEN_DEMO_TAB").as_deref() == Ok("page") {
                Tab::Page
            } else {
                Tab::Markdown
            },
            preset: Some(0),
            landscape: false,
            size_mm: 4.0,
            wrap: 160.0,
            table_width: 0.0,
            page,
            registry,
            font_names,
            jitter: JitterParams::default(),
            // 演示/冒烟：BIZHEN_DEMO_DIALOG=1 启动即打开弹窗
            auto_dialog: std::env::var("BIZHEN_DEMO_DIALOG").is_ok(),
            pending_open: None,
            pending_font: None,
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
            jog_step: 10.0,
            jog_feed: 2500.0,
            poll_frames: 0,
            ticking: false,
        };
        // 编辑内容变化 → 重排预览
        app._editor_sub = Some(cx.subscribe(
            &app.editor,
            |this: &mut WriterApp, editor, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.source = editor.read(cx).value().to_string();
                    this.relayout();
                    cx.notify();
                }
            },
        ));
        app.relayout();
        // 冒烟/演示：BIZHEN_DEMO_OPEN=路径 启动即走一次「打开文件」流程
        if let Ok(path) = std::env::var("BIZHEN_DEMO_OPEN") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                app.pending_open = Some((text, path.clone()));
            }
        }
        // 冒烟/演示：BIZHEN_DEMO_CONNECT=1 启动即连接第一个串口
        if std::env::var("BIZHEN_DEMO_CONNECT").is_ok() {
            if let Some(port) = app.ports.first().map(|p| p.name.clone()) {
                let baud = BAUDS[app.baud_idx.min(BAUDS.len() - 1)];
                app.machine
                    .set_message(format!("正在连接 {port} @ {baud}…"));
                app.machine.send(machine::Cmd::Connect { port, baud });
                app.poke();
            }
        }
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
            ..self.page
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

    /// 选字体文件（异步对话框，避免模态重入导致崩溃）。
    fn import_font(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new()
                .add_filter("单线字体", &["gfont", "jhf", "json"])
                .pick_file()
                .await;
            let Some(handle) = picked else { return };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |app: &mut WriterApp, cx| {
                app.pending_font = Some(path);
                cx.notify();
            });
        })
        .detach();
    }

    /// 把选好的字体拷进用户字库并刷新。
    fn do_import_font(&mut self, path: std::path::PathBuf) -> String {
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

    // -- 操作 ---------------------------------------------------------------
    /// 打开文件：用异步对话框，避免在实体更新中跑 Windows 模态循环
    /// （之前第二次点「打开」会因为重入而崩）。
    fn open_file(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .pick_file()
                .await;
            let Some(handle) = picked else { return };
            let path = handle.path().to_path_buf();
            let text = std::fs::read_to_string(&path);
            let _ = this.update(cx, |app: &mut WriterApp, cx| {
                match text {
                    Ok(text) => {
                        // 真正写进编辑器要 Window，放到下一帧 render 里做
                        app.pending_open = Some((text, path.display().to_string()));
                    }
                    Err(e) => app.status = format!("打开失败：{e}").into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 导出 G-code：同样用异步保存对话框。
    fn export_gcode(&mut self, cx: &mut Context<Self>) {
        let page = self.effective_page();
        let cfg = GCodeConfig::default();
        let result = gcode::generate(&self.doc.strokes(), &cfg, page.height, None);
        let text = result.text().to_string();
        let summary = format!(
            "绘制 {:.1} mm · 预计 {}",
            result.draw_length,
            gcode::format_duration(result.estimated_seconds)
        );
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new()
                .set_file_name("bizhen.gcode")
                .save_file()
                .await;
            let Some(handle) = picked else { return };
            let path = handle.path().to_path_buf();
            let res = std::fs::write(&path, text);
            let _ = this.update(cx, |app: &mut WriterApp, cx| {
                app.status = match res {
                    Ok(()) => format!("已导出 {} · {summary}", path.display()).into(),
                    Err(e) => format!("导出失败：{e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
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

    /// 发过机器命令后保持一段时间刷新，等后台线程回报状态。
    fn poke(&mut self) {
        self.poll_frames = 180;
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
            // 打开串口可能被驱动卡住几秒，先给界面一句「正在连接」，别让用户干等
            self.machine
                .set_message(format!("正在连接 {port} @ {baud}…"));
            self.machine.send(machine::Cmd::Connect { port, baud });
        }
        self.poke();
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
        self.poke();
        cx.notify();
    }

    /// 每帧刷新机器状态：已连接或刚发过命令时保持循环。
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let st = self.machine.snapshot();
        if !st.connected && !st.message.is_empty() {
            self.status = st.message.clone().into();
        }
        self.poll_frames = self.poll_frames.saturating_sub(1);
        self.ticking = false;
        cx.notify();
        if st.connected || st.pending || self.poll_frames > 0 {
            self.ticking = true;
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

impl Render for WriterApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let __perf = std::env::var("BIZHEN_PERF").is_ok();
        let __t0 = std::time::Instant::now();
        // 异步对话框选好的文件在这里落盘/落编辑器（此处才有 Window）
        if let Some(path) = self.pending_font.take() {
            self.status = self.do_import_font(path).into();
        }
        if let Some((text, name)) = self.pending_open.take() {
            self.source = text.clone();
            self.editor
                .update(cx, |s, cx| s.set_value(text, window, cx));
            self.relayout();
            self.status = format!("已打开 {name}").into();
        }

        let page = self.effective_page();
        let current: Vec<Stroke> = self
            .doc
            .pages
            .get(self.page_index)
            .map(|p| p.strokes.clone())
            .unwrap_or_default();
        let zoom = self.zoom;
        let total_pages = self.doc.pages.len().max(1);
        let status = self.status.clone();
        let size_mm_footer = self.size_mm;
        let wrap_footer = self.wrap;
        let preset_name = match self.preset {
            Some(i) => PRESETS[i.min(PRESETS.len() - 1)].0,
            None => "自定义",
        };
        let landscape = self.landscape;
        let tab = self.tab;
        let machine_state = self.machine.snapshot();
        let port_name = self
            .ports
            .get(self.port_idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "无串口".into());
        let baud = BAUDS[self.baud_idx.min(BAUDS.len() - 1)];

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
            .child(button("open", "打开…", cx, |this, w, cx| {
                this.open_file(cx);
                cx.notify();
            }))
            .child(button("export", "导出 G-code", cx, |this, _w, cx| {
                this.export_gcode(cx);
                cx.notify();
            }))
            .child(button(
                "style",
                "字体与抖动",
                cx,
                |_this, window, cx| {
                    open_style_dialog(window, cx);
                },
            ))
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
            // Kit 的代码编辑器模式：行号 + Markdown 语法高亮 + 输入法
            div().flex_1().min_h(px(0.)).p_2().child(
                Editor::new(&self.editor)
                    .h_full()
                    .w_full()
                    .text_size(px(EDITOR_TEXT_SIZE)),
            )
        } else {
            // 纸张：Kit 下拉菜单（预设 + 自定义）
            let paper_menu = {
                let weak = cx.entity().downgrade();
                let selected = self.preset;
                Button::new("paper")
                    .label(format!("纸张 {preset_name} ▾"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (i, (name, w, h, m)) in PRESETS.iter().enumerate() {
                            let weak = weak.clone();
                            let (w, h, m) = (*w, *h, *m);
                            menu = menu.item(
                                PopupMenuItem::new((*name).to_string())
                                    .checked(selected == Some(i))
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |app: &mut WriterApp, cx| {
                                            app.preset = Some(i);
                                            app.page = PageSpec::with_margin(w, h, m);
                                            app.relayout();
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        let weak_c = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new("自定义")
                                .checked(selected.is_none())
                                .on_click(move |_, _, cx| {
                                    let _ = weak_c.update(cx, |app: &mut WriterApp, cx| {
                                        app.preset = None;
                                        cx.notify();
                                    });
                                }),
                        );
                        menu
                    })
            };

            let mut panel = div().flex().flex_col().gap_1().p_3();
            panel = panel
                .child(div().text_xs().text_color(rgb(0x888888)).child("纸张"))
                .child(paper_menu)
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
                .child(setting_row(
                    "纸宽 (mm)",
                    format!("{:.1}", self.page.width),
                    "pw",
                    cx,
                    |a| {
                        a.page.width = (a.page.width - 5.0).max(40.0);
                        a.preset = None;
                        a.relayout();
                    },
                    |a| {
                        a.page.width = (a.page.width + 5.0).min(1000.0);
                        a.preset = None;
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "纸高 (mm)",
                    format!("{:.1}", self.page.height),
                    "ph",
                    cx,
                    |a| {
                        a.page.height = (a.page.height - 5.0).max(40.0);
                        a.preset = None;
                        a.relayout();
                    },
                    |a| {
                        a.page.height = (a.page.height + 5.0).min(1000.0);
                        a.preset = None;
                        a.relayout();
                    },
                ))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("页边距（四边独立）"),
                )
                .child(setting_row(
                    "上 (mm)",
                    format!("{:.1}", self.page.margin_top),
                    "mt",
                    cx,
                    |a| {
                        a.page.margin_top = (a.page.margin_top - 1.0).max(0.0);
                        a.relayout();
                    },
                    |a| {
                        a.page.margin_top = (a.page.margin_top + 1.0).min(60.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "下 (mm)",
                    format!("{:.1}", self.page.margin_bottom),
                    "mb",
                    cx,
                    |a| {
                        a.page.margin_bottom = (a.page.margin_bottom - 1.0).max(0.0);
                        a.relayout();
                    },
                    |a| {
                        a.page.margin_bottom = (a.page.margin_bottom + 1.0).min(60.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "左 (mm)",
                    format!("{:.1}", self.page.margin_left),
                    "ml",
                    cx,
                    |a| {
                        a.page.margin_left = (a.page.margin_left - 1.0).max(0.0);
                        a.relayout();
                    },
                    |a| {
                        a.page.margin_left = (a.page.margin_left + 1.0).min(60.0);
                        a.relayout();
                    },
                ))
                .child(setting_row(
                    "右 (mm)",
                    format!("{:.1}", self.page.margin_right),
                    "mr",
                    cx,
                    |a| {
                        a.page.margin_right = (a.page.margin_right - 1.0).max(0.0);
                        a.relayout();
                    },
                    |a| {
                        a.page.margin_right = (a.page.margin_right + 1.0).min(60.0);
                        a.relayout();
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

        let preview = div().flex_1().min_w(px(0.)).h_full().p_3().child(
            canvas(
                move |_, _, _| {},
                move |bounds, _state, window, _cx| {
                    // 裁剪到画布范围：缩放后纸张不会盖住工具栏
                    let clip = Bounds {
                        origin: bounds.origin,
                        size: bounds.size,
                    };
                    let __p0 = std::time::Instant::now();
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

                        // 四边独立的页边距参考框
                        let ml = page.margin_left as f32 * scale;
                        let mt = page.margin_top as f32 * scale;
                        let mr = page.margin_right as f32 * scale;
                        let mb = page.margin_bottom as f32 * scale;
                        window.paint_quad(quad(
                            Bounds {
                                origin: point(px(ox + ml), px(oy + mt)),
                                size: size(px(paper_w - ml - mr), px(paper_h - mt - mb)),
                            },
                            px(0.),
                            gpui::transparent_black(),
                            px(0.5),
                            rgb(0xdddddd),
                            Default::default(),
                        ));

                        // 整页笔画合成一条 Path（多子路径）：每帧只 build 一次，
                        // 比「每笔一个 builder」快一个数量级，长文档才不卡
                        let line_width = (0.35_f64 * scale as f64).max(0.9) as f32;
                        let mut builder = PathBuilder::stroke(px(line_width));
                        for stroke in current.iter() {
                            if stroke.points.len() < 2 {
                                continue;
                            }
                            for (i, p) in stroke.points.iter().enumerate() {
                                let x = ox + p.0 as f32 * scale;
                                let y = oy + (page_h - p.1) as f32 * scale;
                                if i == 0 {
                                    builder.move_to(point(px(x), px(y)));
                                } else {
                                    builder.line_to(point(px(x), px(y)));
                                }
                            }
                        }
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, rgb(0x111111));
                        }
                    });
                },
            )
            .size_full(),
        );

        // ---------------- 右侧机器栏 ----------------
        // 串口 / 波特率：用 Kit 的下拉菜单（Button + PopupMenu）
        let app_weak = cx.entity().downgrade();
        let ports_snapshot: Vec<String> = self
            .ports
            .iter()
            .map(|p| format!("{} {}", p.name, p.description))
            .collect();
        let selected_port = self.port_idx;
        let selected_baud = self.baud_idx;
        let port_menu = {
            let weak = app_weak.clone();
            let items = ports_snapshot.clone();
            Button::new("port")
                .label(format!("{port_name} ▾"))
                .dropdown_menu(move |mut menu, _, _| {
                    if items.is_empty() {
                        menu = menu.item(PopupMenuItem::new("没有串口").disabled(true));
                    }
                    for (i, label) in items.iter().enumerate() {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(i == selected_port)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |app: &mut WriterApp, cx| {
                                        app.port_idx = i;
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let baud_menu = {
            let weak = app_weak.clone();
            Button::new("baud")
                .label(format!("{baud} 波特 ▾"))
                .dropdown_menu(move |mut menu, _, _| {
                    for (i, b) in BAUDS.iter().enumerate() {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{b}"))
                                .checked(i == selected_baud)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |app: &mut WriterApp, cx| {
                                        app.baud_idx = i;
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let mut machine_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .child(div().text_xs().text_color(rgb(0x888888)).child("串口"))
            .child(port_menu)
            .child(button(
                "refresh_ports",
                "刷新端口",
                cx,
                |this, _w, cx| {
                    this.refresh_ports();
                    cx.notify();
                },
            ))
            .child(baud_menu);
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
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child(machine_state.message.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(button("unlock", "解锁", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Unlock);
                            this.poke();
                            cx.notify();
                        }))
                        .child(button("home", "回零", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Home);
                            this.poke();
                            cx.notify();
                        }))
                        .child(button("reset", "复位", cx, |this, _w, cx| {
                            this.machine.send(machine::Cmd::Reset);
                            this.poke();
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
                    this.poke();
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
            .child(div().text_xs().text_color(rgb(0x777777)).child(format!(
                "{:.1} mm · 折行 {:.0}",
                size_mm_footer, wrap_footer
            )));

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

        // 冒烟/演示：首帧自动打开弹窗
        if self.auto_dialog {
            self.auto_dialog = false;
            open_style_dialog(window, cx);
        }

        if __perf {
            eprintln!(
                "[perf] render {:.1} ms",
                __t0.elapsed().as_secs_f64() * 1000.0
            );
        }

        // 已连接或刚发过机器命令时刷新状态（只在没有现成链时起一条）
        if (machine_state.connected || machine_state.pending || self.poll_frames > 0)
            && !self.ticking
        {
            self.ticking = true;
            cx.on_next_frame(window, |this, window, cx| Self::tick(this, window, cx));
        }

        root
    }
}

/// 用 Kit 的 Dialog 打开「字体与手写抖动」面板（每帧重建，数值始终跟着状态走）。
fn open_style_dialog(window: &mut Window, cx: &mut Context<WriterApp>) {
    let weak = cx.entity().downgrade();
    window.open_dialog(cx, move |dialog, _window, cx| {
        build_style_dialog(dialog, &weak, cx)
    });
}

/// 弹窗里的按钮：拿不到 Context<WriterApp>，用 WeakEntity 回写状态。
fn wb(
    weak: &WeakEntity<WriterApp>,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&mut WriterApp, &mut Context<WriterApp>) + 'static,
) -> impl IntoElement {
    let weak = weak.clone();
    Button::new(id.into())
        .label(label.into())
        .on_click(move |_, _window, cx| {
            let _ = weak.update(cx, |app: &mut WriterApp, cx| on_click(app, cx));
        })
}

/// 弹窗里的一行参数（−/+），改完自动重排预览。
fn w_param(
    weak: &WeakEntity<WriterApp>,
    label: &str,
    value: String,
    id: &str,
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
        .child(wb(
            weak,
            SharedString::from(format!("{id}_dec")),
            "−",
            move |a: &mut WriterApp, cx| {
                dec(a);
                a.relayout();
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
        .child(wb(
            weak,
            SharedString::from(format!("{id}_inc")),
            "＋",
            move |a: &mut WriterApp, cx| {
                inc(a);
                a.relayout();
                cx.notify();
            },
        ))
}

/// 「字体与手写抖动」的弹窗内容。
fn build_style_dialog(dialog: Dialog, weak: &WeakEntity<WriterApp>, cx: &mut App) -> Dialog {
    const PER_PAGE: usize = 9;

    let Some(handle) = weak.upgrade() else {
        return dialog;
    };
    let (chain, entries, jitter, page, size_mm) = {
        let app = handle.read(cx);
        let entries: Vec<(String, String, String)> = app
            .registry
            .entries()
            .iter()
            .map(|e| (e.name.clone(), e.display.clone(), e.kind.to_string()))
            .collect();
        (
            app.font_names.clone(),
            entries,
            app.jitter.clone(),
            app.font_page,
            app.size_mm,
        )
    };

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

    // ---- 字体链 ----
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
                .child(wb(weak, format!("up_{i}"), "↑", move |this, cx| {
                    if let Some(pos) = this.font_names.iter().position(|c| c == &up) {
                        if pos > 0 {
                            this.font_names.swap(pos - 1, pos);
                        }
                    }
                    this.relayout();
                    cx.notify();
                }))
                .child(wb(weak, format!("del_{i}"), "×", move |this, cx| {
                    if this.font_names.len() <= 1 {
                        return;
                    }
                    if let Some(pos) = this.font_names.iter().position(|c| c == &del) {
                        this.font_names.remove(pos);
                    }
                    this.relayout();
                    cx.notify();
                })),
        );
    }

    // ---- 可选字体（分页）----
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
                .child(wb(weak, format!("pick_{name}"), label, move |this, cx| {
                    if let Some(pos) = this.font_names.iter().position(|c| c == &n) {
                        this.font_names.remove(pos);
                    } else {
                        this.font_names.push(n.clone());
                    }
                    this.relayout();
                    cx.notify();
                }))
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
        .child(wb(weak, "font_prev", "◀", |this, cx| {
            this.font_page = this.font_page.saturating_sub(1);
            cx.notify();
        }))
        .child(div().text_xs().text_color(rgb(0x999999)).child(format!(
            "字体 {}/{}（共 {} 款）",
            page + 1,
            pages,
            total
        )))
        .child(wb(weak, "font_next", "▶", |this, cx| {
            this.font_page += 1;
            cx.notify();
        }))
        .child(wb(weak, "import_font", "导入字体…", |this, cx| {
            this.import_font(cx);
            cx.notify();
        }));

    // ---- 抖动预设 ----
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
        .child(wb(weak, "j_off", "关闭", |this, cx| {
            this.jitter = JitterParams::default();
            this.relayout();
            cx.notify();
        }))
        .child(wb(weak, "j_subtle", "轻微", |this, cx| {
            this.jitter = JitterParams::subtle(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(wb(weak, "j_natural", "自然", |this, cx| {
            this.jitter = JitterParams::natural(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(wb(weak, "j_strong", "强烈", |this, cx| {
            this.jitter = JitterParams::strong(this.size_mm);
            this.relayout();
            cx.notify();
        }))
        .child(wb(
            weak,
            "j_reroll",
            "换一套（重摇种子）",
            |this, cx| {
                this.jitter.seed = this.jitter.seed.wrapping_add(1);
                this.relayout();
                cx.notify();
            },
        ));

    // ---- 参数 ----
    let mut rows_a = div().flex().flex_col().gap_1();
    let mut rows_b = div().flex().flex_col().gap_1();
    let mut rows_c = div().flex().flex_col().gap_1();
    rows_a = rows_a
        .child(w_param(
            weak,
            "强度",
            format!("{:.2}", jitter.intensity),
            "ji",
            |a| a.jitter.intensity = (a.jitter.intensity - 0.1).max(0.1),
            |a| a.jitter.intensity = (a.jitter.intensity + 0.1).min(5.0),
        ))
        .child(w_param(
            weak,
            "字号波动 σ",
            format!("{:.3}", jitter.size_sigma),
            "js",
            |a| a.jitter.size_sigma = (a.jitter.size_sigma - 0.01).max(0.0),
            |a| a.jitter.size_sigma = (a.jitter.size_sigma + 0.01).min(0.5),
        ))
        .child(w_param(
            weak,
            "字符位移 σ(mm)",
            format!("{:.2}", jitter.char_x_sigma),
            "jx",
            |a| {
                a.jitter.char_x_sigma = (a.jitter.char_x_sigma - 0.05).max(0.0);
                a.jitter.char_y_sigma = (a.jitter.char_y_sigma - 0.04).max(0.0);
            },
            |a| {
                a.jitter.char_x_sigma = (a.jitter.char_x_sigma + 0.05).min(3.0);
                a.jitter.char_y_sigma = (a.jitter.char_y_sigma + 0.04).min(3.0);
            },
        ));
    rows_b = rows_b
        .child(w_param(
            weak,
            "字符旋转 σ(°)",
            format!("{:.1}", jitter.char_rot_sigma),
            "jr",
            |a| a.jitter.char_rot_sigma = (a.jitter.char_rot_sigma - 0.2).max(0.0),
            |a| a.jitter.char_rot_sigma = (a.jitter.char_rot_sigma + 0.2).min(15.0),
        ))
        .child(w_param(
            weak,
            "行基线 σ(mm)",
            format!("{:.2}", jitter.line_y_sigma),
            "jy",
            |a| a.jitter.line_y_sigma = (a.jitter.line_y_sigma - 0.05).max(0.0),
            |a| a.jitter.line_y_sigma = (a.jitter.line_y_sigma + 0.05).min(3.0),
        ))
        .child(w_param(
            weak,
            "行斜 σ(mm)",
            format!("{:.2}", jitter.line_tilt_sigma),
            "jt",
            |a| a.jitter.line_tilt_sigma = (a.jitter.line_tilt_sigma - 0.05).max(0.0),
            |a| a.jitter.line_tilt_sigma = (a.jitter.line_tilt_sigma + 0.05).min(4.0),
        ));
    rows_c = rows_c
        .child(w_param(
            weak,
            "笔画旋转 σ(°)",
            format!("{:.1}", jitter.stroke_theta_sigma),
            "jst",
            |a| a.jitter.stroke_theta_sigma = (a.jitter.stroke_theta_sigma - 0.5).max(0.0),
            |a| a.jitter.stroke_theta_sigma = (a.jitter.stroke_theta_sigma + 0.5).min(20.0),
        ))
        .child(w_param(
            weak,
            "笔画修剪(mm)",
            format!("{:.2}", jitter.stroke_trim_mm),
            "jtr",
            |a| a.jitter.stroke_trim_mm = (a.jitter.stroke_trim_mm - 0.01).max(0.0),
            |a| a.jitter.stroke_trim_mm = (a.jitter.stroke_trim_mm + 0.01).min(1.0),
        ))
        .child(w_param(
            weak,
            "结构线起伏(mm)",
            format!("{:.2}", jitter.line_wobble),
            "jw",
            |a| a.jitter.line_wobble = (a.jitter.line_wobble - 0.05).max(0.0),
            |a| a.jitter.line_wobble = (a.jitter.line_wobble + 0.05).min(2.0),
        ));

    let _ = size_mm;

    dialog
        .title("字体与手写抖动")
        .w(px(780.))
        .close_button(true)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(chips)
                .child(font_grid)
                .child(page_row)
                .child(div().h(px(1.)).bg(rgb(0x444444)))
                .child(preset_row)
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(rows_a)
                        .child(rows_b)
                        .child(rows_c),
                ),
        )
        .footer(
            div().flex().justify_end().gap_2().child(
                Button::new("dlg_close")
                    .label("关闭")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            ),
        )
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        // 初始化组件库的主题、图标与字体（用 Textarea 等控件前必须调用）
        gpui_kit::init(cx);
        gpui_kit::component::theme::Theme::change(
            gpui_kit::component::theme::ThemeMode::Dark,
            None,
            cx,
        );
        let bounds = Bounds::centered(None, size(px(1000.), px(730.)), cx);
        // 用 Kit 的入口打开窗口：它会挂上 Root，菜单/弹窗等浮层才有地方落
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("笔阵 Bizhen".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| WriterApp::new(window, cx)),
        )
        .expect("打开窗口失败");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}

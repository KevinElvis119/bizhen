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
    canvas, div, point, prelude::*, px, quad, rgb, size, AnyElement, App, Bounds, ContentMask,
    Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent,
    PathBuilder, Render, ScrollEvent, SharedString, Subscription, WeakEntity, Window, WindowBounds,
    WindowOptions,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel as KitPanel, PanelEvent,
    PanelState,
};

use bizhen_core::font::FontRegistry;
use bizhen_core::gcode::{self, GCodeConfig};
use bizhen_core::markdown::{self, MarkdownStyle, PageSpec, RenderedDoc};
use bizhen_core::serial::{self, PortInfo};
use bizhen_core::sim::JitterParams;
use bizhen_core::symbols::SymbolFonts;
use bizhen_core::Stroke;

use machine::Machine;


// ---------------- Dock 面板 ----------------

/// 面板内容分区：同一个 WriterApp 的 render 体按分区返回不同片段。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    All,
    Markdown,
    Page,
    Preview,
    Machine,
}

impl Part {
    fn label(self) -> &'static str {
        match self {
            Part::All => "笔阵",
            Part::Markdown => "编辑",
            Part::Page => "页面",
            Part::Preview => "预览",
            Part::Machine => "机器控制",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Part::All => "All",
            Part::Markdown => "Markdown",
            Part::Page => "Page",
            Part::Preview => "Preview",
            Part::Machine => "Machine",
        }
    }
}

/// 侧栏面板：内容由 WriterApp 渲染（`app.update` 取得），自身不持有状态。
struct AppPanel {
    part: Part,
    app: WeakEntity<WriterApp>,
    focus: FocusHandle,
}

impl AppPanel {
    fn new(part: Part, app: WeakEntity<WriterApp>, cx: &mut Context<Self>) -> Self {
        Self {
            part,
            app,
            focus: cx.focus_handle(),
        }
    }
}

impl BasePanel for AppPanel {
    fn panel_name(&self) -> &'static str {
        self.part.label()
    }

    fn dump(&self, _: &App) -> PanelState {
        PanelState::new(self.part.name())
    }
}

impl KitPanel for AppPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from(self.part.label()))
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().text_sm().child(self.part.label())
    }
}

impl EventEmitter<PanelEvent> for AppPanel {}

impl Focusable for AppPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let part = self.part;
        self.app
            .update(cx, |app, cx| app.ui(window, cx, part))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
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
    /// Dock 区域（首次渲染时构建）
    /// 所有 dock 面板：状态变化时逐个通知，预览才会即时重绘
    panels: Vec<Entity<AppPanel>>,
    dock_left: Option<Entity<DockArea>>,
    dock_right: Option<Entity<DockArea>>,
    font_page: usize,

    symbols: SymbolFonts,
    doc: RenderedDoc,
    page_index: usize,
    zoom: f32,
    /// 预览平移（屏幕像素）
    /// 数值行里可编辑输入框的状态（按行 id 缓存）
    num_inputs: std::collections::HashMap<&'static str, Entity<InputState>>,
    pan_x: f32,
    pan_y: f32,
    /// 右键拖动中
    panning: bool,
    last_mouse: (f32, f32),
    status: SharedString,

    machine: Machine,
    ports: Vec<PortInfo>,
    port_idx: usize,
    baud_idx: usize,
    /// 反转 X 轴（原点在右上角一侧的机型）
    /// 写字起点（页面左上角起算，mm）
    start_x: f64,
    start_y: f64,
    invert_x: bool,
    /// 反转 Y 轴（原点在左下角一侧的机型）
    invert_y: bool,
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
            panels: Vec::new(),
            pending_font: None,
            dock_left: None,
            dock_right: None,
            font_page: 0,
            symbols,
            doc: RenderedDoc::default(),
            page_index: 0,
            num_inputs: std::collections::HashMap::new(),
            pan_x: 0.0,
            pan_y: 0.0,
            panning: false,
            last_mouse: (0.0, 0.0),
            zoom: std::env::var("BIZHEN_DEMO_ZOOM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            status: "就绪".into(),
            machine: Machine::new(),
            ports: serial::list_ports(),
            port_idx: 0,
            baud_idx: 4, // 115200
            start_x: 0.0,
            start_y: 0.0,
            // 冒烟/演示：BIZHEN_DEMO_INVERT=x 或 y 或 xy
            invert_x: std::env::var("BIZHEN_DEMO_INVERT")
                .map(|v| v.contains('x'))
                .unwrap_or(false),
            invert_y: std::env::var("BIZHEN_DEMO_INVERT")
                .map(|v| v.contains('y'))
                .unwrap_or(false),
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

    /// 按界面上的轴设置构造 G-code 配置。
    fn gcode_config(&self) -> GCodeConfig {
        let page_h = self.effective_page().height;
        GCodeConfig {
            invert_x: self.invert_x,
            invert_y: self.invert_y,
            // 界面用「距纸面左上角」的毫米数，换算成页面坐标（y 向上）
            start_point: Some((self.start_x, page_h - self.start_y)),
            ..GCodeConfig::default()
        }
    }

    // ---------------- 数值输入行 ----------------

    /// 状态变化后通知主视图与所有面板：预览的 canvas 属于面板，
    /// 只 notify 主视图时它不会重绘（表现为缩放/拖动要等下一次事件才刷新）。
    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        for panel in &self.panels {
            panel.update(cx, |_, cx| cx.notify());
        }
    }

    fn num_value(&self, id: &str) -> f64 {
        match id {
            "pw" => self.page.width,
            "ph" => self.page.height,
            "mt" => self.page.margin_top,
            "mb" => self.page.margin_bottom,
            "ml" => self.page.margin_left,
            "mr" => self.page.margin_right,
            "sz" => self.size_mm,
            "wr" => self.wrap,
            "sx" => self.start_x,
            "sy" => self.start_y,
            "jog_step" => self.jog_step,
            "jog_feed" => self.jog_feed,
            _ => 0.0,
        }
    }

    fn set_num(&mut self, id: &str, v: f64) {
        if !v.is_finite() {
            return;
        }
        match id {
            "pw" => {
                self.page.width = v.clamp(40.0, 1000.0);
                self.preset = None;
            }
            "ph" => {
                self.page.height = v.clamp(40.0, 1000.0);
                self.preset = None;
            }
            "mt" => self.page.margin_top = v.clamp(0.0, 60.0),
            "mb" => self.page.margin_bottom = v.clamp(0.0, 60.0),
            "ml" => self.page.margin_left = v.clamp(0.0, 60.0),
            "mr" => self.page.margin_right = v.clamp(0.0, 60.0),
            "sz" => self.size_mm = v.clamp(1.5, 20.0),
            "wr" => self.wrap = v.clamp(20.0, 300.0),
            "sx" => self.start_x = v.clamp(0.0, self.page.width),
            "sy" => self.start_y = v.clamp(0.0, self.page.height),
            "jog_step" => self.jog_step = v.clamp(0.1, 100.0),
            "jog_feed" => self.jog_feed = v.clamp(10.0, 20000.0),
            _ => return,
        }
        self.relayout();
    }

    /// 取（或创建）某一行对应的输入框状态；未被聚焦时跟随外部数值刷新。
    fn num_input(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let want = format!("{:.1}", self.num_value(id));
        if let Some(state) = self.num_inputs.get(id) {
            let focused = state.read(cx).focus_handle(cx).is_focused(window);
            let text = state.read(cx).value().to_string();
            if !focused && text != want {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
            return state.clone();
        }
        let state = cx.new(|cx| InputState::new(window, cx).default_value(want));
        cx.subscribe_in(
            &state,
            window,
            move |app: &mut WriterApp, state, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    let text = state.read(cx).value().to_string();
                    if let Ok(v) = text.trim().parse::<f64>() {
                        app.set_num(id, v);
                    }
                    cx.notify();
                }
            },
        )
        .detach();
        self.num_inputs.insert(id, state.clone());
        state
    }

    /// 一行数值设置：标签 + [−] + 输入框 + [＋]
    fn num_row(
        &mut self,
        id: &'static str,
        label: &str,
        step: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.num_input(id, window, cx);
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
                    let v = this.num_value(id) - step;
                    this.set_num(id, v);
                    cx.notify();
                },
            ))
            .child(div().w(px(70.)).child(Input::new(&state)))
            .child(button(
                &format!("{id}_inc"),
                "＋",
                cx,
                move |this, _w, cx| {
                    let v = this.num_value(id) + step;
                    this.set_num(id, v);
                    cx.notify();
                },
            ))
            .into_any_element()
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
        let cfg = self.gcode_config();
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
        let cfg = self.gcode_config();
        let lines = gcode::generate(&self.doc.strokes(), &cfg, page.height, None).lines;
        self.machine.send(machine::Cmd::Send {
            lines,
            pen_up: cfg.pen_up_lines(),
            pen_down: cfg.pen_down_lines(None),
        });
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

impl WriterApp {
    fn ui(&mut self, window: &mut Window, cx: &mut Context<Self>, part: Part) -> AnyElement {
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
        let invert_x = self.invert_x;
        let invert_y = self.invert_y;
        let start_x = self.start_x;
        let start_y = self.start_y;
        let pan_x = self.pan_x;
        let pan_y = self.pan_y;
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
            .child(button("open", "打开…", cx, |this, _w, cx| {
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


        // Kit 的代码编辑器模式：行号 + Markdown 语法高亮 + 输入法
        let md_panel: AnyElement = div()
            .flex()
            .flex_col()
            .size_full()
            .p_2()
            .child(
                Editor::new(&self.editor)
                    .h_full()
                    .w_full()
                    .text_size(px(EDITOR_TEXT_SIZE)),
            )
            .into_any_element();
        let page_panel: AnyElement = {
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
                .child(self.num_row("pw", "纸宽 (mm)", 5.0, window, cx))
                .child(self.num_row("ph", "纸高 (mm)", 5.0, window, cx))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("页边距（四边独立）"),
                )
                .child(self.num_row("mt", "上 (mm)", 1.0, window, cx))
                .child(self.num_row("mb", "下 (mm)", 1.0, window, cx))
                .child(self.num_row("ml", "左 (mm)", 1.0, window, cx))
                .child(self.num_row("mr", "右 (mm)", 1.0, window, cx))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child("排版"),
                )
                .child(self.num_row("sz", "字号 (mm)", 0.5, window, cx))
                .child(self.num_row("wr", "折行宽 (mm)", 10.0, window, cx))
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
                ));
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_hidden()
                .child(panel)
                .into_any_element()
        };

        // ---------------- 中间预览 ----------------
        let page_w = page.width;
        let page_h = page.height;

        let preview = div()
            .id(SharedString::from("preview_area"))
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .p_3()
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _w, cx| {
                let dy: f32 = match ev.delta {
                    gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / 50.0,
                    gpui::ScrollDelta::Lines(p) => p.y,
                };
                let step = -dy * 0.08; // 滚轮上 = 放大
                this.zoom = (this.zoom * (1.0 + step)).clamp(0.2, 8.0);
                this.notify_all(cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    this.panning = true;
                    this.last_mouse = (ev.position.x.into(), ev.position.y.into());
                    this.notify_all(cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                if this.panning {
                    let x: f32 = ev.position.x.into();
                    let y: f32 = ev.position.y.into();
                    this.pan_x += x - this.last_mouse.0;
                    this.pan_y += y - this.last_mouse.1;
                    this.last_mouse = (x, y);
                    this.notify_all(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _ev: &MouseUpEvent, _w, cx| {
                    this.panning = false;
                    this.notify_all(cx);
                }),
            )
            .child(
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
                            let ox = f32::from(bounds.origin.x) + (avail_w - paper_w) / 2.0 + pan_x;
                            let oy = f32::from(bounds.origin.y) + (avail_h - paper_h) / 2.0 + pan_y;

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

                            // ---- 写字起点：十字 + X/Y 正方向箭头 ----
                            {
                                let sx = ox + start_x as f32 * scale;
                                let sy = oy + start_y as f32 * scale;
                                let dir_x = if invert_x { -1.0f32 } else { 1.0 };
                                let dir_y = if invert_y { -1.0f32 } else { 1.0 };
                                let arm = 26.0f32;
                                let head = 6.0f32;
                                let mut cross = PathBuilder::stroke(px(1.4));
                                // 十字
                                cross.move_to(point(px(sx - 6.0), px(sy)));
                                cross.line_to(point(px(sx + 6.0), px(sy)));
                                cross.move_to(point(px(sx), px(sy - 6.0)));
                                cross.line_to(point(px(sx), px(sy + 6.0)));
                                // X 轴箭头
                                let ex = sx + dir_x * arm;
                                cross.move_to(point(px(sx), px(sy)));
                                cross.line_to(point(px(ex), px(sy)));
                                cross.move_to(point(px(ex), px(sy)));
                                cross.line_to(point(px(ex - dir_x * head), px(sy - head)));
                                cross.move_to(point(px(ex), px(sy)));
                                cross.line_to(point(px(ex - dir_x * head), px(sy + head)));
                                // Y 轴箭头（机器 +Y 对应纸面向下）
                                let ey = sy + dir_y * arm;
                                cross.move_to(point(px(sx), px(sy)));
                                cross.line_to(point(px(sx), px(ey)));
                                cross.move_to(point(px(sx), px(ey)));
                                cross.line_to(point(px(sx - head), px(ey - dir_y * head)));
                                cross.move_to(point(px(sx), px(ey)));
                                cross.line_to(point(px(sx + head), px(ey - dir_y * head)));
                                if let Ok(path) = cross.build() {
                                    window.paint_path(path, rgb(0xd94f4f));
                                }
                            }
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
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(div().w(px(188.)).child(port_menu))
                    .child(button("refresh_ports", "刷新", cx, |this, _w, cx| {
                        this.refresh_ports();
                        cx.notify();
                    })),
            )
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
                .child(self.num_row("jog_step", "步长 (mm)", 1.0, window, cx))
                .child(self.num_row("jog_feed", "速度 (mm/min)", 250.0, window, cx));

        // 方向键按十字排：上 / 左·右 / 下
        let jog_button = |id: &str, label: &str, dx: f64, dy: f64| {
            div()
                .id(SharedString::from(id.to_string()))
                .w(px(52.))
                .py_1()
                .rounded_md()
                .bg(rgb(0x3a3a3a))
                .text_xs()
                .text_color(rgb(0xe0e0e0))
                .flex()
                .items_center()
                .justify_center()
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
                    .justify_center()
                    .child(jog_button("jog_yp", "Y+", 0.0, 1.0)),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap_1()
                    .child(jog_button("jog_xm", "X−", -1.0, 0.0))
                    .child(jog_button("jog_xp", "X+", 1.0, 0.0)),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .child(jog_button("jog_ym", "Y−", 0.0, -1.0)),
            )
            // 反向轴
            .child(
                div()
                    .flex()
                    .gap_1()
                    .mt_1()
                    .child(button(
                        "inv_x",
                        if invert_x { "X 反向" } else { "X 正向" },
                        cx,
                        |this, _w, cx| {
                            this.invert_x = !this.invert_x;
                            cx.notify();
                        },
                    ))
                    .child(button(
                        "inv_y",
                        if invert_y { "Y 反向" } else { "Y 正向" },
                        cx,
                        |this, _w, cx| {
                            this.invert_y = !this.invert_y;
                            cx.notify();
                        },
                    )),
            )
            .child(div().h(px(1.)).bg(rgb(0x333333)).mt_2())
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child("写字起点（距纸面左上角）"),
            )
            .child(self.num_row("sx", "起点 X (mm)", 5.0, window, cx))
            .child(self.num_row("sy", "起点 Y (mm)", 5.0, window, cx))
            .child(div().h(px(1.)).bg(rgb(0x333333)).mt_2())
            .child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child("作业"),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(button(
                        "pause",
                        if machine_state.paused {
                            "继续"
                        } else {
                            "暂停"
                        },
                        cx,
                        |this, _w, cx| {
                            if this.machine.snapshot().paused {
                                this.machine.send(machine::Cmd::Resume);
                            } else {
                                this.machine.send(machine::Cmd::Pause);
                            }
                            this.poke();
                            cx.notify();
                        },
                    ))
                    .child(button("abort", "终止", cx, |this, _w, cx| {
                        this.machine.send(machine::Cmd::Abort);
                        this.poke();
                        cx.notify();
                    })),
            )
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
                .child(div().w(px(400.)).h_full().child(self.dock_left_entity(window, cx)))
                .child(preview)
                .child(div().w(px(300.)).h_full().child(self.dock_right_entity(window, cx))),
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


        match part {
            Part::All => root.into_any_element(),
            Part::Markdown => md_panel,
            Part::Page => page_panel,
            Part::Preview => div().into_any_element(),
            Part::Machine => div()
                .id(SharedString::from("machine_scroll"))
                .size_full()
                .overflow_y_scroll()
                .child(machine_panel)
                .into_any_element(),
        }
    }

    /// 首次渲染时构建 Dock：左＝Markdown/页面，中＝预览，右＝机器控制
    fn dock_left_entity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(a) = &self.dock_left {
            return a.clone().into_any_element();
        }
        let weak = cx.entity().downgrade();
        let (area, _skin) = DockSkin::dock_area("bizhen-dock-left", None, window, cx);
        let md = cx.new(|cx| AppPanel::new(Part::Markdown, weak.clone(), cx));
        let page = cx.new(|cx| AppPanel::new(Part::Page, weak.clone(), cx));
        self.panels.push(md.clone());
        self.panels.push(page.clone());
        area.update(cx, |area, cx| {
            area.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel(md).panel(page),
                window,
                cx,
            );
            area.set_dock_size(DockPlacement::Left, px(400.), window, cx);
        });
        self.dock_left = Some(area.clone());
        area.into_any_element()
    }

    fn dock_right_entity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(a) = &self.dock_right {
            return a.clone().into_any_element();
        }
        let weak = cx.entity().downgrade();
        let (area, _skin) = DockSkin::dock_area("bizhen-dock-right", None, window, cx);
        let mac = cx.new(|cx| AppPanel::new(Part::Machine, weak.clone(), cx));
        self.panels.push(mac.clone());
        area.update(cx, |area, cx| {
            area.set_dock(
                DockPlacement::Right,
                DockLayout::tabs().panel(mac),
                window,
                cx,
            );
            area.set_dock_size(DockPlacement::Right, px(300.), window, cx);
        });
        self.dock_right = Some(area.clone());
        area.into_any_element()
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ui(window, cx, Part::All)
    }
}

impl Render for WriterApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ui(window, cx, Part::All)
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
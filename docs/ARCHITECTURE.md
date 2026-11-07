# 架构说明

> 读者：要修改本项目的维护者。目标是在 10 分钟内建立「数据从 Markdown 到纸上」的完整心智模型。

## 1. 分层

```
bizhen-app  ── 界面（gpui / GPUI Kit）+ 串口线程
     │           main.rs：状态、布局、预览绘制、按钮回调
     │           machine.rs：串口 worker（连接、点动、发作业、暂停/终止）
     ▼
bizhen-cli  ── 命令行封装（复用 core，不依赖 gpui）
     ▼
bizhen-core ── 纯逻辑，无 UI、无平台假设
```

`bizhen-core` 的模块：

| 模块 | 职责 |
|---|---|
| `geom.rs` | `Vec2 = (f64, f64)`、`BBox`、RDP 简化等几何工具 |
| `stroke.rs` | `Stroke`（点序列 + 抬落笔 + 角色 Role）、笔画集合操作 |
| `font/` | 字体模型与解析：`model.rs`（统一数据结构）、`jhf.rs`（Hershey）、`stroke_json.rs`、`registry.rs`（内置 + 用户字库、懒加载） |
| `markdown.rs` | Markdown → 块（`Block`）→ 排版成 `Page` / `RenderedDoc` |
| `math.rs` | 行内公式子集解析与排版（自研，不依赖 matplotlib） |
| `layout.rs` | 排版细节：折行、标点禁则、行高、字号阶梯 |
| `render.rs` | 块 → 笔画（把排版结果画成 `Stroke`） |
| `gcode.rs` | 页面坐标 → 机器坐标 → G-code 文本 |
| `serial.rs` | GRBL 串口：握手、状态查询、`send_program_controlled`（可暂停/中止） |
| `sim.rs` | 手写抖动参数与施加 |
| `symbols.rs` | 符号/标点兜底字体 |

## 2. 数据流

```
Markdown 文本
   │  markdown::render(style, page)
   ▼
RenderedDoc { pages: Vec<Page { strokes: Vec<Stroke> }>, missing: Vec<char>, tables: ... }
   │  用户在第 N 页点「发送作业」或「导出」
   ▼
doc.pages[N].strokes
   │  gcode::generate(&strokes, &cfg, page_h, start_point)
   ▼
Vec<String>（G-code 行）
   │  machine::Cmd::Send → serial::send_program_controlled
   ▼
串口 → GRBL
```

关键点：

* **坐标**：排版用 mm、**Y 轴向上**、页面原点在**左下角**。G-code 生成时才翻成机器坐标。
* **分页**：`render` 一次算完整份文档的所有页，预览按 `page_index` 取其中一页；
  发送与导出都只取当前页（早期版本误发整份文档，见 CHANGELOG 0.4.0）。
* **字体单位**：字形坐标在字体单位下，`scale = 字号(mm) / units_per_em` 后在排版时缩放。

## 3. 界面结构（bizhen-app）

`WriterApp` 是唯一的状态持有者：

```rust
struct WriterApp {
    source: String,                 // Markdown 源
    editor: Entity<EditorState>,    // Kit 编辑器状态
    doc: RenderedDoc,               // 排版结果（缓存）
    registry: FontRegistry,         // 字库
    machine: Machine,               // 串口句柄（发命令 + 读快照）
    page_index: usize,
    zoom: f32, pan_x: f32, pan_y: f32,
    page_visible: bool, right_visible: bool,
    ...
}
```

`ui()` 每帧重建整棵元素树：

```
root(flex_col)
├── header     顶栏按钮
├── body(flex_row)
│   ├── page_col   280px  （when page_visible）
│   ├── edit_col   420px
│   ├── preview    flex_1
│   └── machine_col 300px （when right_visible）
└── footer     状态栏
```

布局注意：固定列必须 `.flex_none()`，否则窗口变窄时会被压缩；预览外面要包一层
`.flex()` 容器，否则 canvas 拿不到有效 bounds，表现为「预览空白」。

## 4. 串口线程模型

`machine.rs` 起一个 worker 线程，主线程通过 `mpsc` 发 `Cmd`，worker 通过
`Arc<Mutex<State>>` 暴露快照：

```
UI 线程 ──Cmd──▶ worker ──写串口──▶ GRBL
   ▲               │
   └─State 快照◀───┘
```

`Cmd::Send` 分支是**阻塞**调用（`send_program_controlled` 直到作业结束才返回），
期间 worker 不再 `recv`。因此作业进行中的控制命令必须由 `drain_control()` 在
发送循环里主动 `try_recv` 消费——这也是 0.4.0 修掉「暂停后无法继续」的关键。

## 5. 扩展点

| 想做什么 | 改哪里 |
|---|---|
| 新增 Markdown 块类型 | `markdown.rs` 的 `Block` 枚举 + 解析；`render.rs` 画出笔画 |
| 新增公式符号 | `math.rs` 的符号表与排版分支 |
| 支持新的字体格式 | `font/` 下加解析器，产出 `FontFamily`，在 `registry.rs` 注册 |
| 改 G-code 指令形状 | `gcode.rs` 的 `GCodeConfig` 与生成函数 |
| 改界面布局 | `bizhen-app/src/main.rs` 的 `ui()`，注意上面的布局注意点 |
| 加串口命令 | `machine.rs` 的 `Cmd` 枚举 + worker 分支 |

## 6. 测试策略

* `bizhen-core` 覆盖：几何、折行、公式解析、表格、分页、G-code 坐标、串口协议解析。
* 当前共 35 项单元测试（`cargo test`）。
* 涉及 UI 与真机串口的部分没有自动化测试，靠 `examples/*.md` + 实际打印验证。
* `tools/render_png.py` 可把 CLI 导出的 JSON 笔画渲染成 PNG，用于视觉回归。

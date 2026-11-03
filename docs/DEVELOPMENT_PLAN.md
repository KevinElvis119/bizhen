# 开发计划

> 目标：用 Rust 重写 WriterStudio，**只保留 Markdown 排版 → G-code** 这条主线，
> 字体与排版度量对齐原版，并新增**分页**。UI 用 gpui。

---

## 1. 范围

### 保留

| 能力 | 说明 |
|---|---|
| Markdown 块级 | 标题（1–6 级）、段落、无序/有序列表、引用、代码块、分割线 |
| Markdown 表格 | 自动列宽、对齐、单元格内折行、表格宽度约束；**合并单元格**（`<<` / `^^`）作为加分项 |
| 行内公式 | `$...$`，与正文同一支笔 |
| 独立公式块 | `$$...$$`，居中 |
| 图片 | **位置留空**：按给定尺寸预留空白块，不画内容 |
| 自动折行 | 按内容宽度折行，遵守中文标点禁则（行首禁排/行尾禁排） |
| 分页 | 按纸张尺寸与页边距自动分页，块/行不越界 |
| 字体 | 与 WriterStudio 同一套：Hershey `.jhf` + `STRK-Kaiti.json`，同一条回退链 |
| G-code | 抬落笔方式、进给、坐标映射、阅读顺序笔序、可选抽稀 |

### 去掉

笔画编辑、手写扰动、TikZ、SVG 导入、完整 LaTeX 文档编译、项目文件/崩溃恢复、
串口在线控制、MCP 服务、参考图层、竖排（先不做，见待定问题）。

---

## 2. 架构

```
Markdown 文本
   │ pulldown-cmark（CommonMark + GFM 表格）
   ▼
块级 AST（Heading/Paragraph/List/Quote/Code/Hr/Table/Image/Math）
   │
   ├── 行内切分：$...$ 公式 vs 普通文字
   ▼
排版引擎 layout
   │ 逐字挑字体（回退链）→ 步距/字形笔画
   │ 断行（CJK 禁则）→ 行宽
   ▼
块渲染器 markdown：块 → 若干「行原子」(strokes + 宽高)
   │
   ▼
分页流 paginate：原子 → 页（超出下边距换页）
   │
   ├── 预览：GPUI 画折线
   ▼
G-code 生成 gcode：页内笔画 → 机器坐标 → 抬落笔 + 直线插补
```

坐标约定（全项目统一，不可改）：
* 单位 mm，**Y 轴向上**；页面原点在**左下角**；
* 排版内部可以用「光标从页顶往下」的写法，但落盘/导出前一律是左下原点。

---

## 3. 模块映射（Python → Rust）

| 原文件 | 职责 | Rust 模块 | 状态 |
|---|---|---|---|
| `core/geometry.py` | BBox / 仿射 / RDP | `geom.rs` | ✅ |
| `core/strokes.py` | 笔画模型 | `stroke.rs` | ✅ |
| `fonts/model.py` | 字形/字体/度量 | `font/model.rs` | ✅ |
| `fonts/hershey.py` | `.jhf` 解析 | `font/jhf.rs` | ✅ |
| `fonts/stroke_json.py` | `STRK-*.json` 解析 | `font/stroke_json.rs` | M1 |
| `fonts/manager.py` | 扫描/懒加载/缓存 | `font/registry.rs` | ✅（jhf）／M1（json） |
| `fonts/layout.py` | 回退链、折行、行高 | `layout.rs` | M2 |
| `content/markdown.py` | Markdown 渲染器 | `markdown.rs` | M2 |
| `content/equation.py` | mathtext 公式 | `math/` | M3 |
| `machine/config.py` | G-code 配置/坐标映射 | `gcode/config.rs` | M4 |
| `machine/gcode_gen.py` | 笔画 → G-code | `gcode/gen.rs` | M4 |
| `machine/path_optimizer.py` | 笔序（阅读顺序） | `gcode/order.rs` | M4 |
| `page_presets.py` | 纸张预设 | `page.rs` | M3 |
| `ui/*` | PySide6 界面 | `ws-app`（gpui） | M5 |

---

## 4. 里程碑

### M0 环境与骨架 ✅（今天）

- [x] crate 布局、workspace、`default-members`
- [x] 工程内 cargo 镜像配置（不改全局）
- [x] 字库资源入库（32 款 `.jhf` + `STRK-Kaiti.json`）
- [x] BBox / Stroke / RDP
- [x] `.jhf` 解析 + 度量（**与 Python 逐位一致**：ascent 25 / descent −7 / advance 17.979166666666668 / A 高 21）
- [x] `ws-cli fonts|info`
- [x] `cargo test` 5 项通过

**验收**：`cargo test` 全绿；`ws-cli info futural` 的数值与 Python 版一致。

---

### M1 字体层补完

- [ ] `font/stroke_json.rs`：解析 8.4 MB `STRK-Kaiti.json`，**懒加载**（启动只登记路径）
      - 内存目标：常驻 < 40 MB（Python 版压到 ~5 MB，Rust 用扁平 `Vec<f32>` + 索引）
      - 度量算法与 `model.rs` 的 `recompute_metrics` 一致（95%/5% 分位）
- [ ] `FontRegistry`：按名字取字体、按 kind 过滤、默认回退链解析
      （默认链 = 第一款 stroke-json + `futural`/`futuram`/`scripts` 中存在的第一款）
- [ ] 缺字上报：`missing: Vec<char>`（去重、保持出现顺序）

**验收**：`ws-cli info STRK-Kaiti` 字形数 = 20976；载入耗时 < 300 ms；未使用时不占内存。

---

### M1.5 GPUI 冒烟（提前排风险，半天）

- [ ] `ws-app` 加 `gpui = "0.2"`，跑通一个空白窗口
- [ ] 验证：Windows + MSVC 工具链、字体渲染、`cargo run -p ws-app` 能起窗口
- [ ] 若 gpui 在 Windows 上不可用，**立即上报并改方案**（备选：egui / wgpu 自绘 / 先只做 CLI+导出）

**验收**：能起一个 800×600 窗口，画一条折线。

---

### M2 文本排版 + Markdown 块级

- [ ] `layout.rs`：`TextStyle`（size / line_spacing / char_spacing / align）
      - 逐字挑字体：先所有字体找候选 → 链上靠前者优先
      - 步距：`advance * scale + char_spacing`；空格按 `word_space%`
      - 行高：`(ascent - descent) * scale * line_spacing`（以链首字体为准）
      - 基线：首行 `baseline = cursor_y - size`，之后 `y = baseline - (line_height - size)`
- [ ] 断行：`_is_wide` + 标点禁则 + 西文按词 + 超长 token 硬切
- [ ] `markdown.rs`：标题（`heading_scale` 0.6em 后留白）/段落（0.8em）/列表（缩进 6mm，0.4em）/
      引用（左侧竖线，缩进 6mm）/代码块（0.92 字号）/分割线
- [ ] 行内标记剥离：`**粗体**`、`` `代码` ``、`[链接](url)`、`![alt](src)` → 只取文字

**验收**：同一份 Markdown + 同一字体，Rust 与 Python 导出的笔画**总长度相对误差 < 1e-9**、
笔画数相等、包围盒相等（见 §6 黄金对比）。

---

### M3 表格 / 公式 / 图片 / 分页

- [ ] 表格：列宽（`_fit_widths`：只收缩不拉宽，显式宽度时精确贴合，最小列宽 = 最宽单字）、
      单元格内折行、行高、对齐（left/center/right）、表头分隔线
- [ ] 表格贴底线书写：文字基线跟着行界线段走（原版手绘感的来源，可选）
- [ ] 公式引擎（替代 matplotlib mathtext，**自研**）：
      - 支持子集：上下标、`\frac`、`\sqrt`、`\sum`/`\prod`/`\int`（含上下限）、
        `\left(...\right)`、希腊字母、常用关系/运算符、`\text{}`
      - 输出与 mathtext 同构：字形位置 `(char, size, ox, oy)` + 结构矩形（分数线/根号顶线）
      - 字形绘制优先级：字体链 → 内置 Hershey 符号（`mathupp`/`greek`）→ 几何画线（`symbols.rs`）
      - 兜底：解析失败时原样输出 `$...$` 文本，不让公式消失
- [ ] 图片块：占位矩形（默认宽 = 内容宽、高 = 宽 × 2/3），不画内容
- [ ] `paginate.rs`：流式分页（行级换页、表格不跨页、页边距），页号可选
- [ ] `page.rs`：A0–A6 / B0–B5 / Letter / Legal / Tabloid / 照片 / 明信片 / 正方形

**验收**：一张 A4 页面上，内容不越过页边距；表格、公式跨页时整体移到下一页；
公式样本页与原版渲染对比（视觉）。

---

### M4 G-code + CLI 端到端

- [ ] `gcode/config.rs`：抬落笔 6 种方式（z / servo-angle / laser / custom / m3m5 / m7m9）、
      进给、抬落笔 Z、延迟、自定义起止代码、坐标映射（原点角落 / 对调 / 反转）、小数位
- [ ] `gcode/gen.rs`：抬笔 → 移到起点 → 落笔 → 直线插补 → 收笔；
      入笔斜落 / 收笔斜抬（`entry_taper_mm` / `stroke_taper_mm`）；抽稀
- [ ] `gcode/order.rs`：阅读顺序（按分组、组内保持顺序、组间先上后下先左后右）
- [ ] `ws-cli md <file.md> -o out.gcode`：Markdown → 分页 → G-code，打印统计
      （绘制长度、空程、笔画数、估算时间）

**验收**：`ws-cli md sample.md` 生成的 G-code 与原版对同一内容的输出**逐行一致**
（或差异可解释：分页导致的对象顺序不同）。

---

### M5 GPUI 界面

- [ ] 布局：左侧 Markdown 编辑区，右侧分页预览，顶部参数条，底部状态栏
- [ ] 渲染：把折线批量转成 GPUI 的 path/quad，分页缓存，缩放/平移
- [ ] 参数绑定：纸张、页边距、字号、行距、字体链、表格宽度 → 立即重排（防抖）
- [ ] 导出：G-code 文件、PNG 预览（可选）
- [ ] 中文输入法支持（gpui 在 Windows 上的 IME 是大坑，需专门测）

**验收**：A4 一页《静夜思》+ 标题 + 表格 + 公式，编辑即时预览，导出可打印 G-code。

---

### M6 收尾

- [ ] `cargo fmt` / `cargo clippy -- -D warnings` 全绿
- [ ] 发布打包（`cargo build --release` + 字库目录）
- [ ] 文档与示例

---

## 5. 完整参数默认值

见 [`CONTEXT.md`](CONTEXT.md) §5「参数默认值」——所有默认值必须与原版一致，
否则排版结果对不上。

---

## 6. 测试策略

### 6.1 黄金对比（最重要）

Python 侧写一个 headless 导出脚本（不依赖 Qt）：

```python
# tools/export_golden.py（待写）
from writerstudio.fonts.manager import FontManager
from writerstudio.content.markdown import render_markdown, MarkdownStyle
# md + 字体 → JSON: {strokes: [[[x,y],...]], width, height}
```

Rust 侧 `tests/golden.rs` 读同一份 JSON，逐笔比较：
* 笔画数相等；
* 每条笔画点数相等；
* 坐标最大绝对误差 < 1e-9（同一算法应当逐位一致）；
* 失败时打印第一处差异（笔画序号、点序号、坐标）。

首批样本：纯段落折行、标题层级、列表、引用、表格（含对齐）、行内公式。

### 6.2 单元测试

* 断行：CJK 可在字间断、标点不落行首、西文不断词、超长 URL 硬切；
* 列宽：自然宽 < 上限时不拉宽；显式宽度时精确贴合；列数过多时按最小宽返回；
* 分页：一行不越下边距；表格整体换页；空页不产生；
* G-code：抬落笔序列、坐标映射（四种原点）、斜抬斜落的 Z 词、抽稀保端点。

### 6.3 端到端

`cargo test --test e2e`：Markdown 文件 → G-code 文本快照（`insta` 或手写快照）。

---

## 7. 开发流程

```powershell
. .\scripts\dev.ps1          # 每个新终端先跑
cargo test                   # 改完就跑
cargo clippy --all-targets -- -D warnings
cargo fmt
```

* 每个里程碑结束打一个 git tag；
* 提交信息用中文，一句话说清「改了什么 + 为什么」；
* 新增排版规则必须同时补一条黄金对比用例。

---

## 8. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| gpui 在 Windows 上不成熟（IME、输入框、字体） | UI 做不出来 | **M1.5 提前冒烟**；备选 egui / wgpu 自绘 / 先 CLI |
| 公式引擎工作量被低估 | M3 拖延 | 先做子集；解析失败原样输出文本；分两批符号 |
| `STRK-Kaiti.json` 8.4 MB 解析慢/占内存 | 启动卡 | 懒加载 + 扁平数组 + 首次访问才建字形 |
| markdown-it 与 pulldown-cmark 解析差异 | 排版对不上 | 黄金样本覆盖；必要时自己补表格/块解析 |
| 断行禁则的细节（中西混排、标点） | 视觉差 | 直接照抄原版 `_is_wide` / `_can_break_before` 表 |
| 表格抖动（手绘感）参数被误删 | 结果不同 | 参数默认保留，`table_end_jitter=0.5`、`overshoot=0.6` |

---

## 9. 下一步（接手第一件事）

1. 跑 `. .\scripts\dev.ps1`，`cargo test` 确认环境；
2. 做 **M1.5 GPUI 冒烟**（最大不确定性，先排除）；
3. 再做 M1 的 `stroke_json.rs`。

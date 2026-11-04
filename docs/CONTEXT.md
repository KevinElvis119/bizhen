# 交接上下文（Context Handoff）

> 项目名：**笔阵（Bizhen）**——取自卫夫人《笔阵图》。「笔」是书写，「阵」是排布：
> 把 Markdown 排成阵，再交给写字机一笔一笔写出来。
>
> 本文件是「接手时只看这一份就能继续」的备忘。

---

## 1. 参考源码在哪

| 项 | 值 |
|---|---|
| 原项目 | <https://github.com/ZhengXieGang/WriterStudio> |
| 本机完整源码 | `C:\Users\zencoding\WriterStudio`（git HEAD `e0ee07b`，origin 指向上面） |
| 工作区里的 `E:\WritingRobot\WriterStudio` | **坏的 git 目录**：只有 `.git\hooks`、`info`、`objects`、`refs`，缺 `HEAD`/`config`，`git status` 报 "not a git repository"。不影响开发，重写工程在 `E:\WritingRobot\bizhen` |
| 原版技术栈 | Python 3 + PySide6 + markdown-it + matplotlib(mathtext) + numpy/fontTools |
| 原版规模 | `content/markdown.py` 1029 行、`content/equation.py` 864 行、`fonts/layout.py` 707 行 |

读源码的入口顺序：`content/markdown.py` → `fonts/layout.py` → `machine/gcode_gen.py` → `machine/config.py`。

---

## 2. 本机环境事实

| 项 | 值 |
|---|---|
| rustc / cargo | 1.99.1（stable-x86_64-pc-windows-msvc） |
| `CARGO_HOME` | `F:\RustLang\Cargo` |
| `RUSTUP_HOME` | `F:\RustLang\Rustup` |
| 工具链不在 PATH | 变量只写在**用户级注册表**，而 DSH 会话进程早于它们启动 → **每个新终端先 `. .\scripts\dev.ps1`** |
| crates 镜像 | `.cargo/config.toml` 用 `rsproxy.cn`（直连 crates.io 返回 403） |
| 代理 | 系统代理 `127.0.0.1:7890` 开着；`dev.ps1` 会按需补 `HTTP(S)_PROXY` |
| Python（做黄金对比用） | `F:\Python312\python.exe`；不需要装 WriterStudio 依赖即可 import `writerstudio.fonts.*` / `content.markdown`（后者需要 markdown-it） |
| 磁盘 | C: 19.8 GB 空闲、F: 45.6 GB、E: 31.2 GB（目标 `target/` 在 E:，gpui 全量编译约 3–5 GB） |

验证过的命令：

```powershell
. .\scripts\dev.ps1
cd E:\WritingRobot\bizhen
cargo test          # 5 passed
cargo run -p bizhen-cli -- info futural
```

---

## 3. 字库资源

`assets/fonts/` 从原仓库 `writerstudio/fonts/builtin/` 原样拷出：

| 目录 | 内容 | 大小 |
|---|---|---|
| `hershey/*.jhf` | 32 款 Hershey 单线字体（rowman\*、times\*、script\*、gothic\*、greek\*、mathupp/mathlow、symbolic…） | 0.21 MB |
| `stroke-json/STRK-Kaiti.json` | 中文单线楷体，**20976 字形** | 8.4 MB |

**默认字体链**（`content/builder.py:_resolve`）：第一款 `stroke-json` 字体（即 `STRK-Kaiti`）
＋ `futural` / `futuram` / `scripts` 中第一个存在的（即 `futural`）。
即中文走楷体单线、西文走 Hershey 单线——**重写版必须保持这个顺序**，否则字宽全变。

符号兜底：`mathupp`（`∞ ° ≠ ≡ ∈ → ← ↓ ÷ ∥ ⊥ ·`）、`greek`（希腊字母按拉丁转写挂键），
以及 `equation.py:184` 的几何画线符号表。

---

## 4. 关键算法索引（照抄这些，别自己发明）

| 要做的事 | 原版位置 | 要点 |
|---|---|---|
| 字体度量 | `fonts/model.py:106-137` | ascent=上缘 **95% 分位**、descent=下缘 **5% 分位**、default_advance=平均步距；行高 `(asc-desc)*scale*spacing` |
| 挑字体 | `fonts/layout.py:179-203` | 先在所有字体里找能画的候选，未开随机就取**链上靠前**者；都没有→符号兜底 |
| 测宽（热路径） | `fonts/layout.py:375-417` | 只累加步距，不构造笔画；空格按 `word_space%` |
| 横向排版 | `fonts/layout.py:420-556` | 两遍：先沿一条基线累积候选字符，再按 `frame_width` 分组、逐行落基线 |
| 断行规则 | `fonts/layout.py:280-339` | `_is_wide`（CJK 区间）、`_CLOSING_NO_HEAD`（闭合标点不落行首）、`_OPENING_NO_TAIL`（开口标点不悬行尾）、超宽 token 硬切 |
| 基线推进 | `content/markdown.py:_emit_line` | `baseline = y - size`；下一行 `y = baseline - (line_height - size)` |
| 标题 | `markdown.py:449` | `heading_scale=(1.9,1.6,1.4,1.25,1.15,1.1)`，之后 `y -= size*0.6` |
| 段落 | `markdown.py:457` | 折行后 `y -= size*0.8` |
| 列表 | `markdown.py:509` | 缩进 6 mm，marker 单独画，`y -= size*0.4` |
| 引用 | `markdown.py:541` | 缩进 6 mm，左竖线 `x=indent*0.4`，`role=structure` |
| 代码块 | `markdown.py:556` | 字号 ×0.92，逐行，`y -= size*0.6` |
| 表格列宽 | `markdown.py:99-131` | 自然宽只**收缩**不拉宽；显式 `table_width` 时精确贴合；列不窄于「最宽单字 + 2×padding」 |
| 表格渲染 | `markdown.py:571-887` | 列宽→列界→行高→格线→文字贴**底线**写；`CELL_PAD_MM=1.5`；单元格基线 `ys[r1+1] + size*0.14 + (行数-1)*lh` |
| 行内公式切分 | `markdown.py:342-362` | 按 `$...$` 切；公式整块不可拆 |
| 公式符号兜底 | `equation.py:38-89` | Unicode → (内置字体, 键位) 表 + 几何画线集合 |
| 公式排版 | `equation.py:722-859` | mathtext 只负责**排版**：给出每个字形的 `(字体, 字号, 码点, 索引, ox, oy)` 和结构矩形；字形本身用字体链/符号重画，结构线取中线 |
| G-code 主流程 | `machine/gcode_gen.py:77-276` | header → start → 抬笔 → `G0` 起点 → 每笔〔抬笔/`G0`/落笔/`G1`×n〕→ end → footer |
| 斜落斜抬 | `gcode_gen.py:166-194, 226-249` | 仅 Z 模式；坡长按弧长，ΔZ 受 `pen_z_feed/draw_feed` 斜率上限约束 |
| 坐标映射 | `machine/config.py:89-131` | `page_to_machine = (x, page_h - y)`，再做对调/反转，最后加 `start_offset` |
| 笔序 | `machine/path_optimizer.py:218-258` | 阅读顺序：按分组，组内保持顺序，组间按 `(-round(y_top/2)*2, x_left)` 排序 |
| 纸张预设 | `page_presets.py:76-96` | A0–A6 / B0–B5 / Letter / Legal / Tabloid / 照片 4×6 / 明信片 / 正方形，margin 默认 10（Letter 系列 12.7） |

---

## 5. 参数默认值（必须一致）

### MarkdownStyle（`markdown.py:33-59`）

| 参数 | 默认 | 说明 |
|---|---|---|
| `size` | 4.0 mm | 正文字号 |
| `line_spacing` | 1.5 | × 行高 |
| `char_spacing` | 0.0 | 额外字距 |
| `wrap_width` | 160.0 mm | 折行宽度（重写版 = 内容区宽度） |
| `table_width` | 0.0 | 0 = 按内容自适应 |
| `table_end_jitter` | 0.5 | 表格线端随机 σ(mm) |
| `table_overshoot` | 0.6 | 线端出头上限(mm) |
| `table_row_jitter` / `table_col_jitter` | 0.0 | 行界/列界随机 |
| `table_seed` | 20260915 | 同种子重排不跳变 |
| `heading_scale` | (1.9, 1.6, 1.4, 1.25, 1.15, 1.1) | H1–H6 |
| `list_indent` / `quote_indent` | 6.0 mm | |
| `math_size_scale` | 1.0 | |
| `CELL_PAD_MM` | 1.5 | 单元格内边距 |

### TextStyle（`fonts/layout.py:51-68`）

`size 10.0`、`line_spacing 1.4`、`char_spacing 0.0`、`align left`、`word_space 100%`、
`frame_width 0`（0 = 不自动折行）。**Markdown 里用的是 1.5，不是 1.4**。

### GCodeConfig（`machine/config.py:134-212`）

| 参数 | 默认 |
|---|---|
| `pen_mode` | `z`（另有 servo-angle / laser / custom / m3m5 / m7m9） |
| `pen_up_z` / `pen_down_z` | 0.0 / 6.0 |
| `pen_down_s` / `pen_up_s` / `pen_down_angle` | 1000 / 0 / 0 |
| `laser_power` / `laser_preview_power` | 1000 / 50 |
| `draw_feed` / `travel_feed` / `pen_z_feed` | 4400 / 9000 / 12000 (mm/min) |
| `pen_on_delay` / `pen_off_delay` | 0 / 0 |
| `header` / `footer` | `G21 G90 G94 G17` / `M5` |
| `decimals` / `feed_decimals` | 3 / 0 |
| `stroke_taper_mm` / `entry_taper_mm` | 1.5 / 0.5 |
| `order_mode` | `reading` |
| `simplify_mm` | 0.0（不抽稀） |
| `return_to_start` | true |
| `origin_corner` | `none` |
| `swap_xy` / `invert_x` / `invert_y` | false |

### 页面

A4 = 210 × 297 mm，margin 10 mm；Letter = 215.9 × 279.4，margin 12.7 mm。

---

## 6. 一致性验证

### 6.1 字体度量（逐位一致）

用 Python 原版解析 `futural.jhf`，与 Rust 版输出对比：

| 指标 | Python | Rust | 结果 |
|---|---|---|---|
| ascent | 25.0 | 25.0 | ✅ |
| descent | -7.0 | -7.0 | ✅ |
| default_advance | 17.979166666666668 | 17.979166666666668 | ✅ 逐位 |
| 字形数 | 96 | 96 | ✅ |
| 字母 A 墨迹高 | 21.0 | 21.0 | ✅ |
| 4mm/1.5 行高 | 6.0 | 6.0 | ✅ |

### 6.2 排版结果（同一 Markdown + 同一字体链）

`cargo run -p bizhen-cli -- md tests/*.md` 与 Python `render_markdown` 对比
（字体链 = `STRK-Kaiti` + `futural`，正文字号 4mm，行距 1.5，折行宽 160mm）：

| 样例 | Python（笔画/包围盒） | Rust | 结果 |
|---|---|---|---|
| 单段中文 | 91 / 41.56×4.04 | 91 / 41.6×4.0 | ✅ |
| 标题 H1 | 29 / 22.42×7.37 | 29 / 22.4×7.4 | ✅ |
| 表格（3×3） | 118 / 34.61×21.90 | 118 / 34.0×22.6 | ✅（抖动随机数不同） |
| 引用块 | 70 / 35.56×5.52 | 70 / 35.6×5.5 | ✅ |
| 无序列表 | 79 / 19.52×11.12 | 79 / 19.5×11.1 | ✅（见下） |
| 综合（标题+段落+列表+表格+引用） | 500 / 42.89×99.77 | 502 / 86.2×99.8 | 高度一致；宽度差异见下 |

已知的两处**有意偏差**：

1. **列表圆点**：原版字体链画不出 `•` 时不画（版面留空），Rust 版加了「几何圆点」
   兜底，因此多 2 条笔画、列表略宽。
2. **软换行**：原版 markdown-it 的段落内 `\n` 会保留，`_append_text` 把多行画出
   却只推进一行行高（会叠字）；Rust 版按 Markdown 语义把软换行并成空格再折行，
   所以同一段落里手动换行的文档，宽度与原版不同（Rust 的结果是正确的那个）。

### 6.3 测试

`cargo test` → 27 passed（字体、排版、折行、公式、表格、图片占位、分页、G-code）。
`cargo clippy --all-targets -- -D warnings` 全绿。

---

## 7. 已拍板的范围（2026-11-04 用户确认）

1. **公式范围**：常用子集 —— 上下标、`\frac`、`\sqrt`、`\sum`/`\prod`/`\int`（含上下限）、
   `\left(...\right)`、希腊字母、常用关系/运算符、`\text{}`。不做完整 TeX。
2. **图片占位**：支持 `![alt](src){width=80,height=50}`（单位 mm）；
   未给尺寸时默认 **内容宽 × 2/3**；只预留空白，不画内容。
3. **表格合并单元格**：**要**，保留 `<<`（跨列）与 `^^`（跨行）。
4. **表格手绘抖动**：**保留**，默认 `table_end_jitter=0.5`、`table_overshoot=0.6`，可置 0 关闭。
5. **串口联机**：**要**（GRBL），对应原版 `machine/grbl.py` + `serial_link.py`。
6. 竖排：暂不做。
7. 表格跨页：整体移到下一页；只有表格比整页还高时才按行拆。
8. 页码/页眉页脚：暂不做。

---

## 8. 当前进度与下一步

### 已完成（2026-11-04 当天）

M0 骨架 → M1 字库 → M2 排版 → M3 表格/公式/图片/分页 → M4 G-code/串口 → M5 GPUI 界面，
全部落地；`cargo test` 27 项、`clippy -D warnings` 全绿。

### 下一步（按优先级）

1. **编辑器中文输入（IME）**：gpui 0.2 核心没有多行输入控件，当前靠「打开文件/粘贴」。
   两条路：接入 `gpui-component` 的 `TextInput`，或自己处理 `window.update_ime_position`
   与平台组合事件。
2. **编辑器体验**：鼠标点击定位光标、选区、滚动、撤销/重做。
3. **公式补全**：矩阵/多行对齐（`\begin{aligned}`）、`\lim`、`\binom`、`\overline`。
4. **表格**：单元格内多段落、跨页按行拆分（现在整表换页）。
5. **串口**：状态栏轮询 `?` 显示机器坐标/进度、暂停/继续、软复位按钮。
6. **打包**：`cargo build --release` + 字库目录随包分发（`fonts_dir()` 加 exe 同级查找）。

### 收尾时的已知问题

* 列表符号 `•` 用几何圆点兜底，位置略偏左（真值应在缩进区居中）。
* 表格列宽在上限内自适应，超长英文单词可能轻微越界（中文按字折行没问题）。
* 手绘抖动的随机数发生器换成了自研 xorshift（分布一致、数值不同）。

---

## 9. 一句话总结

排版规则的核心都记在 §4／§5：**字体度量、挑字回退链、断行禁则、基线推进、
表格列宽与贴底线写字、G-code 抬落笔与坐标映射**。实现与验收已完成，
剩下的是编辑器体验（IME）与打包这类外围工作。

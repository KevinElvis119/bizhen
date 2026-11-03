# 交接上下文（Context Handoff）

> 本文件是「明天接手时只看这一份就能继续」的备忘。写完于 2026-11-04 凌晨。

---

## 1. 参考源码在哪

| 项 | 值 |
|---|---|
| 原项目 | <https://github.com/ZhengXieGang/WriterStudio> |
| 本机完整源码 | `C:\Users\zencoding\WriterStudio`（git HEAD `e0ee07b`，origin 指向上面） |
| 工作区里的 `E:\WritingRobot\WriterStudio` | **坏的 git 目录**：只有 `.git\hooks`、`info`、`objects`、`refs`，缺 `HEAD`/`config`，`git status` 报 "not a git repository"。不影响开发，新工程在 `E:\WritingRobot\writerstudio-rs` |
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
cd E:\WritingRobot\writerstudio-rs
cargo test          # 5 passed
cargo run -p ws-cli -- info futural
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

## 6. 已完成的一致性验证（今晚）

用 Python 原版解析 `futural.jhf`，与 Rust 版输出对比：

| 指标 | Python | Rust | 结果 |
|---|---|---|---|
| ascent | 25.0 | 25.0 | ✅ |
| descent | -7.0 | -7.0 | ✅ |
| default_advance | 17.979166666666668 | 17.979166666666668 | ✅ 逐位 |
| 字形数 | 96 | 96 | ✅ |
| 字母 A 墨迹高 | 21.0 | 21.0 | ✅ |
| 4mm/1.5 行高 | 6.0 | 6.0 | ✅ |

Rust 测试：`cargo test` → 5 passed。

---

## 7. 待定问题（需要拍板，括号内是我的建议）

1. **公式范围**：只做常用子集（上下标/分式/根式/求和积分/括号/希腊字母）就够，还是要更全？
   （建议：先子集，`\frac \sqrt \sum \int \left(\right)` + 常用运算符；解析失败原样输出文本）
2. **图片占位尺寸**：默认取内容宽 × 2/3？支持 `![alt](src){width=80,height=50}` 吗？
   （建议：默认内容宽 × 2/3，支持 `{width=..,height=..}` 单位 mm）
3. **表格合并单元格**（`<<` 跨列 / `^^` 跨行）要不要？
   （建议：保留，它让两层表头能画）
4. **表格手绘抖动**（线端随机 0.5 mm）要不要？
   （建议：保留默认值，参数可关）
5. **竖排**（原版支持纵向书写）要不要？
   （建议：暂不做，M6 再说）
6. **串口发送 / GRBL 联机**要不要？
   （建议：不要，只导出 `.gcode` 文件；联机交给用户现有的工具）
7. **表格跨页**：整体移到下一页，还是按行拆开？
   （建议：整体换页；只有表格比整页还高时才按行拆）
8. **页码/页眉页脚**？
   （建议：暂不做）

---

## 8. 明天开工清单

1. `. .\scripts\dev.ps1`，`cargo test` 确认环境（应 5 passed）。
2. **M1.5 GPUI 冒烟**：给 `ws-app` 加 `gpui = "0.2"`，起一个空白窗口。
   这是最大不确定性——Windows + MSVC + gpui 0.2.2 的组合没验证过；
   如果起不来，先汇报，再决定换 egui 还是纯 CLI（核心层不受影响）。
3. M1：`font/stroke_json.rs`（懒加载 8.4 MB 中文字库）+ `ws-cli info STRK-Kaiti` 验证 20976 字形。
4. M2：`layout.rs` 照抄 §4 表格里的算法，配黄金对比测试。

---

## 9. 一句话总结

排版规则的核心已经全部抄在 §4／§5 里了：**字体度量、挑字回退链、断行禁则、基线推进、
表格列宽与贴底线写字、G-code 抬落笔与坐标映射**。剩下的工作是把这些用 Rust 实现一遍，
每一块都用「Python 导出 → Rust 复现」的黄金对比来验收。

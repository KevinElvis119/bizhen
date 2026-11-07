# 笔阵 · Bizhen

> **笔**是书写，**阵**是排布。名字取自卫夫人《笔阵图》。
> 这个程序做的事很直接：把 Markdown 排成「阵」（折行、分页、表格列宽、公式位置），
> 再交给写字机一笔一笔写出来。

[WriterStudio](https://github.com/ZhengXieGang/WriterStudio)（Python + PySide6）的 Rust 重写版，
只保留 **Markdown 排版 → G-code** 这条主线，字体与排版度量对齐原版，并新增**分页**。
界面用 [gpui](https://crates.io/crates/gpui)（经 GPUI Kit 组件库）。

```
┌──────────────── 顶部：打开 / 导出 G-code / 字体与抖动 / 面板开关 / 翻页 ────────────────┐
│ 页面设置 280 │ 内容编辑 420 │        内容预览（自适应）        │ 机器控制 300 │
└──────────────────────────── 底部：状态 · 缩放 · 纸张 · 折行宽 ───────────────────────┘
```

## 特性

| 能力 | 说明 |
|---|---|
| Markdown 块级 | 标题 1–6 级、段落、有序/无序列表、引用、代码块、分割线 |
| Markdown 表格 | 自动列宽、对齐、单元格内折行、**合并单元格**（`<<` / `^^`）、手绘抖动 |
| 行内公式 | `$...$` 子集：上下标、分式、根式、求和积分、括号、希腊字母 |
| 图片 | 占位不绘制：`![alt](src){width=80,height=50}`（单位 mm） |
| 排版 | 自动折行（含中文标点禁则）、分页（页边距、行级换页、表格整体换页） |
| 字体 | `.jhf`（Hershey）与 `stroke-json` 单线字库，支持多字体链与符号兜底 |
| 手写抖动 | 字符 / 行 / 笔画 / 结构线四级随机，预设 轻微 · 自然 · 强烈，可换种子 |
| G-code | 页面坐标 → 机器坐标、轴反转、写字起点偏移、抬落笔方式可选 |
| 串口 | GRBL 连接、状态轮询、解锁 `$X`、回零 `$H`、软复位、点动、发送作业 |
| 作业控制 | 发送中可**暂停 / 继续 / 终止**；只发送当前预览页 |

## 快速开始

### 环境要求

* Windows 10/11 + **MSVC 生成工具**（Visual Studio「使用 C++ 的桌面开发」）
* Rust 1.85+（MSVC toolchain）
* 首次编译 gpui 依赖较慢（10 分钟量级），后续增量很快

### 构建与运行

```powershell
git clone <repo> bizhen
cd bizhen

# 载入 Rust + MSVC 环境（本会话进程看不到用户级环境变量，必须先跑）
. .\scripts\dev.ps1

cargo build -p bizhen          # 桌面端
cargo run   -p bizhen          # 启动界面

cargo test                     # 35 项单元测试
cargo clippy --all-targets -- -D warnings
```

> 只想用命令行（不需要 gpui）：`cargo run -p bizhen-cli -- md examples/sample.md -o out.gcode`

## 命令行

```powershell
bizhen-cli fonts                     # 列出内置字体
bizhen-cli info <font>               # 字体度量
bizhen-cli ports                     # 列出串口（需 serial 特性）
bizhen-cli md <file.md> [选项]        # Markdown → G-code / SVG / JSON
```

### `md` 的选项

| 选项 | 说明 |
|---|---|
| `-o, --out <path>` | 输出文件，扩展名决定格式：`.gcode` / `.svg` / `.json` |
| `--page A4` | 纸张预设：`A4` `A5` `B5` `A3` `Letter` `明信片` |
| `--landscape` | 横向 |
| `--margin <mm>` | 四边统一页边距 |
| `--margin-top/-bottom/-left/-right <mm>` | 四边独立页边距 |
| `--size <mm>` | 字号（默认 4.0） |
| `--font <链>` | 字体链，逗号分隔，如 `STRK-Kaiti,futural` |
| `--origin-x/--origin-y <mm>` | 写字起点（距纸面左上角） |
| `--invert-x` / `--invert-y` | 反转轴 |
| `--jitter <preset>` `--seed <n>` `--intensity <f>` | 手写抖动 |

## 界面说明

| 区域 | 内容 |
|---|---|
| 页面设置 | 纸张预设 / 自定义尺寸、方向、四边页边距、字号、折行宽、表格宽 |
| 内容编辑 | Kit 代码编辑器：行号、Markdown 高亮、中文输入法、选区、撤销 |
| 内容预览 | 分页预览，滚轮缩放，右键拖动平移，画写字起点十字与坐标轴 |
| 机器控制 | 串口 / 波特率、连接、状态、解锁 / 回零 / 复位、点动、发送作业 |
| 顶栏 | 打开… / 导出 G-code / 字体与抖动 / **隐藏页面** / **隐藏机器** / 翻页 |

* 发送作业只发**当前预览页**；导出 G-code 同样只导当前页（默认文件名 `bizhen-p1.gcode`）。
* 发送过程中「暂停」先抬笔再停投喂，「继续」落笔接着发，「终止」直接软复位 `\x18`。

## 字体

内置：32 款 Hershey `.jhf` + 中文单线楷体 `STRK-Kaiti.json`（20976 字形，懒加载）。

用户字体放 `~/.bizhen/fonts/`（可用环境变量 `BIZHEN_FONT_DIR` 覆盖），支持 `.jhf` 与 `stroke-json`
（`{"U+4E00": [[[x,y],...],...]}`，0..1 归一化、Y 向下）。界面「字体与抖动 → 导入字体…」会
拷贝文件并立即刷新字库。

> **`.gfont` 不再被主程序支持。** 该格式（kvenjoy 加密包）的解析代码已移出本仓库，
> 请用独立工具 `gfont-tool`（`gfont2json`）先转成 `stroke-json` 再导入。

## 目录结构

```
bizhen/
├── crates/
│   ├── bizhen-core/     # 纯逻辑：几何、字体、排版、Markdown、公式、分页、G-code、串口
│   ├── bizhen-cli/      # 命令行：导出、字库信息、串口列举
│   └── bizhen-app/      # GPUI 桌面端（main.rs 界面 + machine.rs 串口线程）
├── assets/fonts/        # 内置字体（许可见 assets/NOTICE.md）
├── examples/            # 示例 Markdown
├── docs/                # 架构 / 使用 / 开发文档，CONTEXT.md 为设计决策日志
├── scripts/dev.ps1      # Windows 环境载入（Rust + MSVC）
└── tools/render_png.py  # 把 CLI 导出的笔画 JSON 渲染成 PNG
```

## 文档

| 文件 | 内容 |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 分层、数据流、关键类型与算法 |
| [docs/USAGE.md](docs/USAGE.md) | 界面与命令行的完整使用手册 |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | 构建环境、代码约定、调试与提交规范 |
| [docs/CONTEXT.md](docs/CONTEXT.md) | 设计决策与踩坑记录（按时间累积） |
| [CHANGELOG.md](CHANGELOG.md) | 各版本对外变化 |

## 已知限制

* 公式是常用子集，不是完整 TeX；解析失败的公式原样显示 `$...$`。
* 侧栏宽度固定（280 / 420 / 300），不可拖拽调整。
* 导出与发送都以「当前页」为单位，没有一次导出全部页的命令。
* 表格手绘抖动用自研 RNG，分布与原版一致但具体数值不同（不影响观感）。
* 界面中文字体走系统字体，Linux 容器里需安装中文字体（如 `fonts-noto-cjk`）。

## 许可

代码 **GPL-3.0-or-later**（见 [LICENSE](LICENSE)）。
内置字体资源版权归各自作者：Hershey 数据集（公有领域，见 `assets/fonts/hershey/ACKNOWLEDGEMENT.txt`）、
中文单线楷体（MIT），详见 [assets/NOTICE.md](assets/NOTICE.md)。

# 笔阵 · Bizhen

> **笔**是书写，**阵**是排布。
> 名字取自卫夫人《笔阵图》——写字先讲「阵」，哪一笔在哪、行与行怎么让位，都有章法。
> 这个程序做的事正是一样：把 Markdown 排成阵，再交给写字机一笔一笔写出来。

[WriterStudio](https://github.com/ZhengXieGang/WriterStudio)（Python + PySide6）的 Rust 重写版：
只保留 **Markdown 排版 → G-code** 这条主线，去掉笔画编辑、手写扰动、TikZ/SVG 导入、MCP 等功能，
字体与排版度量对齐原版，并新增**分页**。UI 用 [gpui](https://crates.io/crates/gpui)。

## 当前状态（M0–M5 已完成）

| 能力 | 状态 |
|---|---|
| 工程骨架（workspace / 3 个 crate） | ✅ |
| 几何与笔画模型（BBox / Stroke / RDP） | ✅ |
| Hershey `.jhf` 解析 + 字体度量 | ✅ 与 Python 版逐位一致 |
| 中文单线字库 `STRK-Kaiti.json`（20976 字形，懒加载） | ✅ |
| 文本排版与折行（含中文标点禁则） | ✅ 与 Python 版数值一致 |
| Markdown：标题/段落/列表/引用/代码/分割线 | ✅ |
| 表格：自动列宽、对齐、合并单元格、手绘抖动 | ✅ |
| 公式：上下标、分式、根式、求和积分、括号、希腊字母 | ✅ 自研排版引擎（不依赖 matplotlib） |
| 图片占位 `![alt](src){width=80,height=50}` | ✅ 只留空不绘制 |
| 全角标点兜底（，。、：；！？《》「」…） | ✅ 几何笔画（字库普遍缺这些字形） |
| 分页（页边距、行级换页、表格整体换页） | ✅ |
| G-code 生成（抬落笔 6 种方式、坐标映射、斜落斜抬、笔序） | ✅ |
| GRBL 串口联机（发程序 / 查状态 / 解锁） | ✅ |
| GPUI 界面（编辑 + 分页预览 + 导出 + 打印） | ✅ 见下方「已知限制」 |

详细任务拆分与验收标准见 [docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)，
源码导读、算法出处与对照数据见 [docs/CONTEXT.md](docs/CONTEXT.md)。

### 已知限制

* 界面里的编辑器是自绘的：中文需要「打开文件」或 `Ctrl+V` 粘贴录入（gpui 0.2 核心
  没有多行文本输入控件，IME 组合输入尚未接入）。
* 公式引擎是常用子集，不是完整 TeX；解析不出来的公式会原样显示 `$...$`。
* 表格手绘抖动用自研 RNG，分布与原版一致但具体数值不同（不影响观感）。

## 快速开始

```powershell
# 1) 载入工具链与 MSVC 环境（本会话进程看不到用户级环境变量，必须先跑）
. .\scripts\dev.ps1

# 2) 构建 + 测试
cargo build
cargo test

# 3) 桌面端
cargo run -p bizhen-app

# 4) 命令行：Markdown → 分页 → G-code / SVG / JSON
cargo run -p bizhen-cli -- md tests\rich.md -o out.gcode
cargo run -p bizhen-cli -- md tests\rich.md -o out.svg
cargo run -p bizhen-cli -- md tests\rich.md -o out.json      # 笔画数据，可配 tools/render_png.py 出图
cargo run -p bizhen-cli -- ports                              # 列串口
```

`bizhen-cli md` 的选项：`--page A4|A5|B5|A3|Letter|…`、`--landscape`、`--margin 10`、
`--size 4`、`--font STRK-Kaiti,futural`。

## 目录

```
bizhen/
├── Cargo.toml               # workspace：default-members 只含核心层
├── .cargo/config.toml       # 工程内 cargo 配置（rsproxy 镜像），不动全局
├── assets/fonts/
│   ├── hershey/*.jhf        # 32 款单线拉丁/希腊/符号（Hershey，可自由使用）
│   └── stroke-json/STRK-Kaiti.json   # 中文单线字库，20976 字形，8.4 MB
├── crates/
│   ├── bizhen-core/             # 纯逻辑：字体/排版/Markdown/公式/分页/G-code/串口
│   ├── bizhen-cli/              # 命令行：排版导出、串口列举、字库信息
│   └── bizhen-app/              # GPUI 桌面端
├── docs/
│   ├── DEVELOPMENT_PLAN.md  # 里程碑、任务、验收标准
│   └── CONTEXT.md           # 交接上下文：源码出处、参数默认值、对照数据
├── scripts/dev.ps1|dev.cmd  # 环境载入（Rust + MSVC）
└── tools/render_png.py      # 把 bizhen-cli 的 JSON 笔画渲染成 PNG（视觉核对）
```

## 许可

GPL-3.0-or-later（与原项目一致）。内置 Hershey 字体数据须随附致谢：
`assets/fonts/hershey/ACKNOWLEDGEMENT.txt`。中文单线字库来自
[chinese-hershey-font](https://github.com/LingDong-/chinese-hershey-font)（MIT）。

## 为什么叫「笔阵」

卫夫人《笔阵图》讲的是写字的布阵：一点如高峰坠石、一横如千里阵云——每一笔落在哪里，
整幅字的气脉就在哪里。程序做的正是同一件事：先把 Markdown 排成「阵」（分页、折行、
表格列宽、公式位置），再让写字机按这个阵一笔一笔落墨。

英文名 **Bizhen** 直接取音；crate 前缀 `bizhen-`、可执行文件 `bizhen`（界面）与
`bizhen-cli`（命令行）。
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
| **`.gfont` 字体包**（kvenjoy 格式：加密头 + ZIP 字形） | ✅ 度量与原版逐位一致（实测 349/8605 字形） |
| 用户字库导入（`.gfont` / `.jhf` / `.json` → `~/.bizhen/fonts`） | ✅ 界面「导入字体…」 |
| **字体与抖动弹窗**：字体链增删/排序、抖动预设与参数、换种子 | ✅ |
| 手写随机抖动（字符/行/笔画/结构线四级） | ✅ 预设：轻微 / 自然 / 强烈 |
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

### 界面

界面跑在 **GPUI Kit**（`gpui-kit 0.9`，底层是 `gpui-pre 0.3.11` 快照）上：

* 编辑器用 Kit 的 `Textarea`：**中文输入法可直接打字**，带选区、撤销、鼠标定位、软换行。
* 串口 / 波特率是 Kit 的下拉菜单（`Button + dropdown_menu`），选中项打勾。
* 按钮统一用 Kit 的 `Button`，悬停、焦点环、键盘可达性由组件库负责。
* 预览画布仍是自绘（`canvas` + `ContentMask` 裁剪），因为它画的是笔画而不是控件。

> 迁移记录见 [docs/CONTEXT.md](docs/CONTEXT.md) 第 12 节。

```
┌──────────────── 顶部：打开 / 导出 / 字体与抖动 / 翻页 ────────────────┐
│ 左栏（Markdown | 页面） │        分页预览        │   右栏：机器      │
└──────────────────────── 底部：状态栏 · 行号 ─────────────────────────┘
```

* **左栏两个标签页**：`Markdown` 是编辑区（滚轮滚动、光标跟随、方向键/Home/End/
  退格/Ctrl+V）；`页面` 放纸张预设、方向、字号、页边距、折行宽、表格宽、预览缩放。
* **中间预览**：缩放后会被裁剪在画布内，不会盖住工具栏。
* **右栏机器**：串口下拉、波特率下拉、刷新端口、连接/断开；状态行显示 GRBL 状态字
  与 MPos；解锁 `$X`、回零 `$H`、软复位 Ctrl-X；手动 Jog（步长 0.1–100mm、
  速度 10–20000 mm/min、Y+/X−/X+/Y−）；「发送作业（打印）」带进度。

串口读写全部跑在后台线程（`machine.rs`），界面只读状态快照，点动和发作业都不会卡 UI。

### 字体与手写抖动

界面上点 **「字体与抖动」** 打开弹窗：

* **字体链**：从内置 + 用户字库里挑多款，按优先级排序（靠前者负责它能画的字符），
  这就是 WriterStudio 的「单文档多字体」机制；末尾的「导入字体…」把 `.gfont`
  （kvenjoy 手写字体包）、`.jhf`、`STRK-*.json` 拷进 `~/.bizhen/fonts` 并立即生效。
* **手写抖动**：预设（关闭 / 轻微 / 自然 / 强烈）+ 单项微调（强度、字号波动、字符位移、
  旋转、行基线、行斜、笔画旋转、笔画修剪、结构线起伏），「换一套」只换种子即可重摇。

抖动作用于：字符（缩放/位移/旋转/斜切）、行（基线/行首/行尾落差）、笔画（旋转/伸缩/修剪）、
结构线（沿线起伏）。表格内的文字与格线**不参与抖动**——格线歪斜会让文字错格，
这一点是刻意的取舍。

### 已知限制

* 界面里的编辑器是自绘的（gpui 0.2 核心没有多行文本输入控件；`gpui-component`
  0.9 依赖自家 `gpui-base`/`gpui-pre` 分支，与 crates.io 的 gpui 0.2.2 不是同一套运行时，
  不能直接换）。已支持滚动、方向键、Home/End、退格删除、粘贴；**中文 IME 组合输入仍未接入**，
  中文靠「打开文件」或 `Ctrl+V`。
* 公式引擎是常用子集，不是完整 TeX；解析不出来的公式会原样显示 `$...$`。
* 表格手绘抖动用自研 RNG，分布与原版一致但具体数值不同（不影响观感）。
* `.gfont` 的「口/回/国」盒形字分级收小还没移植（见 CONTEXT 待办），这类字会略偏大。
* 「字体与抖动」弹窗仍是自绘浮层（Kit Dialog 需要重写约 300 行，见 CONTEXT 第 12 节）。
* 依赖 GPUI Kit 后 debug exe 从 24.7 MB 涨到 48.6 MB（组件代码链了进来）。

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

macOS / Linux：

```bash
. ./scripts/dev.sh
cargo test
cargo run -p bizhen-cli -- md tests/rich.md -o out.gcode
```

`bizhen-cli md` 的选项：`--page A4|A5|B5|A3|Letter|…`、`--landscape`、`--margin 10`、
`--size 4`、`--font STRK-Kaiti,futural`。

## 跨平台

代码本身**没有平台相关的逻辑**，平台差异都收敛在依赖与构建脚本里：

| 平台 | 核心层 + CLI | 桌面端（gpui 0.2） | 额外系统依赖 |
|---|---|---|---|
| Windows | ✅ 已验证（本机） | ✅ 已验证（DirectX / blade） | MSVC 生成工具（`dev.ps1` 会导入 vcvars） |
| macOS | ✅ 已用 `cargo check --target aarch64-apple-darwin` 验证 | ⏳ 待实机 | Xcode Command Line Tools |
| Linux | ⏳ 待实机（代码层无平台分支；见下） | ⏳ 待实机（Vulkan + X11/Wayland） | 见下 |

> 交叉检查记录：macOS 目标下 `bizhen-core` + `bizhen-cli` 类型检查通过；
> Linux 目标仅在 `libudev-sys` 的 pkg-config 探测处停下（缺系统包，不是代码问题）。

Linux 需要装的系统包（Debian/Ubuntu 为例）：

```
libudev-dev            # serialport 枚举串口（可用 --no-default-features 跳过）
libxkbcommon-dev libwayland-dev libx11-xcb-dev   # gpui 的 X11/Wayland 后端
libfontconfig-dev      # gpui 的字体加载（font-kit）
libvulkan-dev mesa-vulkan-drivers                # gpui 的 GPU 后端（blade）
```

只想要「Markdown → G-code」、不需要串口的场合，可以不装 `libudev-dev`：

```bash
cargo build -p bizhen-cli --no-default-features   # 关掉 serial 特性
```

两个注意点：

* **中文界面字体**：界面上的中文标签走系统字体，Linux 容器里若没装中文字体会显示方块；
  文档预览是笔画绘制的，不受影响。装 `fonts-noto-cjk` 即可。
* **文件对话框**：`rfd` 在 Linux 上默认走 xdg-portal，需要桌面 portal 服务；
  纯 headless 环境下「打开/导出」按钮会失败，但命令行导出不受影响。

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
# WriterStudio-RS

[WriterStudio](https://github.com/ZhengXieGang/WriterStudio)（Python + PySide6）的 Rust 重写版：
只保留 **Markdown 排版 → G-code** 这条主线，去掉笔画编辑、手写扰动、TikZ/SVG 导入、MCP 等功能，
并新增**分页**能力。UI 计划用 [gpui](https://crates.io/crates/gpui)。

## 当前状态（M0 已完成）

| 能力 | 状态 |
|---|---|
| 工程骨架（workspace / 3 个 crate） | ✅ |
| 几何与笔画模型（BBox / Stroke / RDP） | ✅ |
| Hershey `.jhf` 解析 + 字体度量 | ✅ 与 Python 版逐位一致 |
| 中文单线字库 `STRK-Kaiti.json` | ⬜ M1 |
| 文本排版与折行 | ⬜ M2 |
| Markdown 块级排版 | ⬜ M2 |
| 表格 / 公式 / 图片占位 | ⬜ M3 |
| 分页 | ⬜ M3 |
| G-code 生成 | ⬜ M4 |
| GPUI 界面 | ⬜ M5 |

详细任务拆分与验收标准见 [docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)，
源码导读、算法出处与待定问题见 [docs/CONTEXT.md](docs/CONTEXT.md)。

## 快速开始

```powershell
# 1) 载入工具链（本会话的进程看不到用户级环境变量，必须先跑这个）
. .\scripts\dev.ps1

# 2) 构建纯逻辑层（快）
cargo build

# 3) 跑测试
cargo test

# 4) 看看内置字库
cargo run -p ws-cli -- fonts
cargo run -p ws-cli -- info futural
```

`ws-app`（GPUI）单独构建：`cargo build -p ws-app`（首次编译 gpui 依赖树，耗时很长）。

## 目录

```
writerstudio-rs/
├── Cargo.toml               # workspace：default-members 只含核心层
├── .cargo/config.toml       # 工程内 cargo 配置（rsproxy 镜像），不动全局
├── assets/fonts/
│   ├── hershey/*.jhf        # 32 款单线拉丁/希腊/符号（Hershey，可自由使用）
│   └── stroke-json/STRK-Kaiti.json   # 中文单线字库，20976 字形，8.4 MB
├── crates/
│   ├── ws-core/             # 纯逻辑：字体/排版/Markdown/公式/分页/G-code
│   ├── ws-cli/              # 命令行：验证核心层、批量导出
│   └── ws-app/              # GPUI 桌面端
└── docs/
    ├── DEVELOPMENT_PLAN.md  # 里程碑、任务、验收标准
    └── CONTEXT.md           # 交接上下文：源码出处、参数默认值、待定问题
```

## 许可

GPL-3.0-or-later（与原项目一致）。内置 Hershey 字体数据须随附致谢：
`assets/fonts/hershey/ACKNOWLEDGEMENT.txt`。中文单线字库来自
[chinese-hershey-font](https://github.com/LingDong-/chinese-hershey-font)（MIT）。

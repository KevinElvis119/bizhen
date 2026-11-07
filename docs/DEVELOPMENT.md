# 开发指南

> 读者：要构建、修改、提交代码的开发者。使用说明见 [USAGE.md](USAGE.md)，架构见 [ARCHITECTURE.md](ARCHITECTURE.md)。

## 1. 构建环境

| 项 | 要求 |
|---|---|
| 系统 | Windows 10/11（主要开发平台） |
| 编译器 | MSVC（Visual Studio「使用 C++ 的桌面开发」） |
| Rust | 1.85+，`x86_64-pc-windows-msvc` toolchain |
| 依赖 | gpui-pre 0.3.11、gpui-kit 0.9、tree-sitter（随构建自动拉取） |

### 环境载入脚本

**每个新终端都要先执行**（它导入 MSVC 的 `INCLUDE` / `LIB` / `PATH`，否则 C 依赖会编译失败）：

```powershell
. .\scripts\dev.ps1
```

脚本做三件事：

1. 设置 `CARGO_HOME` / `RUSTUP_HOME` / `PATH`（本机 Rust 装在自定义目录）
2. 调用 `vcvars64.bat` 拿到真正的 MSVC 环境变量
3. 覆盖掉用户级里指向 VC6 的 `INCLUDE` / `LIB`（否则 `cl.exe` 会去用 VC6 的 `stddef.h`）

macOS / Linux 用 `. ./scripts/dev.sh`。

### 常用命令

```powershell
cargo build              # 默认成员（core + cli），很快
cargo build -p bizhen    # 桌面端，首次较慢
cargo run   -p bizhen

cargo test               # 单元测试
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

### 编译速度

workspace 的 `Cargo.toml` 里有针对 debug 的优化配置：

```toml
[profile.dev.package.bizhen-core]
opt-level = 3          # 排版在 debug 下慢 3 倍以上
[profile.dev.package."*"]
opt-level = 2          # gpui / tree-sitter 在 -O0 下会明显卡
```

如果遇到 `cl: 命令行 error D8050: 无法执行 c1.dll`（PDB 太大），加环境变量：

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '0'
```

### 构建产物被占用

Windows 上 `cargo build` 报 `failed to remove file ...bizhen.exe (os error 5)`：程序还在运行。

```powershell
Get-Process bizhen | Stop-Process -Force
```

## 2. 代码约定

* **语言**：注释与提交信息用中文，标识符用英文。
* **分层**：`bizhen-core` 不允许依赖 gpui；界面逻辑只写在 `bizhen-app`。
* **坐标**：core 内一律 mm、Y 轴向上、页面原点左下角；只有 `gcode.rs` 做机器坐标变换。
* **错误**：core 用 `Result<_, FontError>` 或 `String`，界面层负责翻译成中文提示。
* **格式化**：提交前跑 `cargo fmt --all`；CI 期待 `cargo clippy --all-targets -- -D warnings` 通过。
* **无用代码**：不留 `#[allow(dead_code)]` 长住的死代码，重构时一并删除。

## 3. 修改界面时的坑

这几条都是踩过的，改 `main.rs` 前务必知道：

1. **固定宽度的列必须 `.flex_none()`**，否则窗口变窄时会被压缩，预览被挤成 0 宽而空白。
2. **预览要包在 `.flex()` 容器里**，`canvas` 只有在拿到有效 bounds 时才绘制。
3. `WriterApp` 是唯一状态持有者，改状态后调 `cx.notify()`；跨实体（如编辑器）还要通知对应实体。
4. 文件对话框必须用 `rfd::AsyncFileDialog` + `cx.spawn`：同步版本在 `WeakEntity::update`
   里跑模态循环会导致重入崩溃（第二次「打开」必崩）。
5. 串口命令是异步的：UI 发 `Cmd`，状态从 `machine.snapshot()` 读，不要在 UI 线程等结果。

## 4. 测试

```powershell
cargo test                          # 全部
cargo test -p bizhen-core           # 只测逻辑层
cargo test wrapping                 # 按名字过滤
```

当前共 35 项单元测试，集中在 `bizhen-core`：几何、折行、标点禁则、公式解析、表格合并、
分页、G-code 坐标映射、串口协议解析。

**没有自动化测试的部分**：界面交互、真机串口。改动这两块请用 `examples/*.md` 手动验证，
并尽量在 PR 里附截图或说明实测结果。

视觉核对：`tools/render_png.py` 可以把 `bizhen-cli md -o out.json` 的笔画数据渲染成 PNG。

## 5. 调试技巧

| 需求 | 做法 |
|---|---|
| 看渲染耗时 | 设 `BIZHEN_PERF=1`，渲染超过阈值时打印 `[perf] render N ms` |
| 启动即打开文件 | `BIZHEN_DEMO_OPEN=path\to.md` |
| 启动即连串口 | `BIZHEN_DEMO_CONNECT=1` |
| 换字库目录 | `BIZHEN_FONT_DIR=D:\fonts` |
| 日志 | 目前用 `eprintln!`，输出到 stderr |

## 6. 提交规范

* 提交信息：一行标题 + 空行 + 分条说明「为什么改」，中文书写。
* 一次提交只做一件事；重构与功能改动分开，便于回溯。
* 合并前确保 `cargo test` + `cargo clippy -D warnings` + `cargo fmt --check` 全绿。
* 版本号改动同时更新 `CHANGELOG.md`。

## 7. 发布流程

1. 更新 `Cargo.toml` 的 `[workspace.package] version`
2. 在 `CHANGELOG.md` 顶部加新版本小节
3. `cargo build --release -p bizhen`，确认 `target/release/bizhen.exe` 可运行
4. `git tag -a vX.Y.Z -m "..."` 并推送标签

## 8. 目录职责速查

```
crates/bizhen-core/src/
  geom.rs        几何工具            markdown.rs   Markdown 解析与排版
  stroke.rs      笔画模型            math.rs       公式排版
  layout.rs      折行/标点/行高      render.rs     块 → 笔画
  gcode.rs       G-code 生成         serial.rs     GRBL 串口
  sim.rs         手写抖动            symbols.rs    符号兜底
  font/          字体模型与解析
crates/bizhen-app/src/
  main.rs        界面与状态          machine.rs    串口 worker 线程
crates/bizhen-cli/src/main.rs  命令行入口
```

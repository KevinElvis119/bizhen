# 开发环境载入脚本
#
# 本机 Rust 装在 F: 盘，环境变量只写在「用户级」注册表里；
# DSH/终端会话的进程往往是环境变量写入之前启动的，因此每次新会话先执行：
#
#     . .\scripts\dev.ps1
#
# 之后 cargo / rustc / rustup 即可直接用。

$env:CARGO_HOME  = 'F:\RustLang\Cargo'
$env:RUSTUP_HOME = 'F:\RustLang\Rustup'

if ($env:Path -notlike '*F:\RustLang\Cargo\bin*') {
    $env:Path = "F:\RustLang\Cargo\bin;$env:Path"
}

# 直连 crates.io 会 403（本项目 .cargo/config.toml 已改用 rsproxy 镜像），
# 保留代理变量是为了 rustup / git 等其他下载也能走通。
if ($env:HTTPS_PROXY -eq $null -and (Test-NetConnection 127.0.0.1 -Port 7890 -InformationLevel Quiet -WarningAction SilentlyContinue)) {
    $env:HTTPS_PROXY = 'http://127.0.0.1:7890'
    $env:HTTP_PROXY  = 'http://127.0.0.1:7890'
}

Write-Host "Rust: $(& rustc --version)" -ForegroundColor Green
Write-Host "Cargo home: $env:CARGO_HOME" -ForegroundColor DarkGray

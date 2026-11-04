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

# ---------------------------------------------------------------------------
# MSVC 环境（必须！）
#
# 用户级 INCLUDE/LIB 指向 Visual C++ 6.0（F:\VC98），新版 cl.exe 会拿 VC6 的
# stddef.h 去编译，报「size_t 重定义」「找不到 stdint.h」，进而让 ring 之类的
# C 依赖编译失败。这里用 VS 的 vcvars64.bat 把 INCLUDE/LIB/PATH 换成真实值。
# ---------------------------------------------------------------------------
$vcvars = 'F:\VisualStudio\VC\Auxiliary\Build\vcvars64.bat'
if (Test-Path $vcvars) {
    $raw = & cmd /c "`"$vcvars`" >nul 2>&1 && set"
    foreach ($line in $raw) {
        if ($line -match '^([^=]+)=(.*)$') {
            $k = $Matches[1]
            $v = $Matches[2]
            switch ($k) {
                'INCLUDE' { $env:INCLUDE = $v }
                'LIB'     { $env:LIB = $v }
                'LIBPATH' { $env:LIBPATH = $v }
                'PATH'    { $env:PATH = $v }
                'VCToolsInstallDir' { $env:VCToolsInstallDir = $v }
                'WindowsSdkDir' { $env:WindowsSdkDir = $v }
                'WindowsSDKVersion' { $env:WindowsSDKVersion = $v }
                'UCRTVersion' { $env:UCRTVersion = $v }
            }
        }
    }
}

# vcvars64.bat 会把原有 INCLUDE/LIB 追加在末尾，而本机的旧值是 VC98（VC6）——
# 留着会污染头文件搜索顺序，这里逐个剔除。
foreach ($name in 'INCLUDE','LIB','LIBPATH') {
    $val = [Environment]::GetEnvironmentVariable($name)
    if ($val -and $val -like '*VC98*') {
        $parts = $val -split ';' | Where-Object { $_ -and $_ -notlike '*\VC98\*' }
        [Environment]::SetEnvironmentVariable($name, ($parts -join ';'))
    }
}

# 直连 crates.io 会 403（本项目 .cargo/config.toml 已改用 rsproxy 镜像），
# 保留代理变量是为了 rustup / git 等其他下载也能走通。
if ($env:HTTPS_PROXY -eq $null -and (Test-NetConnection 127.0.0.1 -Port 7890 -InformationLevel Quiet -WarningAction SilentlyContinue)) {
    $env:HTTPS_PROXY = 'http://127.0.0.1:7890'
    $env:HTTP_PROXY  = 'http://127.0.0.1:7890'
}

Write-Host "Rust: $(& rustc --version)" -ForegroundColor Green
Write-Host "Cargo home: $env:CARGO_HOME" -ForegroundColor DarkGray
if ($env:INCLUDE -like '*VC98*') {
    Write-Host "警告：INCLUDE 仍指向 VC98，C 依赖会编译失败" -ForegroundColor Red
} elseif ($env:INCLUDE) {
    Write-Host "MSVC INCLUDE 已就绪" -ForegroundColor DarkGray
}

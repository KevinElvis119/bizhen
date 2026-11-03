@echo off
REM 双击即可进入带 Rust 环境的 PowerShell
pwsh -NoLogo -NoExit -ExecutionPolicy Bypass -Command ". '%~dp0dev.ps1'; Set-Location '%~dp0..'"

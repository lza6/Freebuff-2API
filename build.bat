@echo off
chcp 65001 >nul
title Freebuff2API 编译
cd /d "%~dp0"

echo ============================================
echo   Freebuff2API 编译脚本
echo ============================================
echo.

where cargo >nul 2>nul
if errorlevel 1 (
    echo [错误] 未找到 cargo，请先安装 Rust: https://rustup.rs
    pause
    exit /b 1
)

echo [1/2] 编译 release 版本...
cargo build --release
if errorlevel 1 (
    echo [错误] 编译失败
    pause
    exit /b 1
)

echo.
echo [2/2] 编译成功!
echo   - 二进制: target\release\freebuff2api.exe
echo   - 运行:   start.bat
echo.
pause

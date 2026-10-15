@echo off
rem ===========================================================================
rem  keyforge 一键安装到普通用户目录（Windows）
rem
rem  为什么需要它：
rem    从沙箱/代理工作区里拿出来的 keyforge.exe 往往带有 Low 强制完整性标签，
rem    而 Windows 给进程的完整性级别 = min(父进程 token, 可执行文件标签)。
rem    结果就是：无论你从哪里双击，keyforge 都以 Low 完整性运行，只能改键到
rem    同等或更低完整性的窗口（通常只有启动它的那个终端）。
rem
rem  本脚本做的事：
rem    1) 把 keyforge.exe / keyforged.exe 复制到 %LOCALAPPDATA%\Programs\keyforge
rem    2) 用 icacls 把两个文件的完整性标签显式设为 medium
rem    3) 若还没有配置文件，就把示例配置写到 %APPDATA%\keyforge\config.toml
rem    4) 提示如何验证与自启
rem
rem  用法：把本文件放在 keyforge.exe 旁边（或 ..\target\release 下）双击运行。
rem ===========================================================================
setlocal

set "SRC=%~dp0"
if not exist "%SRC%keyforge.exe" (
    if exist "%SRC%..\target\release\keyforge.exe" (
        set "SRC=%~dp0..\target\release\"
    )
)
if not exist "%SRC%keyforge.exe" (
    echo [x] 找不到 keyforge.exe，请把本脚本放在 keyforge.exe 旁边再运行。
    pause
    exit /b 1
)

set "DEST=%LOCALAPPDATA%\Programs\keyforge"
set "CFGDIR=%APPDATA%\keyforge"
echo [*] 源目录: %SRC%
echo [*] 安装到: %DEST%
mkdir "%DEST%" 2>nul
mkdir "%CFGDIR%" 2>nul

copy /y "%SRC%keyforge.exe"  "%DEST%\keyforge.exe"  >nul || goto :fail
copy /y "%SRC%keyforged.exe" "%DEST%\keyforged.exe" >nul || goto :fail

echo [*] 把完整性标签设为 medium（普通用户级别）...
icacls "%DEST%\keyforge.exe"  /setintegritylevel medium >nul
icacls "%DEST%\keyforged.exe" /setintegritylevel medium >nul

if not exist "%CFGDIR%\config.toml" (
    if exist "%SRC%..\examples\caps-ctrl-emacs.toml" (
        copy /y "%SRC%..\examples\caps-ctrl-emacs.toml" "%CFGDIR%\config.toml" >nul
        echo [*] 已写入示例配置: %CFGDIR%\config.toml
    ) else (
        "%DEST%\keyforge.exe" init >nul
        echo [*] 已生成默认配置: %CFGDIR%\config.toml
    )
) else (
    echo [*] 已存在配置，保持不动: %CFGDIR%\config.toml
)

echo.
echo [*] 自检（应显示 process integrity level: Medium (0x2000) 与 hook round trip: OK）:
"%DEST%\keyforge.exe" doctor
echo.
echo [*] 完事。接下来：
echo     双击或运行  "%DEST%\keyforged.exe"      前台运行
echo     注册登录自启 "%DEST%\keyforge.exe" service install
echo                 schtasks /Create /TN keyforge /XML "%CFGDIR%\keyforge-task.xml" /F
echo     停止运行    "%DEST%\keyforge.exe" stop
echo.
pause
exit /b 0

:fail
echo [x] 复制失败：请确认没有正在运行的 keyforge 实例（可执行 keyforge stop --force）。
pause
exit /b 1

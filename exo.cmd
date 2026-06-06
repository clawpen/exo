@echo off
REM Exo Windows wrapper - runs exo commands via WSL2
REM This script forwards all commands to the Linux exo binary in WSL2

setlocal enabledelayedexpansion

REM Get the directory where this script is located
set "SCRIPT_DIR=%~dp0"
set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"

REM Convert Windows path to WSL path
REM F:\Software\exo -> /mnt/f/Software/exo
set "WSL_EXO_PATH=%SCRIPT_DIR%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:\=/%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:C:=/mnt/c%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:D:=/mnt/d%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:E:=/mnt/e%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:F:=/mnt/f%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:G:=/mnt/g%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:H:=/mnt/h%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:I:=/mnt/i%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:J:=/mnt/j%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:K:=/mnt/k%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:L:=/mnt/l%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:M:=/mnt/m%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:N:=/mnt/n%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:O:=/mnt/o%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:P:=/mnt/p%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:Q:=/mnt/q%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:R:=/mnt/r%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:S:=/mnt/s%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:T:=/mnt/t%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:U:=/mnt/u%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:V:=/mnt/v%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:W:=/mnt/w%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:X:=/mnt/x%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:Y:=/mnt/y%"
set "WSL_EXO_PATH=%WSL_EXO_PATH:Z:=/mnt/z%"

REM The path to the exo binary in WSL2
set "EXO_BINARY=%WSL_EXO_PATH%/target/release/exo"

REM Check if we need to build first
if "%1"=="build" (
    wsl -e bash -c "cd %WSL_EXO_PATH% && cargo build --release"
    exit /b %ERRORLEVEL%
)

REM Check if exo binary exists, if not try to build it
wsl -e bash -c "if [ ! -f %EXO_BINARY% ]; then cd %WSL_EXO_PATH% && cargo build --release 2>/dev/null; fi"

REM Forward all arguments to exo in WSL2 as root (cgroup writes require root
REM until the rootless spawn path is finished). The daemon owns /tmp/exo-daemon.sock
REM at 777 perms so non-root callers can still reach it via the socket protocol,
REM but `exo run`'s CLI path goes through Container::new directly which needs
REM cgroup access. Running as root is the simplest fix for Phase 1.
REM
REM Direct exec (no `bash -c`) so args with spaces survive — bash re-parses
REM the joined string and splits on whitespace, breaking volume paths like
REM "/mnt/f/Software/Claw Pen/data:/data".
wsl -u root -e %EXO_BINARY% %*

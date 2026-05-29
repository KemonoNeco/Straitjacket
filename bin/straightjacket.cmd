@echo off
rem Cross-platform launcher for the straightjacket CLI (Windows side).
rem Dispatches to the prebuilt binary matching the host arch. Committed binaries
rem are named straightjacket-<rust-target-triple>.exe and produced by
rem .github/workflows/build-binaries.yml. Invoked both via PATH (bare
rem `straightjacket` from the skills) and by an explicit
rem ${CLAUDE_PLUGIN_ROOT}/bin/straightjacket path (hooks; resolved through
rem PATHEXT to this .cmd). Must print nothing of its own to stdout — hooks read
rem the dispatched binary's stdout as JSON.
setlocal
set "DIR=%~dp0"
set "CPU=x86_64"
rem Windows on ARM runs x64 binaries under emulation. Detect native ARM64 via
rem both PROCESSOR_ARCHITECTURE and PROCESSOR_ARCHITEW6432 (the latter is set
rem when this launcher itself runs as an emulated x86/x64 process), but fall
rem back to the x64 build unless a native aarch64 binary is actually shipped.
if /I "%PROCESSOR_ARCHITECTURE%"=="ARM64" set "CPU=aarch64"
if /I "%PROCESSOR_ARCHITEW6432%"=="ARM64" set "CPU=aarch64"
if "%CPU%"=="aarch64" if not exist "%DIR%straightjacket-aarch64-pc-windows-msvc.exe" set "CPU=x86_64"
set "BIN=%DIR%straightjacket-%CPU%-pc-windows-msvc.exe"
if not exist "%BIN%" (
  echo straightjacket: no prebuilt binary at "%BIN%" 1>&2
  exit /b 127
)
"%BIN%" %*
exit /b %ERRORLEVEL%

@echo off
REM Build/test the regression-tests crate from git bash (or any shell) on Windows.
REM
REM Usage from the Bash tool / git bash:
REM     cmd //c scripts\cargo-msvc.cmd test --lib
REM     cmd //c scripts\cargo-msvc.cmd clippy --all-targets -- -D warnings
REM (keep any `| tail` etc. on the bash side -- cmd has no `tail`.)
REM
REM Why this wrapper exists (two Windows gotchas it dodges):
REM   1. git bash's /usr/bin/link.exe shadows MSVC's linker -> `link: extra operand`.
REM      Running cargo INSIDE cmd, after vcvars, uses cmd's PATH (MSVC link first).
REM   2. Passing a space-containing "C:\...\vcvars64.bat" directly in a `cmd //c '...'`
REM      arg gets MSYS-escaped to \"C:\...\" and cmd can't parse it. Keeping the quoted
REM      path inside this .cmd file (not on the bash arg line) avoids the mangling.
REM
REM vcvars is resolved via vswhere so this survives VS edition/version upgrades
REM (VS18/2026 today; was the now-stale hardcoded VS2022 path before).
set MSBUILDDISABLENODEREUSE=1
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -prerelease -find VC\Auxiliary\Build\vcvars64.bat`) do set "VCVARS=%%i"
if not defined VCVARS (echo ERROR: vcvars64.bat not found via vswhere>&2 & exit /b 1)
call "%VCVARS%" >nul 2>&1
cargo %*

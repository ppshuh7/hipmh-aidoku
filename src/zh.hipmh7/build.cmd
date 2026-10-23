@echo off
setlocal
rem Build the 0.7-format source and produce all release artifacts.
rem Run from anywhere: cmd /c src\zh.hipmh7\build.cmd
call "%~dp0..\..\env.bat"
cd /d "%~dp0"

rem --- read version from res\source.json (findstr: ASCII key, no encoding pitfalls) ---
set "VER="
for /f "tokens=2 delims=:, " %%a in ('findstr /C:"\"version\"" res\source.json') do set "VER=%%~a"
rem strip stray spaces
set "VER=%VER: =%"
if "%VER%"=="" (
	echo ERROR: could not read info.version from res\source.json
	exit /b 1
)
if "%VER%"=="" (
	echo ERROR: could not read info.version from res\source.json
	exit /b 1
)

rem --- build wasm ---
cargo build --release --target wasm32-unknown-unknown || exit /b 1

rem --- stage Payload (paths inside the zip must use forward slashes,
rem     so always pack with bsdtar, never PowerShell Compress-Archive) ---
set "STAGE=%~dp0stage"
rmdir /s /q "%STAGE%" 2>nul
mkdir "%STAGE%\Payload" || exit /b 1
copy /y "target\wasm32-unknown-unknown\release\hipmh.wasm" "%STAGE%\Payload\main.wasm" >nul || exit /b 1
copy /y "res\source.json" "%STAGE%\Payload\source.json" >nul || exit /b 1
copy /y "res\icon.png" "%STAGE%\Payload\icon.png" >nul || exit /b 1

%SystemRoot%\System32\tar.exe --format zip -cf "%STAGE%\package.aix" -C "%STAGE%" Payload || exit /b 1

rem --- release artifacts ---
copy /y "%STAGE%\package.aix" "..\..\zh.hipmh-0.7.aix" >nul || exit /b 1
copy /y "%STAGE%\package.aix" "..\..\zh.hipmh-0.7-v%VER%.aix" >nul || exit /b 1
if not exist "..\..\source-list" mkdir "..\..\source-list"
copy /y "%STAGE%\package.aix" "..\..\source-list\zh.hipmh-0.7.aix" >nul || exit /b 1
copy /y "res\source.json" "..\..\source-list\source.json" >nul || exit /b 1
copy /y "res\icon.png" "..\..\source-list\icon.png" >nul || exit /b 1

echo OK: zh.hipmh-0.7.aix (v%VER%)

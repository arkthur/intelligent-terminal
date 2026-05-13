@echo off
setlocal

set "ARG_QUIET="
set "ARG_PATH="
set "ARG_SHORTCUTS="

:parse
if "%~1"=="" goto run
if /I "%~1"=="/quiet" set "ARG_QUIET=-Quiet"
if /I "%~1"=="/nopath" set "ARG_PATH=-NoPathUpdate"
if /I "%~1"=="/noshortcuts" set "ARG_SHORTCUTS=-NoShortcuts"
shift
goto parse

:run
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-local-terminal.ps1" -PayloadZip "%~dp0payload.zip" %ARG_QUIET% %ARG_PATH% %ARG_SHORTCUTS%
set "EXITCODE=%ERRORLEVEL%"

endlocal & exit /b %EXITCODE%

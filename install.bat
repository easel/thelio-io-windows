@echo off
setlocal

REM Find the latest MSI in the wix directory
for %%f in (wix\thelio-io-*.msi) do set MSI=%%f

if not defined MSI (
    echo No MSI found in wix\
    pause
    exit /b 1
)

echo Installing %MSI%
net stop thelio-io 2>nul
msiexec /i "%~dp0%MSI%" /qn
timeout /t 2 /nobreak >nul
net start thelio-io
sc query thelio-io
pause

@echo off
title Horizon (Ely.by: zamy)
cd /d "%~dp0"

if not exist "horizon-elyby.json" (
    echo [Horizon] No saved Ely.by session found.
    echo [Horizon] Signing in with Ely.by account 'zamy'...
    echo.
    "target\fast\horizon-client.exe" --resources .local\horizon-client\26.3 --elyby-login zamy
) else (
    echo [Horizon] Starting Horizon with saved Ely.by account...
    "target\fast\horizon-client.exe" --resources .local\horizon-client\26.3
)

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo [Horizon] Process ended with exit code %ERRORLEVEL%.
    pause
)

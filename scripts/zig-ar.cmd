@echo off
if not defined BRIDGE_ZIG_EXE exit /b 2
"%BRIDGE_ZIG_EXE%" ar %*

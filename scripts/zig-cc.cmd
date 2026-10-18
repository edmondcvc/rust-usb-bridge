@echo off
if not defined BRIDGE_ZIG_EXE exit /b 2
"%BRIDGE_ZIG_EXE%" cc -target x86_64-linux-musl %*

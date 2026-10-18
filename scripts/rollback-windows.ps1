[CmdletBinding()]
param([switch]$Elevated)
. (Join-Path $PSScriptRoot 'windows-common.ps1')
if (-not (Test-IsAdministrator)) {
    if ($Elevated) { throw 'Administrator authorization was not granted.' }
    $process = Start-Process -FilePath 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Quote-ProcessArgument $PSCommandPath),'-Elevated') -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    exit $process.ExitCode
}
$record = Read-OwnInstallation
$logs = 'C:\ProgramData\RustUsbBridge\logs'
Assert-NoReparsePoint $logs
$null = New-Item -ItemType Directory -Path $logs -Force
Start-Transcript -LiteralPath (Join-Path $logs ('rollback-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')) -Force | Out-Null
try {
    $service = Get-CheckedBridgeService
    if ($service) {
        Stop-BridgeService
        Set-Service UsbBridgeRust -StartupType Disabled
    }
    Write-Output 'Rust USB 后台服务已停止并禁止自动启动。原 CorelLASER 和 VirtualHere 客户端均保留。'
    Write-Output 'Linux 端还需运行 rollback-linux.sh 释放本软件持有的 USB，然后再在原 VirtualHere 客户端连接。'
    Write-Output '网络断开不等于雕刻机已停止；请按原设备的现场停止流程确认。'
} finally { Stop-Transcript | Out-Null }

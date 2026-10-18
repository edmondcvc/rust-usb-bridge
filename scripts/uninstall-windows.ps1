[CmdletBinding()]
param([switch]$Elevated, [switch]$RemovePairings)
. (Join-Path $PSScriptRoot 'windows-common.ps1')
if (-not (Test-IsAdministrator)) {
    if ($Elevated) { throw 'Administrator authorization was not granted.' }
    $argsForElevation = @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Quote-ProcessArgument $PSCommandPath),'-Elevated')
    if ($RemovePairings) { $argsForElevation += '-RemovePairings' }
    $process = Start-Process -FilePath 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -ArgumentList $argsForElevation -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    exit $process.ExitCode
}
$record = Read-OwnInstallation
$InstallDir = Assert-CPath $record.install_dir 'C:\Program Files\Rust USB Bridge'
Assert-NoReparsePoint $InstallDir
Assert-NoReparsePoint 'C:\ProgramData\RustUsbBridge'
$logs = 'C:\ProgramData\RustUsbBridge\logs'
$null = New-Item -ItemType Directory -Path $logs -Force
Start-Transcript -LiteralPath (Join-Path $logs ('uninstall-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')) -Force | Out-Null
try {
    $service = Get-CheckedBridgeService
    $client = Join-Path $InstallDir 'bridge-client.exe'
    if ($service) {
        if (-not (Test-Path -LiteralPath $client)) { throw 'Own binary is missing while the service is still registered. Reinstall before uninstalling.' }
        $service.Dispose()
        Invoke-Checked $client @('--uninstall-service')
    }
    $shortcut = Assert-CPath $record.shortcut 'C:\Users\Public\Desktop'
    if ($shortcut -ne 'C:\Users\Public\Desktop\Rust USB 客户端.lnk') { throw 'Unexpected shortcut name.' }
    if (Test-Path -LiteralPath $shortcut) {
        $shortcutShell = New-Object -ComObject WScript.Shell
        $currentShortcut = $shortcutShell.CreateShortcut($shortcut)
        if (-not $currentShortcut.TargetPath.Equals($client, [StringComparison]::OrdinalIgnoreCase)) { throw 'The desktop shortcut now targets another application. Refusing to remove it.' }
        Remove-Item -LiteralPath $shortcut -Force
    }
    $arp = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\RustUsbBridge'
    if (Test-Path $arp) { Remove-Item -LiteralPath $arp -Force }
    # Verify the final absolute target immediately before recursive deletion.
    if ([IO.Path]::GetFullPath($InstallDir) -ne 'C:\Program Files\Rust USB Bridge') { throw 'Unexpected removal target.' }
    if (Test-Path -LiteralPath $InstallDir) { Remove-Item -LiteralPath $InstallDir -Recurse -Force }
    if ($RemovePairings) {
        $credentials = Assert-CPath 'C:\ProgramData\RustUsbBridge\credentials' 'C:\ProgramData\RustUsbBridge'
        Assert-NoReparsePoint $credentials
        if (Test-Path -LiteralPath $credentials) { Remove-Item -LiteralPath $credentials -Recurse -Force }
        foreach ($name in @('client.json','installation.json')) {
            $path = Assert-CPath (Join-Path 'C:\ProgramData\RustUsbBridge' $name) 'C:\ProgramData\RustUsbBridge'
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
        }
    }
    Write-Output 'Rust USB Bridge 已卸载。安装与运行日志保留。'
    Write-Output '共享的 USBip 驱动保留；需要移除时，在 Windows“已安装的应用”中单独卸载官方 USBip。'
} finally { Stop-Transcript | Out-Null }

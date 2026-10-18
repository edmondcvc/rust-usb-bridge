Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Avoid loading incompatible PowerShell 7 modules inherited by Windows PowerShell.
$env:PSModulePath = Join-Path $PSHOME 'Modules'
function Assert-CPath([string]$Path, [string]$Within = '') {
    $full = [IO.Path]::GetFullPath($Path)
    if ($full -notmatch '^[A-Za-z]:\\') { throw "Use an absolute local Windows drive path: $full" }
    if ($Within) {
        $root = [IO.Path]::GetFullPath($Within).TrimEnd('\')
        if ($full -ne $root -and -not $full.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Path outside expected directory: $full" }
    }
    return $full
}
function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Assert-NoReparsePoint([string]$Path) {
    $full = Assert-CPath $Path
    # Check every existing parent on the selected local drive before entering it.
    $current = [IO.Path]::GetPathRoot($full)
    foreach ($segment in $full.Substring($current.Length).Split('\')) {
        if (-not $segment) { continue }
        $current = Join-Path $current $segment
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Unexpected junction or symlink: $current" }
        }
    }
}
function Write-BridgeJson([string]$Path, [object]$Value) {
    $null = Assert-CPath $Path
    $text = ConvertTo-Json -InputObject $Value -Depth 10
    [IO.File]::WriteAllText($Path, $text + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
}
function Quote-ProcessArgument([string]$Value) {
    if ($Value.Contains('"')) { throw 'Unexpected quote in process argument.' }
    return '"' + $Value + '"'
}
function Invoke-Checked([string]$File, [string[]]$Arguments, [int[]]$AllowedExitCodes = @(0)) {
    & $File @Arguments
    if ($LASTEXITCODE -notin $AllowedExitCodes) { throw "Process failed with exit $LASTEXITCODE`: $File" }
}
function Protect-BridgeState([string]$Directory) {
    $null = Assert-CPath $Directory 'C:\ProgramData\RustUsbBridge'
    # Numeric SIDs are independent of Windows display language.
    Invoke-Checked 'C:\Windows\System32\icacls.exe' @($Directory, '/inheritance:r', '/grant:r', '*S-1-5-18:(OI)(CI)F', '*S-1-5-32-544:(OI)(CI)F', '*S-1-5-19:(OI)(CI)M')
}
function Invoke-BridgeClientCommand([string]$File, [string[]]$Arguments, [string]$ResultPath) {
    if (Test-Path -LiteralPath $ResultPath) { Remove-Item -LiteralPath $ResultPath -Force }
    $quoted = @($Arguments | ForEach-Object { Quote-ProcessArgument $_ })
    $quoted += @('--output', (Quote-ProcessArgument $ResultPath))
    $process = Start-Process -FilePath $File -ArgumentList $quoted -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        $detail = '客户端未能启动或执行服务操作。'
        if (Test-Path -LiteralPath $ResultPath) {
            try { $detail = (Get-Content -Raw -Encoding UTF8 -LiteralPath $ResultPath | ConvertFrom-Json).error } catch {}
        }
        throw "服务操作失败（退出码 $($process.ExitCode)）：$detail"
    }
}
function Read-OwnInstallation {
    $manifest = 'C:\ProgramData\RustUsbBridge\installation.json'
    Assert-NoReparsePoint $manifest
    if (-not (Test-Path -LiteralPath $manifest)) { throw 'Rust USB Bridge installation manifest is missing. Refusing an unscoped uninstall.' }
    $record = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifest | ConvertFrom-Json
    if ($record.product -ne 'RustUsbBridge' -or $record.install_dir -ne 'C:\Program Files\Rust USB Bridge') { throw 'Unexpected installation identity.' }
    return $record
}
function Get-CheckedBridgeService {
    $service = Get-Service -Name UsbBridgeRust -ErrorAction SilentlyContinue
    if ($service) {
        $details = Get-CimInstance -ClassName Win32_Service -Filter "Name='UsbBridgeRust'"
        $expected = 'C:\Program Files\Rust USB Bridge\bridge-client.exe'
        $command = $details.PathName.TrimStart()
        if (-not ($command.StartsWith('"' + $expected + '"', [StringComparison]::OrdinalIgnoreCase) -or $command.Equals($expected, [StringComparison]::OrdinalIgnoreCase))) {
            throw 'UsbBridgeRust is registered to an unexpected executable. Refusing to modify an unrelated service.'
        }
    }
    return $service
}
function Stop-BridgeService {
    $controller = Get-CheckedBridgeService
    if (-not $controller) { return }
    try {
        $controller.Refresh()
        if ($controller.Status -eq [ServiceProcess.ServiceControllerStatus]::Stopped) { return }
        if ($controller.Status -ne [ServiceProcess.ServiceControllerStatus]::StopPending) {
            try { $controller.Stop() } catch {
                $controller.Refresh()
                if ($controller.Status -ne [ServiceProcess.ServiceControllerStatus]::Stopped) { throw }
            }
        }
        $controller.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(180))
    } finally { $controller.Dispose() }
}

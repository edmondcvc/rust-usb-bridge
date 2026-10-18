[CmdletBinding()]
param(
    [Alias('PackageRoot')][string]$PackageDir = (Split-Path -Parent $PSScriptRoot),
    [string]$LinuxHost = '',
    [ValidateRange(1,65535)][int]$Port = 3241,
    [switch]$Elevated,
    [switch]$NoLaunch
)
. (Join-Path $PSScriptRoot 'windows-common.ps1')
$PackageDir = Assert-CPath $PackageDir
$ScriptFile = Assert-CPath $PSCommandPath
$PowerShellExe = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
if (-not (Test-IsAdministrator)) {
    if ($Elevated) { throw 'Administrator authorization was not granted.' }
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Quote-ProcessArgument $ScriptFile), '-PackageDir', (Quote-ProcessArgument $PackageDir), '-LinuxHost', (Quote-ProcessArgument $LinuxHost), '-Port', $Port, '-Elevated')
    if ($NoLaunch) { $arguments += '-NoLaunch' }
    $process = Start-Process -FilePath $PowerShellExe -ArgumentList $arguments -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    exit $process.ExitCode
}
if (-not [Environment]::Is64BitOperatingSystem -or -not [Environment]::Is64BitProcess) { throw 'Use the x64 Windows installer.' }
$build = [int](Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').CurrentBuildNumber
if ($build -lt 18362) { throw 'Windows 10 x64 version 1903 or later is required.' }
if ($LinuxHost -and $LinuxHost -notmatch '^[a-zA-Z0-9._:-]+$') { throw 'Invalid Linux host.' }
$InstallDir = 'C:\Program Files\Rust USB Bridge'
$StateDir = 'C:\ProgramData\RustUsbBridge'
$LogsDir = Join-Path $StateDir 'logs'
foreach ($directory in @($InstallDir, $StateDir, $LogsDir, (Join-Path $StateDir 'temp'))) {
    $null = Assert-CPath $directory
    Assert-NoReparsePoint $directory
    $null = New-Item -ItemType Directory -Path $directory -Force
}
$env:BRIDGE_INSTALL_STAGE = '设置后台服务目录权限'
Protect-BridgeState $StateDir
$log = Join-Path $LogsDir ('install-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
Start-Transcript -LiteralPath $log -Force | Out-Null
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$env:TEMP = Join-Path $StateDir 'temp'
$env:TMP = $env:TEMP
$rebootRequired = $false
$recordPath = Join-Path $StateDir 'installation.json'
$record = @{product='RustUsbBridge';version='0.2.0-rc.1';install_dir=$InstallDir;shortcut='C:\Users\Public\Desktop\Rust USB 客户端.lnk';installed_vendor_by_us=$false;driver_version='0.9.8.1';driver_release_version='0.9.8.1';driver_pnp_version='12.1.30.403';driver_sha256='38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a';installed_at=(Get-Date).ToUniversalTime().ToString('o');reboot_required=$false;status='preparing'}
$ownRecordValidated = $false
try {
    $oldRecord = $null
    if (Test-Path -LiteralPath $recordPath) {
        $oldRecord = Read-OwnInstallation
        $record.installed_vendor_by_us = [bool]$oldRecord.installed_vendor_by_us
        $ownRecordValidated = $true
        if ($oldRecord.reboot_required) {
            $lastBoot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime()
            if ([DateTime]::Parse($oldRecord.installed_at).ToUniversalTime() -gt $lastBoot) { $rebootRequired = $true }
        }
    } elseif (@(Get-ChildItem -LiteralPath $InstallDir -Force).Count -gt 0) {
        throw 'The destination folder contains files without a RustUsbBridge installation identity. Refusing to overwrite them.'
    }
    $null = Get-CheckedBridgeService
    $clientSource = Join-Path $PackageDir 'bridge-client.exe'
    $vendorSource = Join-Path $PackageDir 'vendor\USBip-0.9.8.1-x64.exe'
    if (-not (Test-Path -LiteralPath $clientSource)) { throw 'Package has no bridge-client.exe.' }
    if (-not (Test-Path -LiteralPath $vendorSource)) { throw 'Package has no official signed USBip installer.' }
    $expectedHash = '38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a'
    if ((Get-FileHash -LiteralPath $vendorSource -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) { throw 'Official USBip installer hash mismatch.' }
    $env:BRIDGE_INSTALL_STAGE = '验证官方 USB 驱动签名'
    $signature = Get-AuthenticodeSignature -LiteralPath $vendorSource
    Write-Output ('Official USBip installer Authenticode: ' + $signature.Status)
    if ($signature.Status -ne 'Valid') { throw 'Official USBip installer signature could not be validated. Inspect network/certificate trust and retry; no signature enforcement settings will be changed.' }
    Write-BridgeJson $recordPath $record
    $ownRecordValidated = $true
    # A third-party USBip installation can be shared with other tools. Do not silently replace it.
    $vendorRegistry = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1'
    $preexistingVendor = Test-Path $vendorRegistry
    $installedVendorByUs = $false
    if ($preexistingVendor) {
        $vendorInfo = Get-ItemProperty $vendorRegistry
        if ($vendorInfo.DisplayVersion -ne '0.9.8.1') { throw "A preexisting USBip $($vendorInfo.DisplayVersion) is installed. This installer will not replace a shared driver automatically. Use the agreed pinned version before retrying." }
        Write-Output 'Reusing existing official USBip 0.9.8.1 installation.'
    } else {
        $env:BRIDGE_INSTALL_STAGE = '安装官方 USB 驱动'
        Write-Output 'Installing official signed USBip driver. USB devices may briefly reconnect. Reboot is required before normal use.'
        $driverLog = Join-Path $LogsDir ('driver-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
    $driverArgs = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/RESTARTEXITCODE=3010', '/TYPE=custom', '/COMPONENTS=main,client', '/TASKS=vcredist', '/MERGETASKS=!desktopicon', '/NOICONS', ('/LOG=' + (Quote-ProcessArgument $driverLog)))
        $driverProcess = Start-Process -FilePath $vendorSource -ArgumentList $driverArgs -WindowStyle Hidden -Wait -PassThru
        if ($driverProcess.ExitCode -notin @(0,3010)) { throw "Official driver installation failed: $($driverProcess.ExitCode). See $driverLog" }
        if (-not (Test-Path $vendorRegistry)) { throw 'Official USBip installation registration was not created.' }
        $installedVendorByUs = $true
        $rebootRequired = $true
        $record.installed_vendor_by_us = $true
        $record.reboot_required = $true
        $record.status = 'driver_installed'
        Write-BridgeJson $recordPath $record
    }
    $env:BRIDGE_INSTALL_STAGE = '检查现有 USB 连接'
    $service = Get-CheckedBridgeService
    if ($service -and $service.Status -eq 'Running') {
        $preflight = Join-Path $StateDir 'temp\upgrade-status.json'
        $existingClient = Join-Path $InstallDir 'bridge-client.exe'
        $probe = Start-Process -FilePath $existingClient -ArgumentList @('--status','--output',(Quote-ProcessArgument $preflight)) -WindowStyle Hidden -Wait -PassThru
        if ($probe.ExitCode -ne 0) { throw '无法确认 USB 占用状态，升级已停止；请检查后台服务。' }
        $current = Get-Content -Raw -Encoding UTF8 -LiteralPath $preflight | ConvertFrom-Json
        if (-not $current.ok -or -not $current.snapshot) { throw '无法确认本机 USB 状态，升级已停止。' }
        if (@($current.snapshot.sessions | Where-Object { $null -ne $_.port -or $_.state -eq 'connecting' }).Count -gt 0) { throw '本机有 USB 正在使用，请先在客户端断开设备，再升级。' }
    }
    # Close only this product's GUI before replacing its loaded executable.
    $installedClient = Join-Path $InstallDir 'bridge-client.exe'
    foreach ($gui in @(Get-Process -Name bridge-client -ErrorAction SilentlyContinue)) {
        if ($gui.Path -eq $installedClient -and $gui.MainWindowHandle -ne 0) {
            $null = $gui.CloseMainWindow()
            if (-not $gui.WaitForExit(10000)) { throw '请关闭 Rust USB 客户端窗口后重试更新。' }
        }
    }
    if ($service) { Stop-BridgeService }
    $env:BRIDGE_INSTALL_STAGE = '安装客户端文件'
    Copy-Item -LiteralPath $clientSource -Destination (Join-Path $InstallDir 'bridge-client.exe') -Force
    $scriptTarget = Join-Path $InstallDir 'scripts'
    $null = New-Item -ItemType Directory -Path $scriptTarget -Force
    foreach ($name in @('windows-common.ps1','uninstall-windows.ps1','rollback-windows.ps1','export-logs.ps1','update-linux-from-windows.ps1','connect-host.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $scriptTarget $name) -Force
    }
    foreach ($name in @('README.md','README.zh-CN.md','README.html','README.zh-CN.html','CONTRIBUTING.md','CONTRIBUTING.html','SECURITY.md','SECURITY.html','CHANGELOG.md','CHANGELOG.html','THIRD_PARTY_NOTICES.md','LICENSE','Cargo.lock')) {
        $source = Join-Path $PackageDir $name
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $InstallDir $name) -Force }
    }
    foreach ($directoryName in @('THIRD_PARTY_LICENSES','docs')) {
        $sourceDirectory = Join-Path $PackageDir $directoryName
        if (Test-Path -LiteralPath $sourceDirectory) {
            $targetDirectory = Assert-CPath (Join-Path $InstallDir $directoryName) $InstallDir
            Assert-NoReparsePoint $targetDirectory
            $null = New-Item -ItemType Directory -Path $targetDirectory -Force
            Copy-Item -Path (Join-Path $sourceDirectory '*') -Destination $targetDirectory -Recurse -Force
        }
    }
    $publicCa = Join-Path $PackageDir 'public\ca.pem'
    if (Test-Path -LiteralPath $publicCa) {
        # This is only the public server CA; no credentials or pairing code are distributed.
        $null = New-Item -ItemType Directory -Path (Join-Path $InstallDir 'public') -Force
        Copy-Item -LiteralPath $publicCa -Destination (Join-Path $InstallDir 'public\ca.pem') -Force
        Copy-Item -LiteralPath $publicCa -Destination (Join-Path $InstallDir 'ca.pem') -Force
    }
    $config = Join-Path $StateDir 'client.json'
    $serverUpdate = Join-Path $PackageDir 'server-update'
    if (Test-Path -LiteralPath $serverUpdate -PathType Container) {
        Copy-Item -LiteralPath $serverUpdate -Destination $InstallDir -Recurse -Force
    }
    if (-not (Test-Path -LiteralPath $config)) {
        Write-BridgeJson $config @{host=$LinuxHost;port=$Port;server_name='usb-bridge';display_name=$env:COMPUTERNAME;proxy_port=0;client_id=$null;auto_connect=@()}
    }
    $client = Join-Path $InstallDir 'bridge-client.exe'
    $env:BRIDGE_INSTALL_STAGE = '注册后台服务'
    if ($service) {
        # The native helper creates a new service; replace only the verified own registration.
        $service.Dispose()
        Invoke-BridgeClientCommand $client @('--uninstall-service') (Join-Path $LogsDir 'service-remove-result.json')
        # SCM deletion may remain pending while another service controller closes.
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        do {
            $pending = Get-Service UsbBridgeRust -ErrorAction SilentlyContinue
            if (-not $pending) { break }
            $pending.Dispose()
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($pending) { throw 'Windows 尚未完成旧服务清理，请关闭服务管理窗口后重新安装。' }
    }
    Invoke-BridgeClientCommand $client @('--install-service','--config',$config) (Join-Path $LogsDir 'service-install-result.json')
    $shortcutPath = 'C:\Users\Public\Desktop\Rust USB 客户端.lnk'
    $null = Assert-CPath $shortcutPath 'C:\Users\Public\Desktop'
    $shell = New-Object -ComObject WScript.Shell
    if (Test-Path -LiteralPath $shortcutPath) {
        $existingShortcut = $shell.CreateShortcut($shortcutPath)
        if (-not $existingShortcut.TargetPath.Equals($client, [StringComparison]::OrdinalIgnoreCase)) { throw 'An unrelated desktop shortcut uses the selected name. It will not be overwritten.' }
    }
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $client
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.Arguments = '--config ' + (Quote-ProcessArgument $config)
    $shortcut.Description = '连接 Linux 上共享的 USB 设备'
    $shortcut.Save()
    $record.status = 'installed'
    $record.reboot_required = $rebootRequired
    Write-BridgeJson $recordPath $record
    $arp = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\RustUsbBridge'
    $null = New-Item -Path $arp -Force
    $uninstallCommand = (Quote-ProcessArgument $PowerShellExe) + ' -NoProfile -ExecutionPolicy Bypass -File ' + (Quote-ProcessArgument (Join-Path $scriptTarget 'uninstall-windows.ps1'))
    foreach ($entry in @{DisplayName='Rust USB Bridge';DisplayVersion=$record.version;Publisher='Rust USB Bridge';InstallLocation=$InstallDir;DisplayIcon=$client;UninstallString=$uninstallCommand}.GetEnumerator()) { Set-ItemProperty $arp -Name $entry.Key -Value $entry.Value }
    $env:BRIDGE_INSTALL_STAGE = '启动后台服务'
    if (-not $rebootRequired) {
        Start-Service UsbBridgeRust
        $ready = Get-Service UsbBridgeRust
        try { $ready.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(30)) }
        finally { $ready.Dispose() }
    }
    if ($rebootRequired) { Write-Output '安装完成。请先重启 Windows，再从桌面的“Rust USB 客户端”打开配对。' }
    else { Write-Output '安装完成。桌面的“Rust USB 客户端”已准备好。' }
    Write-Output ('安装日志：' + $log)
    # The desktop entry starts the normal user's GUI; this elevated process stays an installer.
} catch {
    if ($ownRecordValidated -and (Test-Path -LiteralPath $recordPath)) {
        $record.status = 'failed'
        $record.failure = $_.Exception.Message
        Write-BridgeJson $recordPath $record
    }
    throw
} finally {
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
    Stop-Transcript | Out-Null
}
if ($rebootRequired) { exit 3010 }
exit 0





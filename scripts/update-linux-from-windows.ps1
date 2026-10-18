[CmdletBinding()]
param([string]$LinuxHost = '192.0.2.10', [string]$UserName = 'operator', [ValidateRange(1,65535)][int]$SshPort = 22, [string]$ResultPath = '', [switch]$QuietResult)
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
Add-Type -AssemblyName System.Windows.Forms
$savedEnvironment = @{}
foreach ($name in @('SSH_ASKPASS','SSH_ASKPASS_REQUIRE','DISPLAY','BRIDGE_OWNER_PASSWORD')) { $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name,'Process') }
try {
    if ($LinuxHost -notmatch '^[A-Za-z0-9][A-Za-z0-9.:-]*$' -or $UserName -notmatch '^[a-z_][a-z0-9_-]*$') { throw 'Invalid Linux address or user name.' }
    $root = Split-Path -Parent $PSScriptRoot
    $serverPayload = Join-Path $root 'server-update'
    $authorization = Join-Path $env:LOCALAPPDATA 'RustUsbBridge\owner-authorization'
    $known = Join-Path $authorization 'known_hosts'
    if (-not (Test-Path -LiteralPath $known)) { $known = Join-Path $env:LOCALAPPDATA 'CorelUsbBridge\ssh\known_hosts' }
    foreach ($path in @($known)) { if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw '请先在客户端用 Linux 密码连接一次，以确认主机身份；升级需要完整安装包中的 Linux 服务文件。' } }
    $sshArgs = @('-F','NUL','-T','-p',$SshPort.ToString(),'-o','PreferredAuthentications=password','-o','PubkeyAuthentication=no','-o','NumberOfPasswordPrompts=1','-o','StrictHostKeyChecking=yes','-o',('UserKnownHostsFile='+$known),'-o','ConnectTimeout=8')
    $scpArgs = @('-F','NUL','-P',$SshPort.ToString(),'-o','PreferredAuthentications=password','-o','PubkeyAuthentication=no','-o','NumberOfPasswordPrompts=1','-o','StrictHostKeyChecking=yes','-o',('UserKnownHostsFile='+$known),'-o','ConnectTimeout=8')
    $destination = $UserName + '@' + $LinuxHost
    $statusFile = Join-Path $env:TEMP ('rust-usb-preupgrade-'+[guid]::NewGuid().ToString('N')+'.json')
    try {
        $client = Join-Path $root 'bridge-client.exe'
        $proc = Start-Process -FilePath $client -ArgumentList @('--list','--output',('"'+$statusFile+'"')) -WindowStyle Hidden -Wait -PassThru
        if ($proc.ExitCode -ne 0) { throw '无法读取 Linux 当前占用状态，请先打开客户端确认连接正常。' }
        $reply = Get-Content -Raw -Encoding UTF8 -LiteralPath $statusFile | ConvertFrom-Json
        if (-not $reply.ok -or -not $reply.snapshot -or $reply.snapshot.network_error -or $reply.snapshot.config.host -ne $LinuxHost) { throw 'Linux 状态尚未确认或地址不匹配，升级已取消。' }
        if (@($reply.snapshot.devices | Where-Object { $_.owner }).Count -gt 0) { throw '有人正在使用 USB。请先让所有电脑释放连接，再升级。' }
    } finally { if (Test-Path -LiteralPath $statusFile) { Remove-Item -LiteralPath $statusFile -Force } }
    $credential = Get-Credential -UserName $UserName -Message '一次性升级 Linux USB 服务。输入 Linux 管理员密码；密码仅用于本次升级，不保存。'
    if (-not $credential) { return }
    $null = New-Item -ItemType Directory -Path $authorization -Force
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls.exe $authorization /inheritance:r /grant:r ('*'+$sid+':(OI)(CI)F') '*S-1-5-18:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw '无法保护本机临时授权目录。' }
    $prompt = Join-Path $authorization 'PasswordPrompt.exe'
    if (-not (Test-Path -LiteralPath $prompt)) { Add-Type -OutputAssembly $prompt -OutputType ConsoleApplication -TypeDefinition 'using System; class PasswordPrompt { static void Main() { Console.Write(Environment.GetEnvironmentVariable("BRIDGE_OWNER_PASSWORD")); } }' | Out-Null }
    $env:BRIDGE_OWNER_PASSWORD = $credential.GetNetworkCredential().Password
    $env:SSH_ASKPASS = $prompt; $env:SSH_ASKPASS_REQUIRE = 'force'; $env:DISPLAY = 'usb-upgrade'
    $archOutput = @(& ssh.exe @sshArgs $destination 'uname -m')
    if ($LASTEXITCODE -ne 0) { throw '无法确认 Linux 架构，升级未开始。' }
    $remoteArch = ($archOutput -join '').Trim()
    if ($remoteArch -notin @('x86_64','aarch64')) { throw '仅支持 x86_64 与 aarch64 Linux。' }
    $binary = Join-Path $serverPayload ($remoteArch+'\usb-bridge-server')
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw '安装包中缺少此架构的 Linux 程序，请使用对应架构离线包更新。' }
    $hash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    $remote = '/tmp/rust-usb-update-' + [guid]::NewGuid().ToString('N')
    & ssh.exe @sshArgs $destination "umask 077; mkdir '$remote'"
    if ($LASTEXITCODE -ne 0) { throw '无法连接 Linux，升级未开始。' }
    try {
        & scp.exe @scpArgs $binary ($destination+':'+$remote+'/usb-bridge-server')
        if ($LASTEXITCODE -ne 0) { throw '上传失败，升级未开始。' }
        $script = @'
set -eu
test "$(uname -m)" = __ARCH__
test -f /opt/rust-usb-bridge/usb-bridge-server
test -f /var/lib/rust-usb-bridge/config.json
test "$(sha256sum __SOURCE__ | cut -d ' ' -f 1)" = __HASH__
backup=/opt/rust-usb-bridge/usb-bridge-server.before-0.2.0-rc.1-$(date +%s)
test ! -e "$backup"
cp -p /opt/rust-usb-bridge/usb-bridge-server "$backup"
for status in /sys/bus/usb/devices/*/usbip_status; do
  if [ -f "$status" ] && [ "$(cat "$status")" = 2 ]; then
    echo 'USB is in use. Release all devices before updating.' >&2
    exit 1
  fi
done
systemctl stop rust-usb-bridge
restore() {
  status=$?
  if [ "$status" -ne 0 ]; then
    cp -p "$backup" /opt/rust-usb-bridge/usb-bridge-server
    systemctl start rust-usb-bridge || true
    echo 'Upgrade failed; previous binary restored.' >&2
  fi
}
trap restore EXIT
install -m 755 -o root -g root __SOURCE__ /opt/rust-usb-bridge/usb-bridge-server
systemctl start rust-usb-bridge
sleep 3
if ! systemctl is-active --quiet rust-usb-bridge; then
  exit 1
fi
echo 'Linux upgrade completed; existing authorizations preserved.'
'@
        $script = $script.Replace('__ARCH__', $remoteArch).Replace('__SOURCE__',"'$remote/usb-bridge-server'").Replace('__HASH__',"'$hash'").Replace("`r",'')
        $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($script))
        $command = "sudo -k -S -p '' bash -c 'echo $encoded | base64 -d | bash'"
        $password = $credential.GetNetworkCredential().Password
        try { $output = ($password + "`n") | & ssh.exe @sshArgs $destination $command 2>&1; $exitCode = $LASTEXITCODE }
        finally { $password = $null; $credential = $null }
        if ($exitCode -ne 0) { throw ('升级失败：' + ($output -join "`n")) }
        if ($ResultPath) { @{ok=$true;host=$LinuxHost;sha256=$hash;output=($output -join "`n")} | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding UTF8 }
        if (-not $QuietResult) { [Windows.Forms.MessageBox]::Show('Linux 服务已升级，已有配对保留。','升级完成') | Out-Null }
    } finally { & ssh.exe @sshArgs $destination "rm -f '$remote/usb-bridge-server'; rmdir '$remote'" | Out-Null }
} catch {
    if ($ResultPath) { @{ok=$false;error=$_.Exception.Message} | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding UTF8 }
    if (-not $QuietResult) { [Windows.Forms.MessageBox]::Show($_.Exception.Message,'Linux 升级未完成') | Out-Null }
    exit 1
} finally {
    $password = $null; $credential = $null
    foreach ($name in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name,$savedEnvironment[$name],'Process') }
}

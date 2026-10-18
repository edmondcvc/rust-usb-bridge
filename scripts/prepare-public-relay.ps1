[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$ServerHost,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [ValidateRange(1,65535)][int]$ControlPort=7000,
    [ValidateRange(1,65535)][int]$PublicUsbPort=13241,
    [ValidateRange(1,65535)][int]$PublicSshPort=12222,
    [string]$LinuxAddress='127.0.0.1'
)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'windows-common.ps1')
$OutputDirectory=Assert-CPath $OutputDirectory
if($ServerHost -notmatch '^[A-Za-z0-9][A-Za-z0-9.:-]*$' -or $LinuxAddress -notmatch '^[A-Za-z0-9][A-Za-z0-9.:-]*$'){throw 'Invalid server address.'}
if(@($ControlPort,$PublicUsbPort,$PublicSshPort | Select-Object -Unique).Count -ne 3){throw 'The three relay ports must be different.'}
Assert-NoReparsePoint $OutputDirectory
if(Test-Path -LiteralPath $OutputDirectory){throw 'Use a new directory; existing relay configuration will not be overwritten.'}
$null=New-Item -ItemType Directory -Path $OutputDirectory
& icacls.exe $OutputDirectory /inheritance:r /grant:r ($env:USERNAME+':(OI)(CI)F') 'SYSTEM:(OI)(CI)F' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Cannot protect relay configuration directory.'}
$random=[byte[]]::new(32)
$rng=[Security.Cryptography.RandomNumberGenerator]::Create()
try{$rng.GetBytes($random)}finally{$rng.Dispose()}
$token=([BitConverter]::ToString($random)).Replace('-','').ToLowerInvariant()
$server=@"
bindAddr = "0.0.0.0"
bindPort = $ControlPort
transport.tls.force = true
auth.method = "token"
auth.token = "$token"
auth.additionalScopes = ["HeartBeats", "NewWorkConns"]
allowPorts = [{ single = $PublicUsbPort }, { single = $PublicSshPort }]
maxPortsPerClient = 2
"@
$client=@"
serverAddr = "$ServerHost"
serverPort = $ControlPort
loginFailExit = false
transport.tls.enable = true
auth.method = "token"
auth.token = "$token"
auth.additionalScopes = ["HeartBeats", "NewWorkConns"]

[[proxies]]
name = "my-usb-host"
type = "tcp"
localIP = "$LinuxAddress"
localPort = 3241
remotePort = $PublicUsbPort

[[proxies]]
name = "my-linux-owner-login"
type = "tcp"
localIP = "$LinuxAddress"
localPort = 22
remotePort = $PublicSshPort
"@
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'frps.toml'),$server,[Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'frpc.toml'),$client,[Text.UTF8Encoding]::new($false))
Write-BridgeJson (Join-Path $OutputDirectory 'connection.json') @{host=$ServerHost;ssh_port=$PublicSshPort;usb_port=$PublicUsbPort;control_port=$ControlPort;deployed=$false}
$token=$null
Write-Host 'Relay configuration prepared. It has not been deployed. The two protected TOML files contain the generated relay credential; do not include them in general installers.'

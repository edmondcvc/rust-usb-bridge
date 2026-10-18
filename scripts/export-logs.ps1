[CmdletBinding()]
param([string]$OutputFile = 'C:\Users\Public\Documents\RustUsbBridge-logs.zip')
. (Join-Path $PSScriptRoot 'windows-common.ps1')
$OutputFile = Assert-CPath $OutputFile
$logs = 'C:\ProgramData\RustUsbBridge\logs'
Assert-NoReparsePoint $OutputFile
Assert-NoReparsePoint $logs
if (-not (Test-Path -LiteralPath $logs)) { throw 'No logs are available.' }
# Export logs only; never package client keys, pairing codes, or credentials.
$null = New-Item -ItemType Directory -Path (Split-Path -Parent $OutputFile) -Force
Compress-Archive -LiteralPath $logs -DestinationPath $OutputFile -Force
Write-Output $OutputFile

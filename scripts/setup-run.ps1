[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$PackageDir)
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Text.UTF8Encoding]::new($false)
$env:BRIDGE_INSTALL_STAGE = '检查安装环境'
try {
    & (Join-Path $PSScriptRoot 'install-windows.ps1') -PackageDir $PackageDir -NoLaunch
    exit $LASTEXITCODE
} catch {
    $result = @{ok=$false;stage=$env:BRIDGE_INSTALL_STAGE;error=$_.Exception.Message;location=$_.InvocationInfo.PositionMessage}
    [IO.File]::WriteAllText((Join-Path $PackageDir 'install-result.json'), ($result | ConvertTo-Json -Depth 4), [Text.UTF8Encoding]::new($false))
    [Console]::Error.WriteLine(($result.stage + ': ' + $result.error))
    exit 1
}

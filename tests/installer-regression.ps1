$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$project = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path $project ('.tools\installer-test-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$utf8 = [Text.UTF8Encoding]::new($true)
$bootstrap = [IO.File]::ReadAllText((Join-Path $project 'scripts\setup-run.ps1'))
[IO.File]::WriteAllText((Join-Path $fixture 'setup-run.ps1'),$bootstrap,$utf8)
$fake = 'param([string]$PackageDir,[switch]$NoLaunch); $env:BRIDGE_INSTALL_STAGE="注册后台服务"; throw "测试：访问被拒绝"'
[IO.File]::WriteAllText((Join-Path $fixture 'install-windows.ps1'),$fake,$utf8)
$ps = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
$child = Start-Process $ps -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$fixture+'\setup-run.ps1"'),'-PackageDir',('"'+$fixture+'"')) -Wait -PassThru
if ($child.ExitCode -ne 1) { throw 'Failure did not propagate' }
$report = Get-Content -Raw -Encoding UTF8 (Join-Path $fixture 'install-result.json') | ConvertFrom-Json
if ($report.stage -ne '注册后台服务' -or $report.error -ne '测试：访问被拒绝') { throw 'Chinese failure report was lost' }
# A parser failure before the install script can start must also be captured.
[IO.File]::WriteAllText((Join-Path $fixture 'install-windows.ps1'),'function broken {',$utf8)
$child = Start-Process $ps -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$fixture+'\setup-run.ps1"'),'-PackageDir',('"'+$fixture+'"')) -Wait -PassThru
$report = Get-Content -Raw -Encoding UTF8 (Join-Path $fixture 'install-result.json') | ConvertFrom-Json
if ($child.ExitCode -ne 1 -or $report.stage -ne '检查安装环境' -or -not $report.error) { throw 'Bootstrap failure was not captured' }
$common = [IO.File]::ReadAllText((Join-Path $project 'scripts\windows-common.ps1'))
foreach ($exitCode in @(0,3010)) {
    [IO.File]::WriteAllText((Join-Path $fixture 'install-windows.ps1'),('exit '+$exitCode),$utf8)
    $child = Start-Process $ps -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$fixture+'\setup-run.ps1"'),'-PackageDir',('"'+$fixture+'"')) -Wait -PassThru
    if ($child.ExitCode -ne $exitCode) { throw 'Success or reboot status was lost' }
}
[IO.File]::WriteAllText((Join-Path $fixture 'windows-common.ps1'),$common,$utf8)
. (Join-Path $fixture 'windows-common.ps1')
# Intercept process launch; this never creates or changes a real Windows service.
function Start-Process {
    param($FilePath,$ArgumentList,$WindowStyle,[switch]$Wait,[switch]$PassThru)
    if (-not $Wait -or -not $PassThru) { throw 'Service helper did not wait' }
    if ($ArgumentList -notcontains '--output') { throw 'Missing structured error output' }
    $output = $ArgumentList[-1].Trim('"')
    [IO.File]::WriteAllText($output,'{"ok":false,"error":"服务创建失败"}',[Text.UTF8Encoding]::new($false))
    return [pscustomobject]@{ExitCode=5}
}
$caught = $false
try { Invoke-BridgeClientCommand 'fixture.exe' @('--install-service','--config','C:\path with spaces\client.json') (Join-Path $fixture 'service.json') }
catch { $caught = $_.Exception.Message.Contains('服务创建失败') -and $_.Exception.Message.Contains('5') }
if (-not $caught) { throw 'Service error was swallowed' }
Write-Output 'PASS: bootstrap failure, early parser failure, UTF-8 report, waited service command and native exit propagation.'

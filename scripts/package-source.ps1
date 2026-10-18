[CmdletBinding()]
param([string]$OutputDirectory = '', [string]$PythonExe = 'python')
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $project 'dist' }
& $PythonExe (Join-Path $PSScriptRoot 'audit-public.py')
if ($LASTEXITCODE -ne 0) { throw 'Public source audit failed.' }
& $PythonExe (Join-Path $PSScriptRoot 'release.py') source --output $OutputDirectory
if ($LASTEXITCODE -ne 0) { throw 'Source packaging failed; existing archives are never overwritten.' }
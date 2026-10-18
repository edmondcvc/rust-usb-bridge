[CmdletBinding()]
param(
    [ValidateSet('Build','Check','TestCompile')][string]$Action='Build',
    [ValidateSet('x86_64-unknown-linux-musl','aarch64-unknown-linux-musl')][string]$Target='x86_64-unknown-linux-musl',
    [string]$CargoExe='cargo', [string]$CargoHome='', [string]$RustupHome='', [string]$ZigExe='', [switch]$DebugBuild
)
$ErrorActionPreference='Stop'
$names=@('PATH','CARGO_HOME','RUSTUP_HOME','RUSTFLAGS')
$saved=@{};foreach($name in $names){$saved[$name]=[Environment]::GetEnvironmentVariable($name,'Process')}
try {
    if($CargoHome){$env:CARGO_HOME=$CargoHome};if($RustupHome){$env:RUSTUP_HOME=$RustupHome}
    if($ZigExe){$env:PATH=(Split-Path -Parent (Resolve-Path -LiteralPath $ZigExe).Path)+';'+$env:PATH}
    $env:RUSTFLAGS=$null
    $project=Split-Path -Parent $PSScriptRoot
    $arguments=if($Action -eq 'Check'){@('check')}else{@('zigbuild')}
    $arguments+=@('--locked','--manifest-path',(Join-Path $project 'Cargo.toml'),'--target',$Target,'-p','bridge-server')
    if($Action -eq 'TestCompile'){$arguments+='--tests'}
    if(-not $DebugBuild){$arguments+='--release'}
    & $CargoExe @arguments
    if($LASTEXITCODE -ne 0){throw "Linux cross-build failed: $LASTEXITCODE"}
} finally {foreach($name in $names){[Environment]::SetEnvironmentVariable($name,$saved[$name],'Process')}}

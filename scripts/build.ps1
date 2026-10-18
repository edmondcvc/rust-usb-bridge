[CmdletBinding()]
param([ValidateSet('Build','Test','Check')][string]$Action='Build', [string]$CargoExe='cargo', [string]$CargoHome='', [string]$RustupHome='', [string]$ToolchainBin='', [string]$Target='', [string]$Package='', [string]$SetupPayload='')
$ErrorActionPreference='Stop'
$project=Split-Path -Parent $PSScriptRoot
$names=@('PATH','CARGO_HOME','RUSTUP_HOME','RUSTFLAGS','BRIDGE_SETUP_PAYLOAD')
$saved=@{}; foreach($name in $names){$saved[$name]=[Environment]::GetEnvironmentVariable($name,'Process')}
try {
    if($CargoHome){$env:CARGO_HOME=$CargoHome}; if($RustupHome){$env:RUSTUP_HOME=$RustupHome}
    if($ToolchainBin){$env:PATH=$ToolchainBin+';'+$env:PATH}
    if($SetupPayload){$env:BRIDGE_SETUP_PAYLOAD=(Resolve-Path -LiteralPath $SetupPayload).Path}
    $env:RUSTFLAGS=($env:RUSTFLAGS+' -C target-feature=+crt-static').Trim()
    $verb=@{Build='build';Test='test';Check='check'}[$Action]
    $cargoArguments=@($verb,'--locked','--manifest-path',(Join-Path $project 'Cargo.toml'))
    if($Action -eq 'Build'){$cargoArguments+='--release'}
    if($Target){$cargoArguments+=@('--target',$Target)}
    if($Package){$cargoArguments+=@('-p',$Package)}elseif($Action -ne 'Build'){$cargoArguments+='--workspace'}
    & $CargoExe @cargoArguments
    if($LASTEXITCODE -ne 0){throw "Cargo failed: $LASTEXITCODE"}
} finally {foreach($name in $names){[Environment]::SetEnvironmentVariable($name,$saved[$name],'Process')}}

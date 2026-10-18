[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$WindowsBinary,
    [string]$LinuxBinary = '',
    [string]$LinuxArm64Binary = '',
    [string]$DriverInstaller = '',
    [string]$PublicCa = '',
    [string]$OutputDirectory = '',
    [ValidatePattern('^$|^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$')][string]$ReleaseVersion = '',
    [switch]$WindowsOnly,
    [switch]$StageOnly,
    [switch]$AllowOverwrite
)
. (Join-Path $PSScriptRoot 'windows-common.ps1')
$project = Assert-CPath (Split-Path -Parent $PSScriptRoot)
if (-not $DriverInstaller) { $DriverInstaller = Join-Path $project 'dist\vendor\USBip-0.9.8.1-x64.exe' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $project 'dist' }
$OutputDirectory = Assert-CPath $OutputDirectory
Assert-NoReparsePoint $OutputDirectory
if (-not $WindowsOnly -and -not $LinuxBinary) { throw 'LinuxBinary is required unless WindowsOnly is selected.' }
$requiredFiles = @($WindowsBinary,$DriverInstaller)
if ($LinuxBinary) { $requiredFiles += $LinuxBinary }; if ($LinuxArm64Binary) { $requiredFiles += $LinuxArm64Binary }
foreach ($file in $requiredFiles) {
    $null = Assert-CPath $file
    Assert-NoReparsePoint $file
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Required package file missing: $file" }
}
$driverHash = '38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a'
if ((Get-FileHash -LiteralPath $DriverInstaller -Algorithm SHA256).Hash.ToLowerInvariant() -ne $driverHash) { throw 'Vendor installer hash mismatch.' }
$suffix = if ($ReleaseVersion) { '-' + $ReleaseVersion } else { '' }
$windowsZip = Join-Path $OutputDirectory ('RustUSBBridge-Windows' + $suffix + '-x64.zip')
$linuxArchive = Join-Path $OutputDirectory ('RustUSBBridge-Linux' + $suffix + '-x64.tar.gz')
$archives = @($windowsZip)
if (-not $WindowsOnly) { $archives += $linuxArchive }
$archiveManifest = Join-Path $OutputDirectory ('SHA256SUMS' + $suffix)
foreach ($output in @($archives) + @($archiveManifest)) {
    Assert-NoReparsePoint $output
    if ((Test-Path -LiteralPath $output) -and -not $AllowOverwrite) { throw "Refusing to overwrite a published artifact: $output" }
}
$stage = Join-Path $project '.tools\package'
if ($ReleaseVersion) { $stage = Join-Path $stage $ReleaseVersion }
$winStage = Join-Path $stage 'windows'
$linuxStage = Join-Path $stage 'linux'
$stages = @($winStage)
if (-not $WindowsOnly) { $stages += $linuxStage }
foreach ($directory in $stages) {
    $full = Assert-CPath $directory $stage
    Assert-NoReparsePoint $full
    if ($full -ne $winStage -and $full -ne $linuxStage) { throw 'Unexpected package stage removal target.' }
    if (Test-Path -LiteralPath $full) { Remove-Item -LiteralPath $full -Recurse -Force }
}
foreach ($directory in @($OutputDirectory) + $stages) { $null = New-Item -ItemType Directory -Path $directory -Force }
Copy-Item -LiteralPath $WindowsBinary -Destination (Join-Path $winStage 'bridge-client.exe') -Force
if (-not $WindowsOnly) { Copy-Item -LiteralPath $LinuxBinary -Destination (Join-Path $linuxStage 'usb-bridge-server') -Force }
foreach ($sourceRoot in @((Join-Path $project 'docs'),(Join-Path $project 'THIRD_PARTY_LICENSES'))) {
    Assert-NoReparsePoint $sourceRoot
    foreach ($entry in Get-ChildItem -LiteralPath $sourceRoot -Recurse -Force) { Assert-NoReparsePoint $entry.FullName }
}
foreach ($directory in $stages) {
    foreach ($name in @('README.md','README.zh-CN.md','README.html','README.zh-CN.html','CONTRIBUTING.md','CONTRIBUTING.html','SECURITY.md','SECURITY.html','CHANGELOG.md','CHANGELOG.html','THIRD_PARTY_NOTICES.md','LICENSE','Cargo.lock')) { Copy-Item -LiteralPath (Join-Path $project $name) -Destination (Join-Path $directory $name) -Force }
    $docsTarget = Join-Path $directory 'docs'
    $null = New-Item -ItemType Directory -Path $docsTarget -Force
    Copy-Item -Path (Join-Path $project 'docs\*') -Destination $docsTarget -Recurse -Force
    $licensesTarget = Join-Path $directory 'THIRD_PARTY_LICENSES'
    $null = New-Item -ItemType Directory -Path $licensesTarget -Force
    Copy-Item -Path (Join-Path $project 'THIRD_PARTY_LICENSES\*') -Destination $licensesTarget -Recurse -Force
}
$scriptsTarget = Join-Path $winStage 'scripts'
$null = New-Item -ItemType Directory -Path $scriptsTarget -Force
foreach ($name in @('setup-run.ps1','windows-common.ps1','install-windows.ps1','uninstall-windows.ps1','rollback-windows.ps1','export-logs.ps1','update-linux-from-windows.ps1','connect-host.ps1','prepare-public-relay.ps1')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $scriptsTarget $name) -Force
}
foreach ($subdirectory in @('migration','deployment-snapshots')) {
    $migrationSource = Join-Path $PSScriptRoot $subdirectory
    if (Test-Path -LiteralPath $migrationSource -PathType Container) {
        Assert-NoReparsePoint $migrationSource
        foreach ($entry in Get-ChildItem -LiteralPath $migrationSource -Recurse -Force) { Assert-NoReparsePoint $entry.FullName }
        Copy-Item -LiteralPath $migrationSource -Destination (Join-Path $scriptsTarget $subdirectory) -Recurse -Force
    }
}
if (-not $WindowsOnly) {
    $linuxScripts = Join-Path $linuxStage 'scripts\linux'
    $null = New-Item -ItemType Directory -Path $linuxScripts -Force
    Copy-Item -Path (Join-Path $PSScriptRoot 'linux\*.sh') -Destination $linuxScripts -Force
}
$vendorTarget = Join-Path $winStage 'vendor'
foreach ($entry in @(@{arch='x86_64';binary=$LinuxBinary},@{arch='aarch64';binary=$LinuxArm64Binary})) {
    if ($entry.binary) {
        $serverUpdate = Join-Path $winStage ('server-update\'+$entry.arch)
        $null = New-Item -ItemType Directory -Path $serverUpdate -Force
        Copy-Item -LiteralPath $entry.binary -Destination (Join-Path $serverUpdate 'usb-bridge-server')
    }
}
foreach ($scriptFile in Get-ChildItem -LiteralPath $scriptsTarget -Filter '*.ps1' -Recurse -File) {
    # Windows PowerShell 5.1 requires a BOM to recognize UTF-8 source.
    $scriptText = [IO.File]::ReadAllText($scriptFile.FullName)
    [IO.File]::WriteAllText($scriptFile.FullName, $scriptText, (New-Object Text.UTF8Encoding($true)))
    $parserCheck = '& { $bad = $false; Get-ChildItem -LiteralPath $args[0] -Filter *.ps1 -Recurse | ForEach-Object { $tokens=$null; $errors=$null; [System.Management.Automation.Language.Parser]::ParseFile($_.FullName,[ref]$tokens,[ref]$errors) | Out-Null; if ($errors) { $bad=$true; $errors | ForEach-Object { Write-Output $_.Message } } }; if ($bad) { exit 1 } }'
}
$parserCommand = '$ErrorActionPreference = ''Stop''; ' + $parserCheck + " '" + $scriptsTarget.Replace("'", "''") + "'"
$encodedParser = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($parserCommand))
Invoke-Checked 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' @('-NoProfile','-EncodedCommand',$encodedParser)
$null = New-Item -ItemType Directory -Path $vendorTarget -Force
Copy-Item -LiteralPath $DriverInstaller -Destination (Join-Path $vendorTarget 'USBip-0.9.8.1-x64.exe') -Force
Write-BridgeJson (Join-Path $vendorTarget 'provenance.json') @{project='usbip-win2';version='0.9.8.1';release_version='0.9.8.1';pnp_driver_version='12.1.30.403';architecture='x64';url='https://github.com/vadimgrn/usbip-win2/releases/download/v.0.9.8.1/USBip-0.9.8.1-x64.exe';sha256=$driverHash;signature=(Get-AuthenticodeSignature -LiteralPath $DriverInstaller).Status.ToString()}
if ($ReleaseVersion) {
    Write-BridgeJson (Join-Path $winStage 'release.json') @{product='RustUsbBridge';windows_release=$ReleaseVersion;linux_server_release=$(if ($LinuxBinary) { '0.2.0-rc.1' } else { '0.1.0' });client_sha256=(Get-FileHash -LiteralPath $WindowsBinary -Algorithm SHA256).Hash.ToLowerInvariant();driver_release='0.9.8.1';driver_pnp_version='12.1.30.403';driver_sha256=$driverHash;contains_credentials=$false}
}
if ($PublicCa) {
    $PublicCa = Assert-CPath $PublicCa
    Assert-NoReparsePoint $PublicCa
    $pem = Get-Content -Raw -LiteralPath $PublicCa
    $certificate = [regex]::Match($pem, '\A\s*-----BEGIN CERTIFICATE-----\s*([A-Za-z0-9+/=\r\n]+)\s*-----END CERTIFICATE-----\s*\z')
    if (-not $certificate.Success) { throw 'Only one public CA certificate can be bundled, without keys or appended state.' }
    $der = [Convert]::FromBase64String($certificate.Groups[1].Value)
    $publicCertificate = New-Object Security.Cryptography.X509Certificates.X509Certificate2 -ArgumentList @(,$der)
    try {
        if ($publicCertificate.HasPrivateKey) { throw 'The package CA must not have a private key.' }
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $fingerprint = ([BitConverter]::ToString($sha.ComputeHash($publicCertificate.RawData))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
    } finally { $publicCertificate.Dispose() }
    $publicTarget = Join-Path $winStage 'public'
    $null = New-Item -ItemType Directory -Path $publicTarget -Force
    Copy-Item -LiteralPath $PublicCa -Destination (Join-Path $publicTarget 'ca.pem') -Force
    ('SHA256:' + $fingerprint) | Set-Content -LiteralPath (Join-Path $publicTarget 'fingerprint.txt') -Encoding ASCII
}
foreach ($directory in $stages) {
    $forbidden = Get-ChildItem -LiteralPath $directory -Recurse -File | Where-Object {
        $relative = $_.FullName.Substring($directory.Length + 1).Replace('\','/')
        ($_.Extension -match '^\.(key|pfx|p12|der|log)$') -or
        ($_.Extension -eq '.pem' -and $relative -ne 'public/ca.pem') -or
        ($_.Name -match '^(pair-code\.txt|client\.json|config\.json|installation\.json)$') -or
        ($relative -match '(^|/)(credentials|test-state|target|target-linux|target-client|\.tools|dist|logs)(/|$)')
    }
    if ($forbidden) { throw 'Private credentials or machine state found in packaging stage.' }
    $hashLines = Get-ChildItem -LiteralPath $directory -Recurse -File | Where-Object Name -ne SHA256SUMS | Sort-Object FullName | ForEach-Object {
        (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $_.FullName.Substring($directory.Length + 1).Replace('\','/')
    }
    $hashLines | Set-Content -LiteralPath (Join-Path $directory 'SHA256SUMS') -Encoding ASCII
}
if ($StageOnly) {
    Write-Output $winStage
    if (-not $WindowsOnly) { Write-Output $linuxStage }
    return
}
Compress-Archive -Path (Join-Path $winStage '*') -DestinationPath $windowsZip -Force
if (-not $WindowsOnly) { Invoke-Checked 'C:\Windows\System32\tar.exe' @('-czf',$linuxArchive,'-C',$linuxStage,'.') }
$archives | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName($_) } | Set-Content -LiteralPath $archiveManifest -Encoding ASCII
Write-Output $windowsZip
if (-not $WindowsOnly) { Write-Output $linuxArchive }

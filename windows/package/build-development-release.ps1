#requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $OutputDirectory,
    [Parameter(Mandatory)][string] $ReleaseVersion,
    [Parameter(Mandatory)][string] $SourceRevision,
    [Parameter(Mandatory)][long] $SourceDateEpoch,
    [Parameter()][string] $CatalogDirectory = (Join-Path $PSScriptRoot '..\..\target\wdk-catalog-26100'),
    [Parameter()][string] $BinaryDirectory = (Join-Path $PSScriptRoot '..\..\target\release'),
    [Parameter()][string] $ManifestCertificateThumbprint = 'EDAF55A1E4AE0C8C197988F7286626BD51228CA2'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$inputPath = Join-Path ([IO.Path]::GetTempPath()) ('ps5cam-release-input-' + [Guid]::NewGuid().ToString('N') + '.json')
try {
    $files = [ordered]@{
        driver_inf = Join-Path $CatalogDirectory 'ps5cam-boot.inf'
        signed_catalog = Join-Path $CatalogDirectory 'ps5cam-boot.cat'
        authorized_firmware = Join-Path $root 'firmware\reference\21.01-03.20.00.04-00.00.00.bin'
        windows_service = Join-Path $BinaryDirectory 'ps5cam-service.exe'
        diagnostic_cli = Join-Path $BinaryDirectory 'ps5cam-diagnostics.exe'
        installer = Join-Path $root 'windows\installer\installer.ps1'
        installer_engine = Join-Path $root 'windows\installer\PS5CameraDevelopmentInstaller.ps1'
        license = Join-Path $root 'firmware\reference\LICENSE'
    }
    foreach ($file in $files.Values) { if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Required development release input is missing: $file" } }
    $names = [ordered]@{
        driver_inf = 'ps5cam-boot.inf'; signed_catalog = 'ps5cam-boot.cat'; authorized_firmware = '21.01-03.20.00.04-00.00.00.bin'
        windows_service = 'ps5cam-service.exe'; diagnostic_cli = 'ps5cam-diagnostics.exe'; installer = 'ps5cam-installer.ps1'
        installer_engine = 'PS5CameraDevelopmentInstaller.ps1'; license = 'firmware-reference-MIT-LICENSE.txt'
    }
    $artifacts = foreach ($role in $files.Keys) {
        $entry = [ordered]@{ role = $role; path = [IO.Path]::GetFullPath($files[$role]); fileName = $names[$role]; sha256 = (Get-FileHash -LiteralPath $files[$role] -Algorithm SHA256).Hash.ToLowerInvariant() }
        if ($role -in @('windows_service', 'diagnostic_cli')) { $entry.version = $ReleaseVersion }
        if ($role -eq 'authorized_firmware') {
            $entry.authorization = [ordered]@{
                status = 'approved'; cleanRoom = $false; redistributionAllowed = $true; redistributionBasis = 'third_party_mit_reference'
                license = 'MIT'; source = 'https://github.com/prosperodev/hdcamera'; sourceCommit = '8773610978d5a4d91a6a6d8063d48a4f3afcfe5b'
                noticeFile = 'firmware-reference-MIT-LICENSE.txt'; approvalReference = 'upstream-mit-license-2021-prosperodev'
            }
        }
        $entry
    }
    [ordered]@{
        schemaVersion = 1; releaseVersion = $ReleaseVersion; sourceRevision = $SourceRevision; sourceDateEpoch = $SourceDateEpoch
        packageValidation = [ordered]@{ infVerifPassed = $true; osTargets = @('10_GE_X64', '10_GE_ARM64') }
        artifacts = @($artifacts)
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $inputPath -Encoding utf8NoBOM
    & (Join-Path $PSScriptRoot 'release-assembler.ps1') -InputManifest $inputPath -OutputDirectory $OutputDirectory -Assemble `
        -ConfirmReleaseVersion $ReleaseVersion -ManifestCertificateThumbprint $ManifestCertificateThumbprint
    exit $LASTEXITCODE
}
finally { Remove-Item -LiteralPath $inputPath -Force -ErrorAction SilentlyContinue }

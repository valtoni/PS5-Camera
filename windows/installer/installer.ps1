#requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter()][ValidateSet('Install', 'Repair', 'Uninstall', 'Rollback')][string] $Action = 'Install',
    [Parameter()][string] $ReleaseManifest,
    [Parameter()][string] $BindingObservationPath,
    [Parameter()][string] $ConfirmTemporaryPublishedName,
    [Parameter()][string] $ConfirmReleaseVersion,
    [Parameter()][string] $ConfirmDevelopmentCertificateThumbprint,
    [Parameter()][switch] $RemoveDevelopmentCertificate,
    [Parameter()][switch] $Execute
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($Execute) {
    if ($Action -eq 'Rollback') {
        throw 'Rollback is not enabled by the development-trust installer. Use Uninstall, then install the reviewed release explicitly.'
    }
    $engine = Join-Path $PSScriptRoot 'PS5CameraDevelopmentInstaller.ps1'
    if (-not (Test-Path -LiteralPath $engine -PathType Leaf)) {
        throw 'The verified development installer engine is missing beside installer.ps1.'
    }
    & $engine -Action $Action -ReleaseManifest $ReleaseManifest -ConfirmReleaseVersion $ConfirmReleaseVersion `
        -ConfirmDevelopmentCertificateThumbprint $ConfirmDevelopmentCertificateThumbprint `
        -RemoveDevelopmentCertificate:$RemoveDevelopmentCertificate -Execute
    exit $LASTEXITCODE
}

Import-Module (Join-Path $PSScriptRoot 'InstallerCoordinator.psd1') -Force

$programFilesRoot = [Environment]::GetFolderPath('ProgramFiles')
$programDataRoot = [Environment]::GetFolderPath('CommonApplicationData')
$packagePipeline = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\package\package-pipeline.ps1'))

$plan = New-Ps5CameraInstallerPlan `
    -Action $Action `
    -ReleaseManifest $ReleaseManifest `
    -BindingObservationPath $BindingObservationPath `
    -ConfirmTemporaryPublishedName $ConfirmTemporaryPublishedName `
    -ConfirmReleaseVersion $ConfirmReleaseVersion `
    -Execute:$Execute `
    -ProgramFilesRoot $programFilesRoot `
    -ProgramDataRoot $programDataRoot `
    -PackagePipeline $packagePipeline

$plan | ConvertTo-Json -Depth 14

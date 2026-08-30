#requires -Version 7.0

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$tests = Join-Path $PSScriptRoot 'tests\InstallerCoordinator.Tests.ps1'
$pester = Get-Module -ListAvailable -Name Pester |
    Where-Object { $_.Version.Major -eq 5 } |
    Sort-Object Version -Descending |
    Select-Object -First 1
if ($null -eq $pester) {
    throw 'Pester 5.x is required to run installer tests.'
}
Import-Module $pester.Path -Force

$configuration = New-PesterConfiguration
$configuration.Run.Path = $tests
$configuration.Run.Exit = $true
Invoke-Pester -Configuration $configuration

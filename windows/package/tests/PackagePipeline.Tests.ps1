$PackageRoot = Split-Path -Parent $PSScriptRoot
$Pipeline = Join-Path $PackageRoot 'package-pipeline.ps1'
$PowerShell = (Get-Command 'pwsh.exe' -ErrorAction Stop).Source
$ExpectedHardwareId = 'USB\VID_05A9&PID_0580'
$TestRoot = Join-Path ([IO.Path]::GetTempPath()) ('ps5cam-pester-' + [guid]::NewGuid().ToString('N'))
$StatePath = Join-Path $TestRoot 'install-state.json'

function Invoke-PipelineDryRun {
    param([Parameter(Mandatory)][string[]] $Arguments)
    $output = & $PowerShell -NoLogo -NoProfile -File $Pipeline @Arguments 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Dry-run failed with exit code $LASTEXITCODE`n$output"
    }
    return $output | ConvertFrom-Json
}

Describe 'PS5 camera Windows package pipeline' {
    BeforeAll {
        New-Item -ItemType Directory -Path $TestRoot | Out-Null
        [ordered]@{
            schema_version = 1
            package_id = 'org.ps5camera.boot.winusb'
            hardware_id = $ExpectedHardwareId
            original_inf_name = 'ps5cam-boot.inf'
            published_name = 'oem42.inf'
            status = 'installed'
        } | ConvertTo-Json | Set-Content -LiteralPath $StatePath -Encoding utf8NoBOM

        $script:InspectPlan = Invoke-PipelineDryRun -Arguments @('-Action', 'Inspect')
        $script:CatalogPlan = Invoke-PipelineDryRun -Arguments @('-Action', 'Catalog')
        $script:InstallPlan = Invoke-PipelineDryRun -Arguments @('-Action', 'Install')
        $script:RollbackPlan = Invoke-PipelineDryRun -Arguments @('-Action', 'Rollback', '-StatePath', $StatePath)

        $guardOutput = & $PowerShell -NoLogo -NoProfile -File $Pipeline -Action Catalog -Execute 2>&1 | Out-String
        $script:GuardExitCode = $LASTEXITCODE
        $script:GuardPlan = $guardOutput | ConvertFrom-Json
    }

    AfterAll {
        if (Test-Path -LiteralPath $TestRoot) {
            Remove-Item -LiteralPath $TestRoot -Recurse -Force
        }
    }

    It 'uses dry-run by default and reports any unavailable tool as actionable data' {
        $InspectPlan.mode | Should Be 'dry_run'
        $InspectPlan.action | Should Be 'inspect'
        $InspectPlan.hardware_id | Should Be $ExpectedHardwareId
        @('ready', 'blocked') -contains $InspectPlan.status | Should Be $true
        @($InspectPlan.blockers | Where-Object { $_.resolution.Length -gt 0 }).Count | Should Be @($InspectPlan.blockers).Count
    }

    It 'plans catalog generation only through Inf2Cat and InfVerif' {
        @($CatalogPlan.steps | Where-Object name -eq 'generate-catalog').Count | Should Be 1
        @($CatalogPlan.steps | Where-Object name -eq 'verify-inf').Count | Should Be 1
        @($CatalogPlan.steps | ForEach-Object command) -contains 'makecat.exe' | Should Be $false
    }

    It 'installs only the reviewed INF without force or reboot flags' {
        $install = $InstallPlan.steps | Where-Object name -eq 'install-driver-package'
        $install.arguments[0] | Should Be '/add-driver'
        $install.arguments[-1] | Should Be '/install'
        @($install.arguments) -contains '/force' | Should Be $false
        @($install.arguments) -contains '/reboot' | Should Be $false
        $install.mutates_system | Should Be $true
    }

    It 'rolls back only the published name from transaction state and rescans devices' {
        $remove = $RollbackPlan.steps | Where-Object name -eq 'remove-driver-package'
        $remove.arguments[0] | Should Be '/delete-driver'
        $remove.arguments[1] | Should Be 'oem42.inf'
        $remove.arguments[2] | Should Be '/uninstall'
        @($remove.arguments) -contains '/force' | Should Be $false
        @($RollbackPlan.steps | Where-Object name -eq 'rescan-devices').Count | Should Be 1
    }

    It 'refuses execute mode without exact hardware confirmation' {
        $GuardExitCode -eq 0 | Should Be $false
        @($GuardPlan.blockers | ForEach-Object code) -contains 'hardware_confirmation_required' | Should Be $true
    }
}

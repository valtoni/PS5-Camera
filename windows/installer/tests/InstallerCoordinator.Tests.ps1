#requires -Version 7.0

$InstallerRoot = Split-Path -Parent $PSScriptRoot
$Module = Join-Path $InstallerRoot 'InstallerCoordinator.psd1'
Import-Module $Module -Force

Describe 'PS5 Camera transactional installer planner' {
    BeforeAll {
        $script:InstallerRoot = Split-Path -Parent $PSScriptRoot
        Import-Module (Join-Path $script:InstallerRoot 'InstallerCoordinator.psd1') -Force

function New-SyntheticRelease {
    param(
        [Parameter(Mandatory)][string] $Root,
        [switch] $IncludeAuthorization
    )
    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    $roles = [ordered]@{
        driver_inf = 'ps5cam-boot.inf'
        signed_catalog = 'ps5cam-boot.cat'
        authorized_firmware = 'ps5cam-firmware.bin'
        windows_service = 'ps5cam-service.exe'
        diagnostic_cli = 'ps5cam-diagnostic.exe'
        installer = 'ps5cam-installer.ps1'
        installer_engine = 'PS5CameraDevelopmentInstaller.ps1'
        license = 'LICENSE.txt'
    }
    $artifacts = @()
    foreach ($entry in $roles.GetEnumerator()) {
        $path = Join-Path $Root $entry.Value
        $content = if ($entry.Key -eq 'driver_inf') {
            "[Version]`nClass=USBDevice`nCatalogFile=ps5cam-boot.cat`n[Manufacturer]`n%Provider%=Models,NTamd64,NTarm64`n[Models.NTamd64]`n%Device%=Boot_Install,USB\%DeviceId%`n[Models.NTarm64]`n%Device%=Boot_Install,USB\%DeviceId%`n[Boot_Install]`nInclude=winusb.inf`nNeeds=WINUSB.NT`n[Strings]`nProvider=Test`nDevice=Camera`nDeviceId=VID_05A9&PID_0580`n"
        }
        else { "synthetic fixture for $($entry.Key)" }
        [IO.File]::WriteAllText($path, $content, [Text.UTF8Encoding]::new($false))
        $artifacts += [ordered]@{
            role = $entry.Key
            file_name = $entry.Value
            size = (Get-Item -LiteralPath $path).Length
            sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            version = '0.1.0'
        }
    }
    $manifest = [ordered]@{
        schema_version = 1
        release_version = '0.1.0'
        source_revision = ('a' * 40)
        source_date_epoch = 0
        hardware_ids = @('USB\VID_05A9&PID_0580')
        artifacts = $artifacts
    }
    $sbomPath = Join-Path $Root 'sbom.cdx.json'
    [IO.File]::WriteAllText($sbomPath, '{"bomFormat":"CycloneDX","specVersion":"1.6"}', [Text.UTF8Encoding]::new($false))
    $manifest.sbom = [ordered]@{
        file_name = 'sbom.cdx.json'
        sha256 = (Get-FileHash -LiteralPath $sbomPath -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if ($IncludeAuthorization) {
        $manifest.firmware_authorization = [ordered]@{
            status = 'approved'; clean_room = $true; redistribution_allowed = $true; approval_reference = 'synthetic-test-only'
        }
    }
    $manifestPath = Join-Path $Root 'release-manifest.json'
    $manifest | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifestPath -Encoding utf8NoBOM
    $manifestPath
}

function New-TestPlan {
    param(
        [string] $Action = 'Install',
        [string] $ReleaseManifest,
        [string] $BindingObservationPath,
        [string] $ConfirmTemporaryPublishedName
    )
    $programFiles = Join-Path $TestDrive 'Program Files'
    $programData = Join-Path $TestDrive 'ProgramData'
    New-Item -ItemType Directory -Path $programFiles, $programData -Force | Out-Null
    New-Ps5CameraInstallerPlan -Action $Action -ReleaseManifest $ReleaseManifest `
        -BindingObservationPath $BindingObservationPath `
        -ConfirmTemporaryPublishedName $ConfirmTemporaryPublishedName `
        -ConfirmReleaseVersion '0.1.0' -ProgramFilesRoot $programFiles `
        -ProgramDataRoot $programData -PackagePipeline (Join-Path $InstallerRoot '..\package\package-pipeline.ps1') `
        -SkipLiveBindingInspection
}

function New-OwnedInstallerState {
    param(
        [AllowNull()][object] $RollbackSnapshot = $null,
        [string] $StateStatus = 'installed'
    )
    $stateRoot = Join-Path $TestDrive 'ProgramData\PS5 Camera'
    New-Item -ItemType Directory -Path $stateRoot -Force | Out-Null
    $statePath = Join-Path $stateRoot 'installer-state.json'
    [ordered]@{
        schema_version = 1
        service_name = 'PS5CameraService'
        hardware_id = 'USB\VID_05A9&PID_0580'
        install_root = (Join-Path $TestDrive 'Program Files\PS5 Camera')
        driver_state_path = (Join-Path $stateRoot 'driver-package-state.json')
        previous_published_name = $null
        rollback_snapshot = $RollbackSnapshot
        status = $StateStatus
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
    $statePath
}

    }

    It 'is a blocked, non-mutating dry-run when no release exists' {
        $plan = New-TestPlan
        $plan.mode | Should -Be 'dry_run'
        $plan.status | Should -Be 'blocked'
        (@($plan.blockers.code) -contains 'release_manifest_required') | Should -Be $true
        Test-Path (Join-Path $TestDrive 'Program Files\PS5 Camera') | Should -Be $false
    }

    It 'rejects a release that lacks firmware approval' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'release')
        $plan = New-TestPlan -ReleaseManifest $manifest
        (@($plan.blockers.code) -contains 'firmware_authorization_evidence_missing') | Should -Be $true
    }

    It 'detects tampering before planning copies' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'tampered') -IncludeAuthorization
        Add-Content -LiteralPath (Join-Path (Split-Path -Parent $manifest) 'ps5cam-service.exe') -Value 'tamper'
        $plan = New-TestPlan -ReleaseManifest $manifest
        (@($plan.blockers.code) -contains 'release_artifact_integrity_failed') | Should -Be $true
        (@($plan.steps | ForEach-Object id) -contains 'copy-windows_service') | Should -Be $false
    }

    It 'never includes the protected UVC PID in a driver plan' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'uvc') -IncludeAuthorization
        $document = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
        $document.hardware_ids = @('USB\VID_05A9&PID_0580', 'USB\VID_05A9&PID_058C')
        $document | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifest -Encoding utf8NoBOM
        $plan = New-TestPlan -ReleaseManifest $manifest
        (@($plan.blockers.code) -contains 'release_hardware_scope_invalid') | Should -Be $true
        $plan.protected_hardware_id | Should -Be 'USB\VID_05A9&PID_058C'
    }

    It 'treats external binding JSON as test-only diagnostic evidence' {
        $observation = Join-Path $TestDrive 'binding.json'
        [ordered]@{
            hardware_id = 'USB\VID_05A9&PID_0580'; published_name = 'oem28.inf'; provider = 'libwdi'; temporary = $true
        } | ConvertTo-Json | Set-Content -LiteralPath $observation -Encoding utf8NoBOM
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'binding-release') -IncludeAuthorization
        $plan = New-TestPlan -ReleaseManifest $manifest -BindingObservationPath $observation
        (@($plan.blockers.code) -contains 'external_binding_observation_untrusted') | Should -Be $true
        $plan.binding_evidence.authoritative | Should -Be $false
        $plan.binding_evidence.test_only | Should -Be $true
        @($plan.steps | Where-Object id -eq 'remove-temporary-binding').Count | Should -Be 0
    }

    It 'never promotes externally confirmed oem names into removal authority' {
        $observation = Join-Path $TestDrive 'confirmed-binding.json'
        [ordered]@{
            hardware_id = 'USB\VID_05A9&PID_0580'; published_name = 'oem28.inf'; provider = 'Zadig/libwdi'; temporary = $true
        } | ConvertTo-Json | Set-Content -LiteralPath $observation -Encoding utf8NoBOM
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'confirmed-release') -IncludeAuthorization
        $plan = New-TestPlan -ReleaseManifest $manifest -BindingObservationPath $observation -ConfirmTemporaryPublishedName 'oem28.inf'
        (@($plan.blockers.code) -contains 'external_binding_observation_untrusted') | Should -Be $true
        @($plan.steps | Where-Object id -in @('export-temporary-binding', 'remove-temporary-binding')).Count | Should -Be 0
        $null -eq $plan.temporary_binding | Should -Be $true
    }

    It 'keeps foreign external binding state diagnostic-only rather than guessing' {
        $observation = Join-Path $TestDrive 'foreign-binding.json'
        [ordered]@{
            hardware_id = 'USB\VID_05A9&PID_058C'; published_name = 'oem99.inf'; provider = 'libwdi'; temporary = $true
        } | ConvertTo-Json | Set-Content -LiteralPath $observation -Encoding utf8NoBOM
        $plan = New-TestPlan -BindingObservationPath $observation
        (@($plan.blockers | ForEach-Object code) -contains 'external_binding_observation_untrusted') | Should -Be $true
        (@($plan.steps | ForEach-Object id) -contains 'remove-temporary-binding') | Should -Be $false
    }

    It 'gives every planned system mutation an explicit rollback operation' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'transaction') -IncludeAuthorization
        $plan = New-TestPlan -ReleaseManifest $manifest
        $mutations = @($plan.steps | Where-Object mutates_system)
        $mutations.Count | Should -BeGreaterThan 0
        foreach ($step in $mutations) {
            (-not [string]::IsNullOrWhiteSpace([string]$step.rollback.operation)) | Should -Be $true
        }
        (@($plan.blockers.code) -contains 'non_transactional_step') | Should -Be $false
    }

    It 'preserves the exact Repair snapshot metadata in the committed state journal' {
        $null = New-OwnedInstallerState
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'repair-snapshot') -IncludeAuthorization
        $plan = New-TestPlan -Action Repair -ReleaseManifest $manifest
        $snapshotStep = @($plan.steps | Where-Object id -eq 'snapshot-installed-state')[0]
        $commitStep = @($plan.steps | Where-Object id -eq 'commit-state')[0]
        $null -eq $snapshotStep.arguments.snapshot | Should -Be $false
        ($snapshotStep.arguments.snapshot | ConvertTo-Json -Compress) | Should -Be ($commitStep.arguments.document.rollback_snapshot | ConvertTo-Json -Compress)
        $commitStep.arguments.document.status | Should -Be 'rollback_available'
        (Test-PathInsideRoot (Join-Path $TestDrive 'ProgramData\PS5 Camera\rollback') $commitStep.arguments.document.rollback_snapshot.path) | Should -Be $true
    }

    It 'never reaches ready without a reviewed cryptographic release format' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'authenticity') -IncludeAuthorization
        $null = New-OwnedInstallerState
        foreach ($action in @('Install', 'Repair', 'Uninstall', 'Rollback')) {
            $plan = New-TestPlan -Action $action -ReleaseManifest $manifest
            (@($plan.blockers | ForEach-Object code) -contains 'release_authenticity_format_undefined') | Should -Be $true
            $plan.status | Should -Be 'blocked'
        }
    }

    It 'keeps independent fail-closed gates for staging, reparse points and TOCTOU' {
        $plan = New-TestPlan
        $codes = @($plan.blockers | ForEach-Object code)
        ($codes -contains 'safe_staging_not_implemented') | Should -Be $true
        ($codes -contains 'reparse_point_defense_not_implemented') | Should -Be $true
        ($codes -contains 'artifact_toctou_defense_not_implemented') | Should -Be $true
    }

    It 'parses manufacturer models and macros while ignoring commented hardware IDs' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'structural-inf') -IncludeAuthorization
        $inf = Join-Path (Split-Path -Parent $manifest) 'ps5cam-boot.inf'
        Add-Content -LiteralPath $inf -Value "`n; USB\VID_05A9&PID_058C is documentation only`n[Boot_Install.AddReg]`nHKR,,,0,`"Universal Serial Bus devices`"`n"
        Test-InfScope $inf | Should -Be $true
    }

    It 'rejects a hidden additional manufacturer model after macro expansion' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'extra-model') -IncludeAuthorization
        $inf = Join-Path (Split-Path -Parent $manifest) 'ps5cam-boot.inf'
        Add-Content -LiteralPath $inf -Value "`n[OtherModels.NTamd64]`n%Other%=Boot_Install,USB\%OtherId%`n[OtherManufacturer]`n%Provider%=OtherModels,NTamd64`n[Strings.0409]`nOther=Other`nOtherId=VID_05A9&PID_058C"
        $content = Get-Content -LiteralPath $inf -Raw
        $content = $content.Replace('[Manufacturer]', "[Manufacturer]`n%Provider%=OtherModels,NTamd64")
        [IO.File]::WriteAllText($inf, $content, [Text.UTF8Encoding]::new($false))
        Test-InfScope $inf | Should -Be $false
    }

    It 'rejects extra roles and duplicate destination file names' {
        $manifest = New-SyntheticRelease (Join-Path $TestDrive 'role-set') -IncludeAuthorization
        $document = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
        $extra = $document.artifacts[0].PSObject.Copy()
        $extra.role = 'unreviewed_payload'
        $document.artifacts += $extra
        $document.artifacts[1].file_name = $document.artifacts[0].file_name
        $document | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifest -Encoding utf8NoBOM
        $plan = New-TestPlan -ReleaseManifest $manifest
        (@($plan.blockers.code) -contains 'unexpected_release_role') | Should -Be $true
        (@($plan.blockers.code) -contains 'duplicate_release_file_name') | Should -Be $true
    }

    It 'requires owned installer state for uninstall and rollback' {
        Remove-Item -LiteralPath (Join-Path $TestDrive 'ProgramData\PS5 Camera') -Recurse -Force -ErrorAction SilentlyContinue
        foreach ($action in @('Uninstall', 'Rollback')) {
            $plan = New-TestPlan -Action $action
            (@($plan.blockers.code) -contains 'installer_state_missing') | Should -Be $true
            @($plan.steps | Where-Object operation -match 'driver').Count | Should -Be 0
        }
    }

    It 'validates exact owned paths and rollback snapshot structure in state' {
        $snapshot = [ordered]@{
            schema_version = 1
            kind = 'ps5camera-installer-rollback'
            path = (Join-Path $TestDrive 'outside\snapshot')
        }
        $statePath = New-OwnedInstallerState -RollbackSnapshot $snapshot
        $document = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        $document.install_root = (Join-Path $TestDrive 'foreign-program-files')
        $document.previous_published_name = 'arbitrary.inf'
        $document | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
        $plan = New-TestPlan -Action Uninstall
        (@($plan.blockers.code) -contains 'install_root_ownership_mismatch') | Should -Be $true
        @($plan.steps).Count | Should -Be 0
    }

    It 'rejects unsafe driver state, previous package and rollback snapshot fields independently' {
        $statePath = New-OwnedInstallerState
        $document = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        $document.driver_state_path = (Join-Path $TestDrive 'foreign-driver-state.json')
        $document | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
        $plan = New-TestPlan -Action Uninstall
        (@($plan.blockers | ForEach-Object code) -contains 'driver_state_path_unsafe') | Should -Be $true

        $statePath = New-OwnedInstallerState
        $document = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        $document.previous_published_name = 'not-an-oem-package.inf'
        $document | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
        $plan = New-TestPlan -Action Uninstall
        (@($plan.blockers | ForEach-Object code) -contains 'previous_published_name_invalid') | Should -Be $true

        $statePath = New-OwnedInstallerState -StateStatus 'rollback_available' -RollbackSnapshot ([ordered]@{
                schema_version = 1; kind = 'ps5camera-installer-rollback'; path = (Join-Path $TestDrive 'outside\snapshot')
            })
        $plan = New-TestPlan -Action Rollback
        (@($plan.blockers | ForEach-Object code) -contains 'rollback_snapshot_path_unsafe') | Should -Be $true
    }

    It 'requires a known state status and rejects unknown lifecycle values' {
        $statePath = New-OwnedInstallerState
        $document = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        $document.PSObject.Properties.Remove('status')
        $document | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8NoBOM
        $plan = New-TestPlan -Action Uninstall
        (@($plan.blockers | ForEach-Object code) -contains 'installer_state_structure_invalid') | Should -Be $true

        $statePath = New-OwnedInstallerState -StateStatus 'user_supplied_ready'
        $plan = New-TestPlan -Action Uninstall
        (@($plan.blockers | ForEach-Object code) -contains 'installer_state_status_invalid') | Should -Be $true
    }

    It 'plans rollback only from authenticated-state metadata, without a new release input' {
        $stateRoot = Join-Path $TestDrive 'ProgramData\PS5 Camera'
        $snapshot = [ordered]@{
            schema_version = 1
            kind = 'ps5camera-installer-rollback'
            path = (Join-Path $stateRoot 'rollback\known-snapshot')
        }
        $null = New-OwnedInstallerState -StateStatus 'rollback_available' -RollbackSnapshot $snapshot
        $plan = New-TestPlan -Action Rollback
        $rollback = @($plan.steps | Where-Object id -eq 'rollback-owned-transaction')[0]
        $rollback.arguments.snapshot | Should -Be $snapshot.path
        $null -eq $plan.release_root | Should -Be $true
        (@($plan.blockers | ForEach-Object code) -contains 'authenticated_state_format_undefined') | Should -Be $true
    }

    It 'does not perform binding inspection for Uninstall or Rollback' {
        $null = New-OwnedInstallerState
        foreach ($action in @('Uninstall', 'Rollback')) {
            $plan = New-TestPlan -Action $action -BindingObservationPath (Join-Path $TestDrive 'does-not-exist.json')
            (@($plan.blockers | ForEach-Object code) -contains 'binding_observation_missing') | Should -Be $false
            (@($plan.blockers | ForEach-Object code) -contains 'test_only_binding_inspection_skipped') | Should -Be $false
            $null -eq $plan.binding_evidence | Should -Be $true
        }
    }

    It 'declares and runs on PowerShell 7 or newer' {
        ($PSVersionTable.PSVersion.Major -ge 7) | Should -Be $true
        (Test-Path -LiteralPath (Join-Path $InstallerRoot 'InstallerCoordinator.psd1')) | Should -Be $true
    }
}

InModuleScope InstallerCoordinator {
    Describe 'catalog policy and rollback invariants' {
        It 'checks Authenticode, kernel policy and exact INF membership' {
            Mock Find-SignTool { 'mock-signtool.exe' }
            Mock Invoke-SignToolVerification { $true }
            $blockers = [System.Collections.Generic.List[object]]::new()
            $null = Test-CatalogTrust 'package.cat' 'package.inf' $blockers
            Assert-MockCalled Invoke-SignToolVerification -Times 1 -ParameterFilter { $ToolArguments -contains '/pa' -and $ToolArguments -notcontains '/c' }
            Assert-MockCalled Invoke-SignToolVerification -Times 1 -ParameterFilter { $ToolArguments -contains '/kp' }
            Assert-MockCalled Invoke-SignToolVerification -Times 1 -ParameterFilter { $ToolArguments -contains '/c' -and $ToolArguments -contains 'package.inf' }
            $blockers.Count | Should -Be 0
        }

        It 'reports kernel-policy failure independently of Authenticode and membership' {
            Mock Find-SignTool { 'mock-signtool.exe' }
            Mock Invoke-SignToolVerification { -not ($ToolArguments -contains '/kp') }
            $blockers = [System.Collections.Generic.List[object]]::new()
            $null = Test-CatalogTrust 'package.cat' 'package.inf' $blockers
            (@($blockers | ForEach-Object code) -contains 'catalog_kernel_policy_invalid') | Should -Be $true
            (@($blockers | ForEach-Object code) -contains 'signed_catalog_invalid') | Should -Be $false
            (@($blockers | ForEach-Object code) -contains 'catalog_membership_invalid') | Should -Be $false
        }

        It 'rejects null, scalar and empty-operation rollback values' {
            foreach ($rollback in @($null, 'undo', [ordered]@{}, [ordered]@{ operation = ' ' })) {
                $step = [ordered]@{ id = 'unsafe'; mutates_system = $true; rollback = $rollback }
                (Test-RollbackInvariant $step) | Should -Be $false
            }
            (Test-RollbackInvariant ([ordered]@{ id = 'safe'; mutates_system = $true; rollback = [ordered]@{ operation = 'undo' } })) | Should -Be $true
        }
    }
}

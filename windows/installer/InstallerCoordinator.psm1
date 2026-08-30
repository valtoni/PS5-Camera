#requires -Version 7.0

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:SchemaVersion = 1
$script:HardwareId = 'USB\VID_05A9&PID_0580'
$script:ForbiddenHardwareId = 'USB\VID_05A9&PID_058C'
$script:ServiceName = 'PS5CameraService'
$script:EventSource = 'PS5CameraService'
$script:InstallDirectoryName = 'PS5 Camera'
$script:PinnedReferenceFirmwareSha256 = '10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54'
$script:PinnedReferenceFirmwareFileName = '21.01-03.20.00.04-00.00.00.bin'
$script:PinnedReferenceNoticeFileName = 'firmware-reference-MIT-LICENSE.txt'
$script:AllowedStateStatuses = @('installed', 'repair_pending', 'rollback_available')
$script:RequiredReleaseRoles = @(
    'driver_inf',
    'signed_catalog',
    'authorized_firmware',
    'windows_service',
    'diagnostic_cli',
    'installer',
    'installer_engine',
    'license'
)

function New-InstallerBlocker {
    param(
        [Parameter(Mandatory)][string] $Code,
        [Parameter(Mandatory)][string] $Message,
        [Parameter(Mandatory)][string] $Resolution
    )
    [ordered]@{ code = $Code; message = $Message; resolution = $Resolution }
}

function New-InstallerStep {
    param(
        [Parameter(Mandatory)][string] $Id,
        [Parameter(Mandatory)][string] $Operation,
        [Parameter(Mandatory)][bool] $MutatesSystem,
        [Parameter()][hashtable] $Arguments = @{},
        [Parameter()][AllowNull()][hashtable] $Rollback = $null
    )
    [ordered]@{
        id = $Id
        operation = $Operation
        mutates_system = $MutatesSystem
        arguments = [ordered]@{} + $Arguments
        rollback = if ($MutatesSystem) { [ordered]@{} + $Rollback } else { $null }
    }
}

function Get-CanonicalPath {
    param([Parameter(Mandatory)][string] $Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'Path must not be empty.' }
    [IO.Path]::GetFullPath($Path)
}

function Test-PathInsideRoot {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Candidate
    )
    $rootPath = (Get-CanonicalPath $Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $candidatePath = Get-CanonicalPath $Candidate
    $candidatePath.StartsWith($rootPath, [StringComparison]::OrdinalIgnoreCase)
}

function Get-ReleaseArtifactMap {
    param(
        [Parameter(Mandatory)][object] $Manifest,
        [Parameter(Mandatory)][string] $ReleaseRoot,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Blockers
    )
    $map = [ordered]@{}
    $fileNames = [Collections.Generic.Dictionary[string, string]]::new([StringComparer]::OrdinalIgnoreCase)
    if ($Manifest.PSObject.Properties.Name -notcontains 'artifacts') {
        $Blockers.Add((New-InstallerBlocker 'release_artifacts_missing' 'The release manifest has no artifacts array.' 'Use release-manifest.json emitted by the release assembler.'))
        return $map
    }

    foreach ($artifact in @($Manifest.artifacts)) {
        $properties = @($artifact.PSObject.Properties.Name)
        if (@('role', 'file_name', 'size', 'sha256') | Where-Object { $_ -notin $properties }) {
            $Blockers.Add((New-InstallerBlocker 'release_artifact_metadata_incomplete' 'A release artifact lacks role, file_name, size, or sha256.' 'Regenerate the release with the reviewed assembler.'))
            continue
        }
        $role = [string]$artifact.role
        $fileName = [string]$artifact.file_name
        if ($role -notin $script:RequiredReleaseRoles) {
            $Blockers.Add((New-InstallerBlocker 'unexpected_release_role' "Release contains an unrecognized role: $role" 'Use exactly the reviewed installer artifact-role set.'))
            continue
        }
        if ([string]::IsNullOrWhiteSpace($role) -or $map.Contains($role)) {
            $Blockers.Add((New-InstallerBlocker 'duplicate_or_empty_release_role' "Release role is empty or duplicated: $role" 'Every release role must occur exactly once.'))
            continue
        }
        if ([string]::IsNullOrWhiteSpace($fileName) -or $fileName -cne [IO.Path]::GetFileName($fileName)) {
            $Blockers.Add((New-InstallerBlocker 'unsafe_release_file_name' "Release artifact $role has an unsafe file name." 'Use a plain file name without directory components.'))
            continue
        }
        if ($fileNames.ContainsKey($fileName)) {
            $Blockers.Add((New-InstallerBlocker 'duplicate_release_file_name' "Release roles $($fileNames[$fileName]) and $role use the same file name: $fileName" 'Every role must have a unique destination file name.'))
            continue
        }
        $fileNames[$fileName] = $role
        $path = Get-CanonicalPath (Join-Path $ReleaseRoot $fileName)
        if (-not (Test-PathInsideRoot $ReleaseRoot $path)) {
            $Blockers.Add((New-InstallerBlocker 'release_path_escape' "Release artifact $role escapes the release root." 'Regenerate the release using plain artifact file names.'))
            continue
        }
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            $Blockers.Add((New-InstallerBlocker 'release_artifact_missing' "Release artifact is missing: $fileName" 'Provide the complete assembled release directory.'))
            continue
        }
        $declaredHash = ([string]$artifact.sha256).ToLowerInvariant()
        $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        $actualSize = (Get-Item -LiteralPath $path).Length
        if ($declaredHash -notmatch '^[0-9a-f]{64}$' -or $declaredHash -cne $actualHash -or [long]$artifact.size -ne $actualSize) {
            $Blockers.Add((New-InstallerBlocker 'release_artifact_integrity_failed' "Hash or size mismatch for release artifact $role." 'Discard the directory and obtain the exact assembled release again.'))
            continue
        }
        $map[$role] = [ordered]@{
            role = $role
            file_name = $fileName
            path = $path
            size = $actualSize
            sha256 = $actualHash
            version = if ($properties -contains 'version') { [string]$artifact.version } else { $null }
        }
    }
    $map
}

function Remove-InfComment {
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Line)
    $quoted = $false
    for ($index = 0; $index -lt $Line.Length; $index++) {
        if ($Line[$index] -eq '"') { $quoted = -not $quoted }
        elseif ($Line[$index] -eq ';' -and -not $quoted) { return $Line.Substring(0, $index) }
    }
    $Line
}

function ConvertFrom-InfFile {
    param([Parameter(Mandatory)][string] $Path)
    $sections = [Collections.Generic.Dictionary[string, System.Collections.Generic.List[object]]]::new([StringComparer]::OrdinalIgnoreCase)
    $current = $null
    foreach ($rawLine in Get-Content -LiteralPath $Path) {
        $line = (Remove-InfComment $rawLine).Trim()
        if (-not $line) { continue }
        if ($line -match '^\[([^\]]+)\]$') {
            $current = $Matches[1].Trim()
            if (-not $sections.ContainsKey($current)) { $sections[$current] = [System.Collections.Generic.List[object]]::new() }
            continue
        }
        if (-not $current) { throw "INF directive appears before any section: $rawLine" }
        if ($line -match '^([^=]+?)\s*=\s*(.*)$') {
            $sections[$current].Add([ordered]@{ key = $Matches[1].Trim(); value = $Matches[2].Trim() })
        }
        else {
            # AddReg/DelReg and similar INF sections legitimately contain
            # comma-delimited directives without a key/value separator.
            $sections[$current].Add([ordered]@{ key = $null; value = $line })
        }
    }
    $sections
}

function Get-InfKeyValueMap {
    param(
        [Parameter(Mandatory)][Collections.Generic.Dictionary[string, System.Collections.Generic.List[object]]] $Sections,
        [Parameter(Mandatory)][string] $Section
    )
    $values = [Collections.Generic.Dictionary[string, string]]::new([StringComparer]::OrdinalIgnoreCase)
    if (-not $Sections.ContainsKey($Section)) { return $values }
    foreach ($entry in $Sections[$Section]) {
        if ([string]::IsNullOrWhiteSpace([string]$entry.key)) { throw "Keyless directive is not allowed in [$Section]." }
        if ($values.ContainsKey([string]$entry.key)) { throw "Duplicate INF key $($entry.key) in [$Section]." }
        $values[[string]$entry.key] = ([string]$entry.value).Trim().Trim('"')
    }
    $values
}

function Expand-InfValue {
    param(
        [Parameter(Mandatory)][string] $Value,
        [Parameter(Mandatory)][Collections.Generic.Dictionary[string, string]] $Strings
    )
    $expanded = $Value
    for ($iteration = 0; $iteration -lt 16; $iteration++) {
        $matches = @([regex]::Matches($expanded, '%([^%]+)%'))
        if (-not $matches.Count) { return $expanded }
        $changed = $false
        foreach ($match in $matches) {
            $name = $match.Groups[1].Value
            if (-not $Strings.ContainsKey($name)) { throw "Unresolved INF string macro: %$name%" }
            $expanded = $expanded.Replace($match.Value, $Strings[$name])
            $changed = $true
        }
        if (-not $changed) { break }
    }
    if ($expanded -match '%[^%]+%') { throw 'INF string macro expansion exceeded the safe recursion limit.' }
    $expanded
}

function Get-InfAnalysis {
    param([Parameter(Mandatory)][string] $Path)
    $sections = ConvertFrom-InfFile $Path
    $strings = [Collections.Generic.Dictionary[string, string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($sectionName in @($sections.Keys | Where-Object { $_ -match '^Strings(?:\.|$)' })) {
        foreach ($entry in $sections[$sectionName]) {
            if ([string]::IsNullOrWhiteSpace([string]$entry.key)) { throw "Keyless directive is not allowed in [$sectionName]." }
            $value = ([string]$entry.value).Trim().Trim('"')
            if ($strings.ContainsKey([string]$entry.key) -and $strings[[string]$entry.key] -cne $value) {
                throw "Conflicting localized INF string macro: $($entry.key)"
            }
            $strings[[string]$entry.key] = $value
        }
    }
    if (-not $sections.ContainsKey('Manufacturer')) { throw 'INF has no [Manufacturer] section.' }
    $modelSections = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in $sections['Manufacturer']) {
        if ([string]::IsNullOrWhiteSpace([string]$entry.key)) { throw 'Keyless directive is not allowed in [Manufacturer].' }
        $tokens = @((Expand-InfValue ([string]$entry.value) $strings).Split(',') | ForEach-Object Trim | Where-Object { $_ })
        if (-not $tokens.Count) { throw 'Manufacturer entry has no models section.' }
        if ($tokens.Count -eq 1) { $modelSections.Add($tokens[0]) }
        else { foreach ($decoration in $tokens[1..($tokens.Count - 1)]) { $modelSections.Add("$($tokens[0]).$decoration") } }
    }
    $hardwareIds = [System.Collections.Generic.List[string]]::new()
    $declaredHardwareIds = [System.Collections.Generic.List[string]]::new()
    $installSections = [System.Collections.Generic.List[string]]::new()
    foreach ($modelSection in @($modelSections | Sort-Object -Unique)) {
        if (-not $sections.ContainsKey($modelSection)) { throw "Manufacturer references missing models section [$modelSection]." }
        if (-not $sections[$modelSection].Count) { throw "Models section [$modelSection] is empty." }
        foreach ($entry in $sections[$modelSection]) {
            if ([string]::IsNullOrWhiteSpace([string]$entry.key)) { throw "Keyless directive is not allowed in models section [$modelSection]." }
            $tokens = @((Expand-InfValue ([string]$entry.value) $strings).Split(',') | ForEach-Object Trim | Where-Object { $_ })
            if ($tokens.Count -lt 2) { throw "Model entry in [$modelSection] has no HardwareId." }
            $installSections.Add($tokens[0])
            foreach ($id in $tokens[1..($tokens.Count - 1)]) {
                if ($id -notmatch '^USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}(?:&REV_[0-9A-F]{4})?$') { throw "Unsupported model HardwareId: $id" }
                $hardwareIds.Add($id.ToUpperInvariant())
            }
        }
    }
    foreach ($sectionName in $sections.Keys) {
        foreach ($entry in $sections[$sectionName]) {
            $candidates = @([string]$entry.value)
            foreach ($macro in @([regex]::Matches([string]$entry.value, '%([^%]+)%'))) {
                if ($strings.ContainsKey($macro.Groups[1].Value)) { $candidates += $strings[$macro.Groups[1].Value] }
            }
            foreach ($candidate in $candidates) {
                foreach ($match in @([regex]::Matches($candidate, '(?:USB\\)?VID_[0-9A-F]{4}&PID_[0-9A-F]{4}', 'IgnoreCase'))) {
                    $id = $match.Value.ToUpperInvariant()
                    if (-not $id.StartsWith('USB\')) { $id = "USB\$id" }
                    $declaredHardwareIds.Add($id)
                }
            }
        }
    }
    [ordered]@{
        sections = $sections
        strings = $strings
        version = Get-InfKeyValueMap $sections 'Version'
        model_sections = @($modelSections | Sort-Object -Unique)
        install_sections = @($installSections | Sort-Object -Unique)
        hardware_ids = @($hardwareIds | Sort-Object -Unique)
        declared_hardware_ids = @($declaredHardwareIds | Sort-Object -Unique)
    }
}

function Test-InfScope {
    param([Parameter(Mandatory)][string] $Path)
    try { $analysis = Get-InfAnalysis $Path } catch { return $false }
    if ($analysis.hardware_ids.Count -ne 1 -or $analysis.hardware_ids[0] -cne $script:HardwareId -or
        $analysis.declared_hardware_ids.Count -ne 1 -or $analysis.declared_hardware_ids[0] -cne $script:HardwareId) { return $false }
    if (-not $analysis.version.ContainsKey('Class') -or $analysis.version['Class'] -ine 'USBDevice' -or
        -not $analysis.version.ContainsKey('CatalogFile') -or $analysis.version['CatalogFile'] -ine 'ps5cam-boot.cat') { return $false }
    foreach ($section in $analysis.install_sections) {
        $values = Get-InfKeyValueMap $analysis.sections $section
        if (-not $values.ContainsKey('Include') -or $values['Include'] -ine 'winusb.inf' -or
            -not $values.ContainsKey('Needs') -or $values['Needs'] -ine 'WINUSB.NT') { return $false }
    }
    $true
}

function Find-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { return $command.Source }
    $kits = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits\10'
    if (-not (Test-Path -LiteralPath $kits -PathType Container)) { return $null }
    Get-Item -Path (Join-Path $kits 'bin\*\x64\signtool.exe') -ErrorAction SilentlyContinue |
        Where-Object { -not $_.PSIsContainer } |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

function Test-CatalogTrust {
    param(
        [Parameter(Mandatory)][string] $Catalog,
        [Parameter(Mandatory)][string] $Inf,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Blockers
    )
    $signTool = Find-SignTool
    if (-not $signTool) {
        $Blockers.Add((New-InstallerBlocker 'signtool_missing' 'SignTool is unavailable, so catalog trust and INF membership cannot be revalidated.' 'Install the supported Windows SDK/WDK and rerun the dry-run.'))
        return $null
    }
    if (-not (Invoke-SignToolVerification $signTool @('verify', '/v', '/pa', $Catalog))) {
        $Blockers.Add((New-InstallerBlocker 'signed_catalog_invalid' 'The supplied catalog does not have a trusted signature.' 'Use the real catalog signed by the authorized pipeline.'))
    }
    if (-not (Invoke-SignToolVerification $signTool @('verify', '/v', '/kp', $Catalog))) {
        $Blockers.Add((New-InstallerBlocker 'catalog_kernel_policy_invalid' 'The catalog does not satisfy Windows kernel-mode driver signing policy.' 'Sign the driver package through the authorized Windows distribution pipeline and verify with SignTool /kp.'))
    }
    if (-not (Invoke-SignToolVerification $signTool @('verify', '/v', '/pa', '/c', $Catalog, $Inf))) {
        $Blockers.Add((New-InstallerBlocker 'catalog_membership_invalid' 'The catalog does not cover the exact supplied INF.' 'Generate and sign the catalog from this exact INF.'))
    }
    $signTool
}

function Invoke-SignToolVerification {
    param(
        [Parameter(Mandatory)][string] $Tool,
        [Parameter(Mandatory)][string[]] $ToolArguments
    )
    & $Tool @ToolArguments *> $null
    $LASTEXITCODE -eq 0
}

function Read-BindingObservation {
    param(
        [Parameter()][string] $ObservationPath,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Blockers
    )
    if ([string]::IsNullOrWhiteSpace($ObservationPath)) { return $null }
    if (-not (Test-Path -LiteralPath $ObservationPath -PathType Leaf)) {
        $Blockers.Add((New-InstallerBlocker 'binding_observation_missing' 'The explicit binding observation file is missing.' 'Capture the current 0580 binding and pass the resulting JSON file.'))
        return $null
    }
    try { $observation = Get-Content -LiteralPath $ObservationPath -Raw | ConvertFrom-Json }
    catch {
        $Blockers.Add((New-InstallerBlocker 'binding_observation_invalid' $_.Exception.Message 'Provide valid inspection JSON; do not infer a driver package name.'))
        return $null
    }
    $properties = @($observation.PSObject.Properties.Name)
    $Blockers.Add((New-InstallerBlocker 'external_binding_observation_untrusted' 'External binding JSON is diagnostic-only and cannot authorize driver removal.' 'Run live PnP inspection against the current devnode and installed OEM INF.'))
    [ordered]@{
        hardware_id = if ($properties -contains 'hardware_id') { [string]$observation.hardware_id } else { $null }
        published_name = if ($properties -contains 'published_name') { [string]$observation.published_name } else { $null }
        provider = if ($properties -contains 'provider') { [string]$observation.provider } else { $null }
        source = 'external_json'
        authoritative = $false
        test_only = $true
    }
}

function Get-LiveBootBindingObservation {
    param([Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Blockers)
    $getPnpDevice = Get-Command Get-PnpDevice -ErrorAction SilentlyContinue
    $getPnpProperty = Get-Command Get-PnpDeviceProperty -ErrorAction SilentlyContinue
    if (-not $getPnpDevice -or -not $getPnpProperty) {
        $Blockers.Add((New-InstallerBlocker 'binding_inspection_unavailable' 'PnP cmdlets required for authoritative binding inspection are unavailable.' 'Run the planner in elevated PowerShell 7 on the target Windows host.'))
        return $null
    }
    try {
        $devices = @(Get-PnpDevice -ErrorAction Stop | Where-Object { $_.InstanceId -like "$($script:HardwareId)\*" })
    }
    catch {
        $Blockers.Add((New-InstallerBlocker 'binding_inspection_failed' $_.Exception.Message 'Run the planner with permission to inspect live PnP state. External JSON cannot substitute for this check.'))
        return $null
    }
    if (-not $devices.Count) { return $null }

    $observations = [System.Collections.Generic.List[object]]::new()
    $infRoot = Join-Path ([Environment]::GetFolderPath('Windows')) 'INF'
    foreach ($device in $devices) {
        $properties = @{}
        try {
            foreach ($property in @(Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName @(
                            'DEVPKEY_Device_DriverInfPath',
                            'DEVPKEY_Device_DriverProvider',
                            'DEVPKEY_Device_Service'
                        ) -ErrorAction Stop)) {
                $properties[[string]$property.KeyName] = [string]$property.Data
            }
        }
        catch {
            $Blockers.Add((New-InstallerBlocker 'binding_property_inspection_failed' $_.Exception.Message 'Read DriverInfPath, provider and service directly from every target devnode.'))
            return $null
        }
        $driverInfPath = [string]$properties.DEVPKEY_Device_DriverInfPath
        if ($driverInfPath -notmatch '^oem\d+\.inf$') {
            $Blockers.Add((New-InstallerBlocker 'live_binding_inf_path_invalid' 'The live target devnode does not expose an exact oem#.inf DriverInfPath.' 'Do not migrate a binding whose installed package identity is ambiguous.'))
            return $null
        }
        $publishedName = $driverInfPath.ToLowerInvariant()
        $infPath = Get-CanonicalPath (Join-Path $infRoot $publishedName)
        if (-not (Test-PathInsideRoot $infRoot $infPath) -or -not (Test-Path -LiteralPath $infPath -PathType Leaf)) {
            $Blockers.Add((New-InstallerBlocker 'live_binding_oem_inf_missing' "The exact live OEM INF is unavailable: $publishedName" 'Repair driver-store integrity before migration.'))
            return $null
        }
        try { $analysis = Get-InfAnalysis $infPath }
        catch {
            $Blockers.Add((New-InstallerBlocker 'live_binding_oem_inf_invalid' $_.Exception.Message 'Only a structurally verified OEM INF may be migrated.'))
            return $null
        }
        if ($analysis.hardware_ids.Count -ne 1 -or $analysis.hardware_ids[0] -cne $script:HardwareId -or
            $analysis.declared_hardware_ids.Count -ne 1 -or $analysis.declared_hardware_ids[0] -cne $script:HardwareId) {
            $Blockers.Add((New-InstallerBlocker 'live_binding_scope_invalid' "OEM INF $publishedName contains a model other than the exact 0580 HardwareId." 'Do not remove a shared or 058C-capable driver package.'))
            return $null
        }
        $provider = [string]$properties.DEVPKEY_Device_DriverProvider
        $temporary = $provider -match '(?i)libwdi|zadig'
        $observations.Add([ordered]@{
                hardware_id = $script:HardwareId
                instance_id = [string]$device.InstanceId
                published_name = $publishedName
                provider = $provider
                service = [string]$properties.DEVPKEY_Device_Service
                temporary = $temporary
                inf_path = $infPath
                inf_sha256 = (Get-FileHash -LiteralPath $infPath -Algorithm SHA256).Hash.ToLowerInvariant()
                source = 'live_pnp_and_oem_inf'
                authoritative = $true
                test_only = $false
            })
    }
    $uniqueNames = @($observations.published_name | Sort-Object -Unique)
    if ($uniqueNames.Count -ne 1) {
        $Blockers.Add((New-InstallerBlocker 'live_binding_ambiguous' 'Target devnodes use more than one OEM INF.' 'Resolve the live binding ambiguity before migration.'))
        return $null
    }
    $observations[0]
}

function Read-InstallerState {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $ExpectedInstallRoot,
        [Parameter(Mandatory)][string] $ExpectedStateRoot,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Blockers
    )
    $expectedStatePath = Get-CanonicalPath (Join-Path $ExpectedStateRoot 'installer-state.json')
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals((Get-CanonicalPath $Path), $expectedStatePath)) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_path_unsafe' 'Installer state must use the exact owned ProgramData path.' 'Use the fixed PS5 Camera installer-state.json path.'))
        return $null
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_missing' 'The owned installer state is missing.' 'Supply the state recorded by a successful installation; never guess an oem#.inf name.'))
        return $null
    }
    try { $state = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json }
    catch {
        $Blockers.Add((New-InstallerBlocker 'installer_state_invalid' $_.Exception.Message 'Recover a valid transaction state before changing the system.'))
        return $null
    }
    $requiredProperties = @('schema_version', 'service_name', 'hardware_id', 'install_root', 'driver_state_path', 'previous_published_name', 'rollback_snapshot', 'status')
    $allowedProperties = @($requiredProperties) + @('release_root', 'release_version')
    if (@($requiredProperties | Where-Object { $_ -notin $state.PSObject.Properties.Name }).Count) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_structure_invalid' 'Installer state is missing required ownership or rollback fields.' 'Recover a complete state emitted by this installer schema.'))
        return $null
    }
    if (@($state.PSObject.Properties.Name | Where-Object { $_ -notin $allowedProperties }).Count) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_structure_invalid' 'Installer state contains unrecognized fields.' 'Use only the exact versioned state structure emitted by this installer.'))
        return $null
    }
    if ($state.schema_version -ne $script:SchemaVersion -or $state.service_name -cne $script:ServiceName -or $state.hardware_id -cne $script:HardwareId) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_ownership_mismatch' 'The state does not belong to this installer, service, and HardwareId.' 'Do not use foreign state; recover the matching PS5 Camera state.'))
        return $null
    }
    if ([string]$state.status -notin $script:AllowedStateStatuses) {
        $Blockers.Add((New-InstallerBlocker 'installer_state_status_invalid' "Installer state has unsupported status: $($state.status)" "Use one of the versioned states: $($script:AllowedStateStatuses -join ', ')."))
        return $null
    }
    try {
        $storedInstallRoot = Get-CanonicalPath ([string]$state.install_root)
        $storedDriverState = Get-CanonicalPath ([string]$state.driver_state_path)
    }
    catch {
        $Blockers.Add((New-InstallerBlocker 'installer_state_path_invalid' $_.Exception.Message 'State paths must be non-empty canonical paths owned by this installer.'))
        return $null
    }
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals($storedInstallRoot, (Get-CanonicalPath $ExpectedInstallRoot))) {
        $Blockers.Add((New-InstallerBlocker 'install_root_ownership_mismatch' 'Stored install_root is not the exact owned Program Files directory.' 'Do not remove or restore files outside the fixed PS5 Camera install root.'))
        return $null
    }
    $expectedDriverState = Get-CanonicalPath (Join-Path $ExpectedStateRoot 'driver-package-state.json')
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals($storedDriverState, $expectedDriverState) -or
        -not (Test-PathInsideRoot $ExpectedStateRoot $storedDriverState)) {
        $Blockers.Add((New-InstallerBlocker 'driver_state_path_unsafe' 'The stored package-pipeline state escapes the owned state directory.' 'Recover a state file created by this installer.'))
        return $null
    }
    $previousName = $state.previous_published_name
    if ($null -ne $previousName -and [string]$previousName -notmatch '^oem\d+\.inf$') {
        $Blockers.Add((New-InstallerBlocker 'previous_published_name_invalid' 'Stored previous_published_name is neither null nor an exact oem#.inf name.' 'Never infer or normalize a foreign driver package name.'))
        return $null
    }
    $snapshot = $state.rollback_snapshot
    if ($null -ne $snapshot) {
        $snapshotProperties = @($snapshot.PSObject.Properties.Name)
        if ($snapshotProperties.Count -ne 3 -or
            @(@('schema_version', 'kind', 'path') | Where-Object { $_ -notin $snapshotProperties }).Count -gt 0 -or
            $snapshot.schema_version -ne 1 -or $snapshot.kind -cne 'ps5camera-installer-rollback') {
            $Blockers.Add((New-InstallerBlocker 'rollback_snapshot_structure_invalid' 'Rollback snapshot metadata has an unexpected structure.' 'Use only a snapshot journal emitted by this installer.'))
            return $null
        }
        try { $snapshotPath = Get-CanonicalPath ([string]$snapshot.path) }
        catch {
            $Blockers.Add((New-InstallerBlocker 'rollback_snapshot_path_invalid' $_.Exception.Message 'Rollback snapshot paths must be canonical children of the fixed rollback root.'))
            return $null
        }
        $rollbackRoot = Get-CanonicalPath (Join-Path $ExpectedStateRoot 'rollback')
        if (-not (Test-PathInsideRoot $rollbackRoot $snapshotPath)) {
            $Blockers.Add((New-InstallerBlocker 'rollback_snapshot_path_unsafe' 'Rollback snapshot escapes the fixed rollback root.' 'Discard foreign state and recover the owned transaction journal.'))
            return $null
        }
    }
    $state
}

function Add-InstallSteps {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Steps,
        [Parameter(Mandatory)][System.Collections.IDictionary] $Artifacts,
        [Parameter(Mandatory)][string] $ReleaseRoot,
        [Parameter(Mandatory)][string] $InstallRoot,
        [Parameter(Mandatory)][string] $StateRoot,
        [Parameter(Mandatory)][string] $StatePath,
        [Parameter(Mandatory)][string] $PackagePipeline,
        [Parameter()][AllowNull()][object] $TemporaryBinding,
        [Parameter()][AllowNull()][object] $RollbackSnapshot = $null
    )
    $driverState = Join-Path $StateRoot 'driver-package-state.json'
    $bindingBackup = Join-Path $StateRoot 'temporary-binding-backup'
    $Steps.Add((New-InstallerStep 'prepare-state-root' 'ensure_directory' $true @{ path = $StateRoot; acl = 'SYSTEM:F,Administrators:F' } @{ operation = 'remove_directory_if_empty'; path = $StateRoot }))
    $Steps.Add((New-InstallerStep 'prepare-install-root' 'ensure_directory' $true @{ path = $InstallRoot; acl = 'SYSTEM:F,Administrators:F,Users:RX' } @{ operation = 'remove_directory'; path = $InstallRoot }))
    foreach ($artifact in ($Artifacts.Values | Sort-Object role)) {
        if ($artifact.role -in @('driver_inf', 'signed_catalog', 'installer')) { continue }
        $destination = Join-Path $InstallRoot $artifact.file_name
        $Steps.Add((New-InstallerStep "copy-$($artifact.role)" 'copy_verified_file' $true @{
                    source = $artifact.path; destination = $destination; sha256 = $artifact.sha256
                } @{ operation = 'remove_file'; path = $destination }))
    }
    if ($TemporaryBinding) {
        $publishedName = ([string]$TemporaryBinding.published_name).ToLowerInvariant()
        $Steps.Add((New-InstallerStep 'export-temporary-binding' 'export_driver_package' $true @{
                    hardware_id = $script:HardwareId; published_name = $publishedName; destination = $bindingBackup
                } @{ operation = 'remove_directory'; path = $bindingBackup }))
        $Steps.Add((New-InstallerStep 'remove-temporary-binding' 'remove_exact_driver_package' $true @{
                    hardware_id = $script:HardwareId; published_name = $publishedName
                } @{ operation = 'restore_exported_driver_package'; source = $bindingBackup; expected_previous_published_name = $publishedName; hardware_id = $script:HardwareId }))
    }
    $Steps.Add((New-InstallerStep 'install-boot-driver' 'package_pipeline' $true @{
                action = 'Install'; script = $PackagePipeline; staging_directory = $ReleaseRoot; state_path = $driverState; confirm_hardware_id = $script:HardwareId
            } @{ operation = 'package_pipeline'; action = 'Rollback'; script = $PackagePipeline; state_path = $driverState; confirm_hardware_id = $script:HardwareId }))
    # ReportEventW uses the structured insertion string directly, so the
    # service executable is the single Event Log message resource. Keeping it
    # as the same verified artifact avoids a second, duplicate PE payload.
    $messageDestination = Join-Path $InstallRoot $Artifacts.windows_service.file_name
    $Steps.Add((New-InstallerStep 'register-event-source' 'register_event_source' $true @{
                source = $script:EventSource; message_file = $messageDestination; types_supported = 7
            } @{ operation = 'remove_event_source'; source = $script:EventSource }))
    $serviceDestination = Join-Path $InstallRoot $Artifacts.windows_service.file_name
    $Steps.Add((New-InstallerStep 'create-service' 'create_service' $true @{
                name = $script:ServiceName; binary_path = $serviceDestination; start = 'auto'; account = 'LocalSystem'
            } @{ operation = 'delete_service'; name = $script:ServiceName }))
    $Steps.Add((New-InstallerStep 'start-service' 'start_service' $true @{ name = $script:ServiceName } @{ operation = 'stop_service'; name = $script:ServiceName }))
    $Steps.Add((New-InstallerStep 'commit-state' 'write_installer_state' $true @{
                path = $StatePath
                document = [ordered]@{
                    schema_version = $script:SchemaVersion
                    service_name = $script:ServiceName
                    hardware_id = $script:HardwareId
                    install_root = $InstallRoot
                    driver_state_path = $driverState
                    previous_published_name = if ($TemporaryBinding) { ([string]$TemporaryBinding.published_name).ToLowerInvariant() } else { $null }
                    rollback_snapshot = $RollbackSnapshot
                    release_root = $ReleaseRoot
                    status = if ($null -eq $RollbackSnapshot) { 'installed' } else { 'rollback_available' }
                }
            } @{ operation = 'remove_file'; path = $StatePath }))
}

function Add-UninstallSteps {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]] $Steps,
        [Parameter(Mandatory)][object] $State,
        [Parameter(Mandatory)][string] $StatePath,
        [Parameter(Mandatory)][string] $PackagePipeline
    )
    $Steps.Add((New-InstallerStep 'stop-service' 'stop_service' $true @{ name = $script:ServiceName } @{ operation = 'start_service'; name = $script:ServiceName }))
    $Steps.Add((New-InstallerStep 'delete-service' 'delete_service' $true @{ name = $script:ServiceName } @{ operation = 'restore_service_from_state'; state_path = $StatePath }))
    $Steps.Add((New-InstallerStep 'remove-event-source' 'remove_event_source' $true @{ source = $script:EventSource } @{ operation = 'restore_event_source_from_state'; state_path = $StatePath }))
    $Steps.Add((New-InstallerStep 'uninstall-boot-driver' 'package_pipeline' $true @{
                action = 'Uninstall'; script = $PackagePipeline; state_path = [string]$State.driver_state_path; confirm_hardware_id = $script:HardwareId
            } @{ operation = 'restore_release_driver_from_state'; state_path = $StatePath }))
    if ($State.previous_published_name) {
        $Steps.Add((New-InstallerStep 'restore-previous-binding' 'restore_exported_driver_package' $true @{
                    source = (Join-Path (Split-Path -Parent $StatePath) 'temporary-binding-backup'); expected_previous_published_name = ([string]$State.previous_published_name).ToLowerInvariant(); hardware_id = $script:HardwareId
                } @{ operation = 'remove_exact_restored_driver_from_journal'; state_path = $StatePath }))
    }
    $Steps.Add((New-InstallerStep 'quarantine-install-root' 'move_to_transaction_backup' $true @{ path = [string]$State.install_root } @{ operation = 'restore_from_transaction_backup'; path = [string]$State.install_root }))
    $Steps.Add((New-InstallerStep 'remove-installer-state' 'remove_file_after_commit' $true @{ path = $StatePath } @{ operation = 'restore_file_from_transaction_backup'; path = $StatePath }))
}

function Test-RollbackInvariant {
    param([Parameter(Mandatory)][object] $Step)
    if ($Step.mutates_system -ne $true) { return $true }
    $rollback = $Step.rollback
    if ($null -eq $rollback -or
        ($rollback -isnot [Collections.IDictionary] -and $rollback -isnot [pscustomobject])) { return $false }
    $operation = if ($rollback -is [Collections.IDictionary]) {
        if (-not $rollback.Contains('operation')) { return $false }
        [string]$rollback['operation']
    }
    else {
        if ($rollback.PSObject.Properties.Name -notcontains 'operation') { return $false }
        [string]$rollback.operation
    }
    -not [string]::IsNullOrWhiteSpace($operation)
}

function New-Ps5CameraInstallerPlan {
    [CmdletBinding()]
    param(
        [Parameter()][ValidateSet('Install', 'Repair', 'Uninstall', 'Rollback')][string] $Action = 'Install',
        [Parameter()][string] $ReleaseManifest,
        [Parameter()][string] $BindingObservationPath,
        [Parameter()][string] $ConfirmTemporaryPublishedName,
        [Parameter()][string] $ConfirmReleaseVersion,
        [Parameter()][switch] $Execute,
        [Parameter()][switch] $SkipLiveBindingInspection,
        [Parameter(Mandatory)][string] $ProgramFilesRoot,
        [Parameter(Mandatory)][string] $ProgramDataRoot,
        [Parameter(Mandatory)][string] $PackagePipeline
    )
    $blockers = [System.Collections.Generic.List[object]]::new()
    $steps = [System.Collections.Generic.List[object]]::new()
    $installRoot = Get-CanonicalPath (Join-Path $ProgramFilesRoot $script:InstallDirectoryName)
    $stateRoot = Get-CanonicalPath (Join-Path $ProgramDataRoot $script:InstallDirectoryName)
    $statePath = Join-Path $stateRoot 'installer-state.json'
    if (-not (Test-PathInsideRoot $ProgramFilesRoot $installRoot) -or -not (Test-PathInsideRoot $ProgramDataRoot $stateRoot)) {
        $blockers.Add((New-InstallerBlocker 'unsafe_owned_path' 'Installer paths must remain under Program Files and ProgramData.' 'Use the fixed product roots selected by the coordinator.'))
    }
    if (-not (Test-Path -LiteralPath $PackagePipeline -PathType Leaf)) {
        $blockers.Add((New-InstallerBlocker 'package_pipeline_missing' 'The reviewed package-pipeline.ps1 was not found.' 'Restore windows/package/package-pipeline.ps1 from the same source revision.'))
    }

    $manifest = $null
    $releaseRoot = $null
    $releaseManifestSha256 = $null
    $artifacts = [ordered]@{}
    $blockers.Add((New-InstallerBlocker 'release_authenticity_format_undefined' 'No reviewed cryptographic signature format exists yet for release-manifest.json or installed-state provenance.' 'Define a detached or enveloped release-signature format, trust policy and verification tool; self-asserted JSON can never authorize LocalSystem changes.'))
    $blockers.Add((New-InstallerBlocker 'safe_staging_not_implemented' 'The planner does not yet create and lock a private same-volume staging directory.' 'Implement owned staging with restrictive ACLs and atomic promotion before enabling execution.'))
    $blockers.Add((New-InstallerBlocker 'reparse_point_defense_not_implemented' 'Reparse-point checks are not yet enforced for every release, state, staging and destination path.' 'Open paths without following untrusted reparse points and revalidate every ancestor before mutation.'))
    $blockers.Add((New-InstallerBlocker 'artifact_toctou_defense_not_implemented' 'Verified artifacts are not yet held by stable handles through copy and execution.' 'Verify and consume each artifact through the same non-reparse handle or equivalent atomic mechanism.'))
    if ($Action -in @('Repair', 'Uninstall', 'Rollback')) {
        $blockers.Add((New-InstallerBlocker 'authenticated_state_format_undefined' 'Installed-state and rollback journals do not yet have a cryptographically authenticated format.' 'Define state signing or machine-bound authentication; recovery must trust authenticated installed state, never a newly supplied release.'))
    }
    if ($Action -in @('Install', 'Repair')) {
        if ([string]::IsNullOrWhiteSpace($ReleaseManifest) -or -not (Test-Path -LiteralPath $ReleaseManifest -PathType Leaf)) {
            $blockers.Add((New-InstallerBlocker 'release_manifest_required' 'Install and Repair require an existing release-manifest.json.' 'Pass the manifest from a real assembled release.'))
        }
        else {
            try {
                $resolvedManifest = (Resolve-Path -LiteralPath $ReleaseManifest).Path
                $releaseRoot = Split-Path -Parent $resolvedManifest
                $releaseManifestSha256 = (Get-FileHash -LiteralPath $resolvedManifest -Algorithm SHA256).Hash.ToLowerInvariant()
                $manifest = Get-Content -LiteralPath $resolvedManifest -Raw | ConvertFrom-Json
                if ($manifest.schema_version -ne 1 -or [string]$manifest.release_version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
                    $blockers.Add((New-InstallerBlocker 'release_manifest_invalid' 'Unsupported release manifest schema or version.' 'Use release-manifest.json emitted by the reviewed assembler.'))
                }
                $hardwareIds = @($manifest.hardware_ids)
                if ($hardwareIds.Count -ne 1 -or [string]$hardwareIds[0] -cne $script:HardwareId) {
                    $blockers.Add((New-InstallerBlocker 'release_hardware_scope_invalid' 'The release must contain exactly the 0580 boot HardwareId.' 'Never include the 058C UVC device in this installer.'))
                }
                $artifacts = Get-ReleaseArtifactMap $manifest $releaseRoot $blockers
                foreach ($role in $script:RequiredReleaseRoles) {
                    if (-not $artifacts.Contains($role)) {
                        $blockers.Add((New-InstallerBlocker 'required_release_artifact_missing' "Required verified artifact is missing: $role" 'Build and assemble the real artifact; do not substitute a placeholder.'))
                    }
                }
                if ($artifacts.Contains('driver_inf') -and -not (Test-InfScope $artifacts.driver_inf.path)) {
                    $blockers.Add((New-InstallerBlocker 'driver_inf_scope_invalid' 'The INF is not a catalog-backed WinUSB package restricted to the 0580 boot PID.' 'Use the reviewed package pipeline output.'))
                }
                if ($artifacts.Contains('driver_inf') -and $artifacts.driver_inf.file_name -cne 'ps5cam-boot.inf') {
                    $blockers.Add((New-InstallerBlocker 'driver_inf_name_invalid' 'The package pipeline requires the reviewed INF name ps5cam-boot.inf.' 'Regenerate the release without renaming driver-package files.'))
                }
                if ($artifacts.Contains('signed_catalog') -and $artifacts.signed_catalog.file_name -cne 'ps5cam-boot.cat') {
                    $blockers.Add((New-InstallerBlocker 'catalog_name_invalid' 'The package pipeline requires the reviewed catalog name ps5cam-boot.cat.' 'Regenerate the release without renaming driver-package files.'))
                }
                if ($artifacts.Contains('windows_service') -and [IO.Path]::GetExtension($artifacts.windows_service.file_name) -ine '.exe') {
                    $blockers.Add((New-InstallerBlocker 'service_binary_type_invalid' 'The Windows service artifact is not an .exe.' 'Supply the real Windows service executable.'))
                }
                if ($artifacts.Contains('signed_catalog') -and $artifacts.Contains('driver_inf')) {
                    $null = Test-CatalogTrust $artifacts.signed_catalog.path $artifacts.driver_inf.path $blockers
                }
                if ($manifest.PSObject.Properties.Name -notcontains 'sbom' -or
                    [string]$manifest.sbom.file_name -cne 'sbom.cdx.json' -or
                    [string]$manifest.sbom.sha256 -notmatch '^[0-9a-fA-F]{64}$') {
                    $blockers.Add((New-InstallerBlocker 'sbom_metadata_missing' 'The release manifest lacks the deterministic SBOM reference.' 'Use the complete output of release-assembler.ps1.'))
                }
                else {
                    $sbomPath = Get-CanonicalPath (Join-Path $releaseRoot ([string]$manifest.sbom.file_name))
                    if (-not (Test-PathInsideRoot $releaseRoot $sbomPath) -or -not (Test-Path -LiteralPath $sbomPath -PathType Leaf) -or
                        (Get-FileHash -LiteralPath $sbomPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne ([string]$manifest.sbom.sha256).ToLowerInvariant()) {
                        $blockers.Add((New-InstallerBlocker 'sbom_integrity_failed' 'The release SBOM is missing or does not match its declared hash.' 'Discard the release and obtain a complete assembled directory.'))
                    }
                }
                $manifestProperties = @($manifest.PSObject.Properties.Name)
                $authorization = if ($manifestProperties -contains 'firmware_authorization') { $manifest.firmware_authorization } else { $null }
                $isCleanRoomFirmware = $null -ne $authorization -and $authorization.clean_room -eq $true
                $isPinnedMitReference = $null -ne $authorization -and $authorization.clean_room -eq $false -and
                    $authorization.redistribution_basis -ceq 'third_party_mit_reference' -and
                    $authorization.license -ceq 'MIT' -and
                    $authorization.source_commit -ceq '8773610978d5a4d91a6a6d8063d48a4f3afcfe5b' -and
                    $authorization.notice_file -ceq $script:PinnedReferenceNoticeFileName
                if ($null -eq $authorization -or $authorization.status -cne 'approved' -or $authorization.redistribution_allowed -ne $true -or [string]::IsNullOrWhiteSpace([string]$authorization.approval_reference) -or (-not $isCleanRoomFirmware -and -not $isPinnedMitReference)) {
                    $blockers.Add((New-InstallerBlocker 'firmware_authorization_evidence_missing' 'The release manifest lacks approved clean-room authorization or the pinned MIT reference provenance.' 'Use clean-room authorization or the reviewed V1 MIT reference metadata; never infer authorization from a file name.'))
                }
                elseif ($isPinnedMitReference -and (
                        $artifacts.authorized_firmware.file_name -cne $script:PinnedReferenceFirmwareFileName -or
                        $artifacts.authorized_firmware.sha256 -cne $script:PinnedReferenceFirmwareSha256 -or
                        $artifacts.license.file_name -cne $script:PinnedReferenceNoticeFileName)) {
                    $blockers.Add((New-InstallerBlocker 'pinned_reference_artifact_mismatch' 'The release metadata claims the V1 MIT reference but its verified firmware or notice artifact is not the pinned reviewed file.' 'Use the exact firmware SHA-256 and MIT notice emitted by the reviewed release assembler.'))
                }
                if ($ConfirmReleaseVersion -cne [string]$manifest.release_version) {
                    $blockers.Add((New-InstallerBlocker 'release_confirmation_required' "Exact confirmation of release version $($manifest.release_version) is required." "Pass -ConfirmReleaseVersion '$($manifest.release_version)'."))
                }
            }
            catch {
                $blockers.Add((New-InstallerBlocker 'release_manifest_parse_failed' $_.Exception.Message 'Use a valid release-manifest.json and complete release directory.'))
            }
        }
    }

    $binding = $null
    if ($Action -in @('Install', 'Repair')) {
        if (-not [string]::IsNullOrWhiteSpace($BindingObservationPath)) {
            $binding = Read-BindingObservation $BindingObservationPath $blockers
        }
        elseif ($SkipLiveBindingInspection) {
            $binding = [ordered]@{ source = 'skipped_live_inspection_test_seam'; authoritative = $false; test_only = $true }
            $blockers.Add((New-InstallerBlocker 'test_only_binding_inspection_skipped' 'Live binding inspection was skipped through the explicit test seam.' 'Never use -SkipLiveBindingInspection for an operational plan.'))
        }
        else {
            $binding = Get-LiveBootBindingObservation $blockers
        }
    }
    $temporaryBinding = $null
    if ($binding -and $binding.authoritative -eq $true -and $binding.test_only -ne $true) {
        $provider = [string]$binding.provider
        $temporary = $binding.temporary -eq $true -or $provider -match '(?i)libwdi|zadig'
        if ($temporary) {
            $temporaryBinding = $binding
            $publishedName = ([string]$binding.published_name).ToLowerInvariant()
            if ($ConfirmTemporaryPublishedName -cne $publishedName) {
                $blockers.Add((New-InstallerBlocker 'temporary_binding_confirmation_required' "Temporary libwdi/Zadig binding detected for 0580: $publishedName" "Confirm exactly '$publishedName'; no other oem#.inf is inferred or removed."))
            }
        }
    }

    $state = $null
    if ($Action -in @('Repair', 'Uninstall', 'Rollback')) {
        $state = Read-InstallerState $statePath $installRoot $stateRoot $blockers
        if ($state -and $Action -in @('Repair', 'Uninstall') -and [string]$state.status -notin @('installed', 'rollback_available')) {
            $blockers.Add((New-InstallerBlocker 'installer_state_status_incompatible' "Action $Action cannot start from state status $($state.status)." 'Recover or complete the recorded transaction before proceeding.'))
            $state = $null
        }
        if ($state -and $Action -eq 'Rollback' -and [string]$state.status -cne 'rollback_available') {
            $blockers.Add((New-InstallerBlocker 'installer_state_status_incompatible' "Rollback requires rollback_available state, found $($state.status)." 'Use only an authenticated state that records an available snapshot.'))
            $state = $null
        }
    }
    $hasExactArtifacts = @($script:RequiredReleaseRoles | Where-Object { -not $artifacts.Contains($_) }).Count -eq 0 -and
        $artifacts.Count -eq $script:RequiredReleaseRoles.Count
    switch ($Action) {
        'Install' {
            if ($hasExactArtifacts) {
                Add-InstallSteps $steps $artifacts $releaseRoot $installRoot $stateRoot $statePath $PackagePipeline $temporaryBinding
            }
        }
        'Repair' {
            if ($state -and $hasExactArtifacts) {
                $rollbackSnapshot = [ordered]@{
                    schema_version = 1
                    kind = 'ps5camera-installer-rollback'
                    path = Join-Path $stateRoot "rollback\repair-$($releaseManifestSha256.Substring(0, 16))"
                }
                $steps.Add((New-InstallerStep 'snapshot-installed-state' 'create_transaction_backup' $true @{ state_path = $statePath; install_root = $installRoot; snapshot = $rollbackSnapshot } @{ operation = 'restore_transaction_backup'; state_path = $statePath; snapshot = $rollbackSnapshot.path }))
                $steps.Add((New-InstallerStep 'stop-service' 'stop_service' $true @{ name = $script:ServiceName } @{ operation = 'start_service'; name = $script:ServiceName }))
                Add-InstallSteps $steps $artifacts $releaseRoot $installRoot $stateRoot $statePath $PackagePipeline $temporaryBinding $rollbackSnapshot
            }
        }
        'Uninstall' {
            if ($state) { Add-UninstallSteps $steps $state $statePath $PackagePipeline }
        }
        'Rollback' {
            if ($state) {
                if (-not $state.rollback_snapshot) {
                    $blockers.Add((New-InstallerBlocker 'rollback_snapshot_missing' 'No committed rollback snapshot exists in installer state.' 'Rollback only from a transaction snapshot recorded by this installer.'))
                }
                else {
                    $steps.Add((New-InstallerStep 'rollback-owned-transaction' 'restore_transaction_backup' $true @{ state_path = $statePath; snapshot = [string]$state.rollback_snapshot.path } @{ operation = 'reapply_current_transaction'; state_path = $statePath }))
                }
            }
        }
    }

    foreach ($step in $steps) {
        if (-not (Test-RollbackInvariant $step)) {
            $blockers.Add((New-InstallerBlocker 'non_transactional_step' "Mutating step has no rollback object with a non-empty operation: $($step.id)" 'Every mutation must carry an explicit structured inverse before execution is enabled.'))
        }
    }
    if ($Execute) {
        if (-not $IsWindows) {
            $blockers.Add((New-InstallerBlocker 'windows_required' 'Execution is supported only on Windows.' 'Run the dry-run elsewhere, then execute on a reviewed Windows host.'))
        }
        else {
            $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
            $principal = [Security.Principal.WindowsPrincipal]::new($identity)
            if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
                $blockers.Add((New-InstallerBlocker 'elevation_required' 'Execution requires an elevated administrator token.' 'Rerun in an elevated PowerShell after reviewing the dry-run.'))
            }
        }
    }

    [ordered]@{
        schema_version = $script:SchemaVersion
        status = if ($blockers.Count) { 'blocked' } else { 'ready' }
        mode = if ($Execute) { 'execute' } else { 'dry_run' }
        action = $Action.ToLowerInvariant()
        hardware_id = $script:HardwareId
        protected_hardware_id = $script:ForbiddenHardwareId
        release_version = if ($manifest) { [string]$manifest.release_version } else { $null }
        release_root = $releaseRoot
        install_root = $installRoot
        state_path = $statePath
        binding_evidence = if ($binding) { [ordered]@{ source = [string]$binding.source; authoritative = $binding.authoritative -eq $true; test_only = $binding.test_only -eq $true } } else { $null }
        temporary_binding = if ($temporaryBinding) { [ordered]@{ provider = [string]$temporaryBinding.provider; published_name = ([string]$temporaryBinding.published_name).ToLowerInvariant(); inf_sha256 = [string]$temporaryBinding.inf_sha256 } } else { $null }
        blockers = @($blockers)
        steps = @($steps)
    }
}

Export-ModuleMember -Function New-Ps5CameraInstallerPlan, Test-PathInsideRoot, Test-InfScope

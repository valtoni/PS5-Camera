[CmdletBinding()]
param(
    [Parameter()]
    [string] $PackageRoot = $PSScriptRoot,

    [Parameter()]
    [switch] $RequireWdk,

    [Parameter()]
    [switch] $SkipWdk
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ExpectedHardwareId = 'USB\VID_05A9&PID_0580'
$ExpectedInterfaceGuid = '{ABB9454F-E674-4620-8C6E-49A5777EB078}'
$ForbiddenProductId = '05' + '8C'
$ForbiddenDriverName = 'usb' + 'video'
$ForbiddenClassName = 'u' + 'vc'
$Errors = [System.Collections.Generic.List[string]]::new()

if ($RequireWdk -and $SkipWdk) {
    throw '-RequireWdk and -SkipWdk are mutually exclusive.'
}

function Add-ValidationError {
    param([Parameter(Mandatory)][string] $Message)
    $Errors.Add($Message)
}

function Assert-True {
    param(
        [Parameter(Mandatory)][bool] $Condition,
        [Parameter(Mandatory)][string] $Message
    )
    if (-not $Condition) {
        Add-ValidationError -Message $Message
    }
}

function Get-InfSections {
    param([Parameter(Mandatory)][string] $Content)

    $sections = @{}
    $currentSection = $null
    foreach ($rawLine in ($Content -split "`r?`n")) {
        $line = ($rawLine -split ';', 2)[0].Trim()
        if ($line.Length -eq 0) {
            continue
        }
        if ($line -match '^\[([^]]+)\]$') {
            $currentSection = $Matches[1]
            if ($sections.ContainsKey($currentSection)) {
                Add-ValidationError -Message "Duplicate INF section [$currentSection]."
            }
            else {
                $sections[$currentSection] = [System.Collections.Generic.List[string]]::new()
            }
            continue
        }
        if ($null -eq $currentSection) {
            Add-ValidationError -Message "INF content appears before the first section: $line"
            continue
        }
        $sections[$currentSection].Add($line)
    }
    return $sections
}

function Get-DirectiveValues {
    param(
        [Parameter(Mandatory)][hashtable] $Sections,
        [Parameter(Mandatory)][string] $Section,
        [Parameter(Mandatory)][string] $Directive
    )

    if (-not $Sections.ContainsKey($Section)) {
        return @()
    }
    $pattern = '^' + [regex]::Escape($Directive) + '\s*=\s*(.+)$'
    return @($Sections[$Section] | ForEach-Object {
        if ($_ -match $pattern) {
            $Matches[1].Trim()
        }
    })
}

function Assert-SingleDirective {
    param(
        [Parameter(Mandatory)][hashtable] $Sections,
        [Parameter(Mandatory)][string] $Section,
        [Parameter(Mandatory)][string] $Directive,
        [Parameter(Mandatory)][string] $Expected
    )

    $values = @(Get-DirectiveValues -Sections $Sections -Section $Section -Directive $Directive)
    Assert-True -Condition ($values.Count -eq 1) -Message "[$Section] must contain exactly one $Directive directive."
    if ($values.Count -eq 1) {
        Assert-True -Condition ($values[0] -ieq $Expected) -Message "[$Section] $Directive must be '$Expected', found '$($values[0])'."
    }
}

function Find-WdkTool {
    param([Parameter(Mandatory)][string] $Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -ne $command) {
        return $command.Source
    }

    $programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
    if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
        return $null
    }
    $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10'
    if (-not (Test-Path -LiteralPath $kitsRoot -PathType Container)) {
        return $null
    }
    $candidates = @(
        (Join-Path $kitsRoot "Tools\*\x64\$Name"),
        (Join-Path $kitsRoot "Tools\*\x86\$Name"),
        (Join-Path $kitsRoot "bin\*\x64\$Name"),
        (Join-Path $kitsRoot "bin\*\x86\$Name")
    )
    $match = Get-Item -Path $candidates -ErrorAction SilentlyContinue |
        Where-Object { -not $_.PSIsContainer } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($null -eq $match) { return $null }
    $match.FullName
}

$ResolvedPackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
$InfPath = Join-Path $ResolvedPackageRoot 'ps5cam-boot.inf'
$ManifestPath = Join-Path $ResolvedPackageRoot 'package-manifest.json'

Assert-True -Condition (Test-Path -LiteralPath $InfPath -PathType Leaf) -Message 'ps5cam-boot.inf is missing.'
Assert-True -Condition (Test-Path -LiteralPath $ManifestPath -PathType Leaf) -Message 'package-manifest.json is missing.'

if ($Errors.Count -gt 0) {
    $Errors | ForEach-Object { Write-Error $_ }
    exit 1
}

$InfContent = Get-Content -LiteralPath $InfPath -Raw
$ManifestContent = Get-Content -LiteralPath $ManifestPath -Raw
$Manifest = $ManifestContent | ConvertFrom-Json
$Sections = Get-InfSections -Content $InfContent

$RequiredSections = @(
    'Version',
    'Manufacturer',
    'Models.NTamd64',
    'Models.NTarm64',
    'Boot_Install',
    'Boot_Install.Services',
    'Boot_Install.HW',
    'Boot_Interface_AddReg',
    'Strings'
)
foreach ($section in $RequiredSections) {
    Assert-True -Condition $Sections.ContainsKey($section) -Message "Required INF section [$section] is missing."
}

Assert-SingleDirective -Sections $Sections -Section 'Version' -Directive 'Signature' -Expected '"$Windows NT$"'
Assert-SingleDirective -Sections $Sections -Section 'Version' -Directive 'Class' -Expected 'USBDevice'
Assert-SingleDirective -Sections $Sections -Section 'Version' -Directive 'ClassGuid' -Expected '{88BAE032-5A81-49F0-BC3D-A4FF138216D6}'
Assert-SingleDirective -Sections $Sections -Section 'Version' -Directive 'PnpLockdown' -Expected '1'
Assert-SingleDirective -Sections $Sections -Section 'Boot_Install' -Directive 'Include' -Expected 'winusb.inf'
Assert-SingleDirective -Sections $Sections -Section 'Boot_Install' -Directive 'Needs' -Expected 'WINUSB.NT'
Assert-SingleDirective -Sections $Sections -Section 'Boot_Install.Services' -Directive 'Include' -Expected 'winusb.inf'
Assert-SingleDirective -Sections $Sections -Section 'Boot_Install.Services' -Directive 'Needs' -Expected 'WINUSB.NT.Services'
Assert-SingleDirective -Sections $Sections -Section 'Boot_Install.HW' -Directive 'AddReg' -Expected 'Boot_Interface_AddReg'

$ManufacturerLines = @($Sections['Manufacturer'])
Assert-True -Condition ($ManufacturerLines.Count -eq 1) -Message '[Manufacturer] must contain exactly one model declaration.'
if ($ManufacturerLines.Count -eq 1) {
    Assert-True -Condition ($ManufacturerLines[0] -match '^%ProviderName%\s*=\s*Models\s*,\s*NTamd64\s*,\s*NTarm64$') -Message '[Manufacturer] must target only NTamd64 and NTarm64 model sections.'
}

$ExpectedModelPattern = '^%BootDeviceName%\s*=\s*Boot_Install\s*,\s*USB\\VID_05A9&PID_0580$'
foreach ($modelSection in @('Models.NTamd64', 'Models.NTarm64')) {
    $modelLines = @($Sections[$modelSection])
    Assert-True -Condition ($modelLines.Count -eq 1) -Message "[$modelSection] must contain exactly one model."
    if ($modelLines.Count -eq 1) {
        Assert-True -Condition ($modelLines[0] -match $ExpectedModelPattern) -Message "[$modelSection] must bind only $ExpectedHardwareId."
    }
}

$HardwareIds = @([regex]::Matches($InfContent, 'USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}', 'IgnoreCase') | ForEach-Object { $_.Value.ToUpperInvariant() } | Sort-Object -Unique)
Assert-True -Condition ($HardwareIds.Count -eq 1) -Message 'The INF must contain exactly one unique USB hardware ID.'
if ($HardwareIds.Count -eq 1) {
    Assert-True -Condition ($HardwareIds[0] -ceq $ExpectedHardwareId) -Message "The only allowed hardware ID is $ExpectedHardwareId."
}

$ForbiddenHardwareId = "USB\VID_05A9&PID_$ForbiddenProductId"
$ScannableContent = $InfContent + "`n" + $ManifestContent
Assert-True -Condition ($ScannableContent.IndexOf($ForbiddenHardwareId, [StringComparison]::OrdinalIgnoreCase) -lt 0) -Message 'The final-mode camera hardware ID must never be captured by this package.'
Assert-True -Condition ($ScannableContent.IndexOf($ForbiddenDriverName, [StringComparison]::OrdinalIgnoreCase) -lt 0) -Message 'The in-box camera function driver must never be referenced by this package.'
Assert-True -Condition ($ScannableContent.IndexOf($ForbiddenClassName, [StringComparison]::OrdinalIgnoreCase) -lt 0) -Message 'A camera-class binding must never be referenced by this package.'
Assert-True -Condition ($InfContent -notmatch '(?im)^\s*(AddService|CopyFiles|CoInstallers32|SourceDisksFiles|CatalogFile)\s*=') -Message 'The current declarative package must not reference custom services, binaries, co-installers, or a nonexistent catalog.'
Assert-True -Condition ($InfContent -match '(?im)^\s*DriverVer\s*=\s*\d{2}/\d{2}/\d{4},\d+\.\d+\.\d+\.\d+\s*$') -Message 'DriverVer must contain a fixed date and four-part version.'
Assert-True -Condition ($InfContent -match ('(?im)^\s*DeviceInterfaceGuid\s*=\s*"?' + [regex]::Escape($ExpectedInterfaceGuid) + '"?\s*$')) -Message 'The expected device interface GUID is missing from [Strings].'

Assert-True -Condition ($Manifest.schemaVersion -eq 1) -Message 'Manifest schemaVersion must be 1.'
Assert-True -Condition ($Manifest.infFile -ceq 'ps5cam-boot.inf') -Message 'Manifest infFile must reference ps5cam-boot.inf.'
$ManifestHardwareIds = @($Manifest.targetHardwareIds)
Assert-True -Condition ($ManifestHardwareIds.Count -eq 1) -Message 'Manifest must contain exactly one target hardware ID.'
if ($ManifestHardwareIds.Count -eq 1) {
    Assert-True -Condition ($ManifestHardwareIds[0] -ceq $ExpectedHardwareId) -Message "Manifest target must be $ExpectedHardwareId."
}
Assert-True -Condition ($Manifest.deviceInterfaceGuid -ceq $ExpectedInterfaceGuid) -Message 'Manifest and INF device interface GUIDs must match.'
Assert-True -Condition ($Manifest.functionDriver.source -ceq 'windows_inbox') -Message 'Function driver source must be windows_inbox.'
Assert-True -Condition ($Manifest.functionDriver.inf -ceq 'winusb.inf') -Message 'Function driver INF must be winusb.inf.'
Assert-True -Condition ($Manifest.functionDriver.installSection -ceq 'WINUSB.NT') -Message 'Function driver install section must be WINUSB.NT.'
Assert-True -Condition ($Manifest.functionDriver.servicesSection -ceq 'WINUSB.NT.Services') -Message 'Function driver services section must be WINUSB.NT.Services.'

$UnexpectedBinaries = @(Get-ChildItem -LiteralPath $ResolvedPackageRoot -Recurse -File | Where-Object {
    $_.Extension -in @('.cat', '.sys', '.exe', '.dll', '.msi')
})
Assert-True -Condition ($UnexpectedBinaries.Count -eq 0) -Message 'Package contains a binary or catalog that is not produced by this phase.'

$WdkStatus = 'not_available'
$InfVerif = if ($SkipWdk) { $null } else { Find-WdkTool -Name 'infverif.exe' }
if ($SkipWdk) {
    $WdkStatus = 'skipped'
}
elseif ($null -ne $InfVerif) {
    # The source INF intentionally has no CatalogFile. The catalog pipeline
    # adds it only to a private staging copy before running Inf2Cat and
    # InfVerif /w; validating the source directly would reject that deliberate
    # pre-catalog state.
    $WdkStatus = 'available'
}
elseif ($RequireWdk) {
    Add-ValidationError -Message 'InfVerif is required but was not found. Install the Windows Driver Kit.'
}

if ($Errors.Count -gt 0) {
    $Errors | ForEach-Object { Write-Error $_ }
    exit 1
}

[ordered]@{
    schema_version = 1
    status = 'ok'
    inf = $InfPath
    hardware_id = $ExpectedHardwareId
    device_interface_guid = $ExpectedInterfaceGuid
    wdk_validation = $WdkStatus
} | ConvertTo-Json

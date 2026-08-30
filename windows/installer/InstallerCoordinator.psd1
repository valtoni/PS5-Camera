@{
    RootModule = 'InstallerCoordinator.psm1'
    ModuleVersion = '1.0.0'
    GUID = 'b95c5820-2962-4ea3-a6c7-9632032b573f'
    Author = 'PS5 Camera Clean-room Project'
    Description = 'Dry-run-only transactional planner for the PS5 Camera Windows installer.'
    PowerShellVersion = '7.0'
    FunctionsToExport = @('New-Ps5CameraInstallerPlan', 'Test-PathInsideRoot', 'Test-InfScope')
    CmdletsToExport = @()
    VariablesToExport = @()
    AliasesToExport = @()
}

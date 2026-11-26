param(
    [Parameter(Mandatory = $true)][string]$BinaryPath,
    [Parameter(Mandatory = $true)][string]$ServiceBinaryPath,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Tag,
    [Parameter(Mandatory = $true)][string]$OutDir
)

$ErrorActionPreference = "Stop"

$RootDir = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$BinAbs = (Resolve-Path $BinaryPath).Path
$SvcAbs = (Resolve-Path $ServiceBinaryPath).Path
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$OutAbs = (Resolve-Path $OutDir).Path

# Copy standalone portable executable and zip archive (includes optional MFT index service binary)
Copy-Item $BinAbs (Join-Path $OutAbs "texplorer-$Tag-windows-x86_64.exe") -Force

$Stage = Join-Path $OutAbs "stage_win"
New-Item -ItemType Directory -Path $Stage -Force | Out-Null
Copy-Item $BinAbs (Join-Path $Stage "texplorer.exe")
Copy-Item $SvcAbs (Join-Path $Stage "texplorer-service.exe")
Copy-Item (Join-Path $RootDir "README.md") (Join-Path $Stage "README.md")
Compress-Archive -Path "$Stage\*" -DestinationPath (Join-Path $OutAbs "texplorer-$Tag-windows-x86_64.zip") -Force
Remove-Item -Path $Stage -Recurse -Force

# Locate or install Inno Setup 6
$Iscc = (Get-Command "iscc.exe" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source)
if (-not $Iscc) {
    $DefaultIscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    if (Test-Path $DefaultIscc) {
        $Iscc = $DefaultIscc
    }
}
if (-not $Iscc) {
    Write-Host "Installing Inno Setup via Chocolatey..."
    choco install innosetup -y --no-progress | Out-Null
    $DefaultIscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    if (Test-Path $DefaultIscc) {
        $Iscc = $DefaultIscc
    } else {
        $Iscc = (Get-Command "iscc.exe" -ErrorAction Stop | Select-Object -First 1 -ExpandProperty Source)
    }
}

$IssPath = Join-Path $PSScriptRoot "texplorer.iss"
$SetupBase = "texplorer-$Tag-windows-x86_64-setup"
& $Iscc "/DMyAppVersion=$Version" "/DBinaryPath=$BinAbs" "/DServiceBinaryPath=$SvcAbs" "/DOutputDir=$OutAbs" "/DOutputBaseFilename=$SetupBase" $IssPath
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
}

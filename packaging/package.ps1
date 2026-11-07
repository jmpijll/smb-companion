<#
.SYNOPSIS
  Builds the release packages from an already-built release exe:
    dist\smb-lol-v<version>-setup.exe          (installer, per-user)
    dist\smb-lol-v<version>-portable.zip       (portable folder)
    dist\SHA256SUMS.txt
.EXAMPLE
  cargo build --release
  pwsh packaging\package.ps1 -Iscc "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
#>
param(
    [string]$Exe = "target\release\smb-lol.exe",
    [string]$Iscc = "ISCC.exe",
    [string]$OutDir = "dist"
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $version) { throw "No version in Cargo.toml" }
if (-not (Test-Path $Exe)) { throw "Missing $Exe. Run: cargo build --release" }
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
Get-ChildItem $OutDir -File | Remove-Item

# Portable: the marker file keeps settings next to the exe instead of %APPDATA%.
$name = "SMB LoL Companion"
$stage = Join-Path ([IO.Path]::GetTempPath()) "smb-lol-portable-$version"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage\$name" | Out-Null
Copy-Item $Exe "$stage\$name\smb-lol.exe"
Copy-Item README.md, THIRD_PARTY_NOTICES.md "$stage\$name\"
Copy-Item LICENSE "$stage\$name\LICENSE.txt"
@"
SMB LoL Companion - portable mode.

While this file sits next to smb-lol.exe, settings are stored in this folder
instead of %APPDATA%. Delete it to switch to the normal per-user settings.
"@ | Set-Content -Encoding utf8 "$stage\$name\smb-lol.portable"
Compress-Archive -Path "$stage\$name" -DestinationPath "$OutDir\smb-lol-v$version-portable.zip" -CompressionLevel Optimal
Remove-Item -Recurse -Force $stage

# Installer.
& $Iscc /Q "/DAppVersion=$version" "/DSourceExe=$Exe" "/DOutputDir=$OutDir" packaging\smb-lol.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

Get-ChildItem $OutDir -File | Where-Object Name -ne SHA256SUMS.txt | Sort-Object Name | ForEach-Object {
    "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
} | Set-Content -Encoding ascii "$OutDir\SHA256SUMS.txt"
Get-ChildItem $OutDir | Format-Table Name, Length -AutoSize

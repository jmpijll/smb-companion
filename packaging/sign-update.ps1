<#
.SYNOPSIS
  Ships a published release to installed copies: signs its installer and puts
  the signed update.json live on GitHub Pages and on the release itself.
  Installed copies pick it up silently within a day (src/update.rs).

  Run on the maintainer's PC after the Release workflow has finished. The
  private key stays on this PC; CI never sees it (src/signing.rs). Without
  this step a release is only a download page: nothing updates.
.EXAMPLE
  pwsh packaging\sign-update.ps1 -Tag v0.4.0
#>
#Requires -Version 7.3
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Key = (Join-Path $HOME ".smb-companion-release\update-key.pk8"),
    [string]$Repo = "jmpijll/smb-companion",
    # Only for releases from before the rename (smb-lol-v0.3.0-setup.exe).
    [string]$Asset = "smb-companion-$Tag-setup.exe"
)
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Split-Path $PSScriptRoot -Parent)

if ($Tag -notmatch '^v(\d+\.\d+\.\d+)$') { throw "Tag must be vX.Y.Z, got $Tag" }
$version = $Matches[1]
if (-not (Test-Path $Key)) { throw "No signing key at $Key. Restore it from the backup; a new key would not be trusted by installed copies." }
$owner, $name = $Repo.Split("/")
$pages = "https://$owner.github.io/$name/update.json"

$work = Join-Path ([IO.Path]::GetTempPath()) "smb-companion-sign-$Tag-$PID"
New-Item -ItemType Directory $work | Out-Null
try {
    gh release download $Tag -R $Repo -p $Asset -p SHA256SUMS.txt -D $work
    $setup = Join-Path $work $Asset
    $hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    $listed = Get-Content (Join-Path $work "SHA256SUMS.txt") | ForEach-Object {
        $h, $f = $_.Trim() -split '\s+', 2
        if ($f -and $f.TrimStart('*') -eq $Asset) { $h.ToLowerInvariant() }
    }
    if ($listed -ne $hash) { throw "$Asset on the release does not match its SHA256SUMS.txt" }

    $feed = Join-Path $work "update.json"
    $url = "https://github.com/$Repo/releases/download/$Tag/$Asset"
    cargo run --quiet --example sign_update -- sign $Key $version $url $setup $feed
    $want = [IO.File]::ReadAllText($feed)

    # Fallback copy on the release (served as releases/latest/download/update.json).
    gh release upload $Tag $feed -R $Repo --clobber

    # GitHub Pages serves the gh-pages branch.
    $existing = try { gh api "repos/$Repo/contents/update.json?ref=gh-pages" --jq .sha 2>$null } catch { $null }
    $put = @("api", "-X", "PUT", "repos/$Repo/contents/update.json",
        "-f", "branch=gh-pages", "-f", "message=update.json: $Tag",
        "-f", "content=$([Convert]::ToBase64String([IO.File]::ReadAllBytes($feed)))")
    if ($existing) { $put += @("-f", "sha=$existing") }
    gh @put --silent

    $latest = gh release view -R $Repo --json tagName --jq .tagName
    $check = @($pages)
    if ($latest -eq $Tag) { $check += "https://github.com/$Repo/releases/latest/download/update.json" }
    else { Write-Warning "$Tag is not the latest release ($latest); only GitHub Pages is checked." }
    foreach ($u in $check) {
        $deadline = (Get-Date).AddMinutes(10)
        while ($true) {
            try { $got = (Invoke-WebRequest "$u`?t=$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())" -TimeoutSec 30).Content } catch { $got = $null }
            if ($got -is [byte[]]) { $got = [Text.Encoding]::UTF8.GetString($got) }
            if ($got -eq $want) { Write-Host "Live: $u"; break }
            if ((Get-Date) -gt $deadline) { throw "$u does not serve the new update.json after 10 minutes" }
            Start-Sleep -Seconds 15
        }
    }
    Write-Host "Installed copies will update to $version within a day."
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}

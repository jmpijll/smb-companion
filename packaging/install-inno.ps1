<#
.SYNOPSIS
  CI helper: installs a pinned Inno Setup in portable mode (no system changes)
  after checking its Authenticode signature, and exports ISCC to GITHUB_ENV.
#>
param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = "Stop"
$tag = "is-" + ($Version -replace '\.', '_')
$exe = Join-Path $env:RUNNER_TEMP "innosetup-$Version.exe"
$dir = Join-Path $env:RUNNER_TEMP "inno"
Invoke-WebRequest "https://github.com/jrsoftware/issrc/releases/download/$tag/innosetup-$Version.exe" -OutFile $exe
$sig = Get-AuthenticodeSignature $exe
if ($sig.Status -ne "Valid" -or $sig.SignerCertificate.Subject -notmatch "Pyrsys B\.V\.") {
    throw "Unexpected Inno Setup signature: $($sig.Status) $($sig.SignerCertificate.Subject)"
}
Start-Process -Wait -FilePath $exe -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/CURRENTUSER", "/PORTABLE=1", "/NORESTART", "/DIR=$dir"
$iscc = Join-Path $dir "ISCC.exe"
if (-not (Test-Path $iscc)) { throw "ISCC.exe not found after installing Inno Setup" }
"ISCC=$iscc" | Out-File -Append -Encoding utf8 $env:GITHUB_ENV
Write-Host "Inno Setup $Version ready: $iscc"

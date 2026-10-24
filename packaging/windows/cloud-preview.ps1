<# Unsigned x64 desktop preview; build and test before calling this script. #>
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$Exe = Join-Path $Root 'target/x86_64-pc-windows-msvc/release/lightcraft.exe'
$Sha = (git -C $Root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $Sha -notmatch '^[a-f0-9]{40}$') { throw 'Cannot identify source commit' }
if ($env:LIGHTCRAFT_BUILD_SHA -and $env:LIGHTCRAFT_BUILD_SHA -ne $Sha) { throw 'Build/source commit mismatch' }

# Reject a wrong architecture or a console executable before naming the artifact x64.
$Bytes = [IO.File]::ReadAllBytes($Exe)
if ($Bytes.Length -lt 256) { throw 'Truncated executable' }
$Pe = [BitConverter]::ToInt32($Bytes, 0x3C)
if ($Pe -lt 64 -or $Pe -gt $Bytes.Length - 0x60) { throw 'Invalid PE offset' }
if ([BitConverter]::ToUInt32($Bytes, $Pe) -ne 0x4550 -or
    [BitConverter]::ToUInt16($Bytes, $Pe + 4) -ne 0x8664 -or
    [BitConverter]::ToUInt16($Bytes, $Pe + 0x5C) -ne 2) {
  throw 'Expected a Windows x64 GUI executable'
}

$Stage = Join-Path $Root 'dist/windows-cloud/portable'
# This fixed directory contains only generated packaging output; never carry files
# from a previous build into the downloadable artifact.
if (Test-Path -LiteralPath $Stage) { Remove-Item -LiteralPath $Stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item -LiteralPath $Exe -Destination (Join-Path $Stage 'LightCraft.exe')
foreach ($File in 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE') {
  Copy-Item -LiteralPath (Join-Path $Root $File) -Destination $Stage
}
Copy-Item -LiteralPath (Join-Path $Root 'docs/windows-cloud-testing.md') -Destination (Join-Path $Stage 'START-HERE.md')
. (Join-Path $PSScriptRoot 'font-licences.ps1')
Write-FontLicences (Join-Path $Stage 'FONT-LICENCES.txt') (Join-Path $Root 'assets/fonts/OFL-Inter.txt') $env:CRAFT_FONTS_DIR
$Info = [ordered]@{
  commit = $Sha
  target = 'x86_64-pc-windows-msvc'
  built_at_utc = (Get-Date).ToUniversalTime().ToString('o')
  executable_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
  signed = $false
  cloud = 'Built in; no optional Cloud feature flag'
  online_login_test = 'Not run in this workflow; requires a separate HTTPS Backend/Keycloak'
}
$Info | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Stage 'BUILD-INFO.json') -Encoding utf8
# upload-artifact creates the downloadable ZIP; its root contains LightCraft.exe.
$Checksums = @(Get-ChildItem -LiteralPath $Stage -File | Sort-Object Name | ForEach-Object {
  "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $($_.Name)"
})
$Checksums | Set-Content -LiteralPath (Join-Path $Stage 'SHA256SUMS.txt') -Encoding ascii
Write-Output "Prepared $Stage (unsigned x64 preview, commit $Sha)"

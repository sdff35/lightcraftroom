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

$Name = "lightcraft-cloud-windows-x64-$($Sha.Substring(0, 7))"
$Stage = Join-Path $Root "target/windows-cloud-package/$Name"
$Dist = Join-Path $Root 'dist/windows-cloud'
New-Item -ItemType Directory -Force -Path $Stage, $Dist | Out-Null
Copy-Item -LiteralPath $Exe -Destination $Stage
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
$Zip = Join-Path $Dist "$Name.zip"
Compress-Archive -Path $Stage -DestinationPath $Zip -Force
"$((Get-FileHash -LiteralPath $Zip -Algorithm SHA256).Hash.ToLowerInvariant())  $Name.zip" |
  Set-Content -LiteralPath (Join-Path $Dist 'SHA256SUMS.txt') -Encoding ascii
Copy-Item -LiteralPath (Join-Path $Stage 'BUILD-INFO.json') -Destination $Dist
Write-Output "Prepared $Zip (unsigned preview, commit $Sha)"

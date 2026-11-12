<#
.SYNOPSIS
  Unpacks the committed, Windows-built test fixtures into fixtures/out and verifies them.

.DESCRIPTION
  The images were generated once by windows/make-fixtures.ps1 and windows/make-carving-fixture.ps1
  (Administrator, Windows 11) and are committed compressed, so every machine and CI runner tests
  against exactly the same bytes. Regenerating them on another machine can legitimately differ
  (e.g. whether deleted data was TRIMmed), which would make the tests environment-dependent.

  Works with Windows PowerShell 5.1 and PowerShell 7 on Windows and macOS.
#>
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$out = Join-Path $here 'out'
New-Item -ItemType Directory -Force $out | Out-Null
Expand-Archive -Path (Join-Path $here 'windows-built-fixtures.zip') -DestinationPath $out -Force

$failed = 0
foreach ($line in Get-Content (Join-Path $here 'SHA256SUMS')) {
    if (-not $line.Trim()) { continue }
    $hash, $name = $line -split '\s+', 2
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $out $name)).Hash.ToLower()
    if ($actual -ne $hash) {
        Write-Error "checksum mismatch: $name" -ErrorAction Continue
        $failed++
    } else {
        "ok  $name"
    }
}
if ($failed -gt 0) { throw "$failed fixture(s) failed verification" }
"fixtures ready in $out"

<#
.SYNOPSIS
  Builds raw disk-image test fixtures using Windows' own partitioning and filesystem drivers.

.DESCRIPTION
  Must run as Administrator (diskpart). For each image it creates a fixed VHD, partitions and
  formats it with diskpart, writes deterministic files (including Unicode names, an empty file
  and a deliberately fragmented file), deletes some of them, detaches the VHD and strips the
  512-byte VHD footer, leaving a raw sector-for-sector image.

  A manifest (manifest.json) records the partition layout as Windows reports it and the path,
  size, SHA-256 and deleted-state of every file, so parsers are verified against what the real
  OS wrote rather than against our own writer.

  Outputs go to fixtures/out (git-ignored). Volumes are mounted temporarily into empty folders
  under fixtures/out/mnt (no drive letters); nothing else on the system is modified.
#>
param([string]$OutDir = (Join-Path $PSScriptRoot "..\out"))

$ErrorActionPreference = 'Stop'
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force $OutDir | Out-Null
$log = Join-Path $OutDir "make-fixtures.log"
"started $(Get-Date -Format o)" | Set-Content $log

function Log($m) { $m | Add-Content $log; Write-Host $m }

function Invoke-Diskpart([string[]]$lines) {
    $f = Join-Path $env:TEMP "khodz-dp-$([guid]::NewGuid()).txt"
    Set-Content -Path $f -Value $lines -Encoding ascii
    $out = & diskpart.exe /s $f 2>&1 | Out-String
    Remove-Item $f -Force
    Log $out
    if ($LASTEXITCODE -ne 0) { throw "diskpart failed (exit $LASTEXITCODE)" }
}

function New-Bytes([int]$seed, [int]$len) {
    $r = New-Object System.Random($seed)
    $b = New-Object byte[] $len
    $r.NextBytes($b)
    return ,$b
}

function Get-Sha256([byte[]]$bytes) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    return (($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString("x2") }) -join "")
}

# Writes the standard file set to a volume root and returns manifest entries.
function Add-TestFiles([string]$root, [int]$seedBase) {
    $entries = New-Object System.Collections.ArrayList
    function Put([string]$rel, [byte[]]$data, [bool]$deleted = $false) {
        $full = Join-Path $root $rel
        New-Item -ItemType Directory -Force (Split-Path $full) | Out-Null
        [System.IO.File]::WriteAllBytes($full, $data)
        [void]$entries.Add([ordered]@{ path = $rel.Replace('\', '/'); size = $data.Length; sha256 = (Get-Sha256 $data); deleted = $deleted; fragmented = $false })
    }

    Put "small.txt"                              (New-Bytes ($seedBase + 1) 300)
    Put "empty.txt"                              ([byte[]]@())
    Put "docs\report.bin"                        (New-Bytes ($seedBase + 2) (100KB))
    Put "docs\nested\deep\image.dat"             (New-Bytes ($seedBase + 3) (1MB))
    Put "Long file name with spaces.dat"         (New-Bytes ($seedBase + 4) (20KB))
    Put "caf$([char]0xE9) $([char]0xF1) $([char]0x65E5)$([char]0x672C).txt" (New-Bytes ($seedBase + 5) (5KB))

    # Fragmentation: interleave appends of two files so the first one's clusters are split.
    $a = Join-Path $root "frag-a.bin"; $f = Join-Path $root "filler.bin"
    $partA1 = New-Bytes ($seedBase + 6) (64KB); $fill = New-Bytes ($seedBase + 7) (64KB); $partA2 = New-Bytes ($seedBase + 8) (64KB)
    [System.IO.File]::WriteAllBytes($a, $partA1)
    [System.IO.File]::WriteAllBytes($f, $fill)
    $s = [System.IO.File]::Open($a, 'Append'); $s.Write($partA2, 0, $partA2.Length); $s.Close()
    $whole = New-Object byte[] ($partA1.Length + $partA2.Length)
    [Array]::Copy($partA1, $whole, $partA1.Length); [Array]::Copy($partA2, 0, $whole, $partA1.Length, $partA2.Length)
    [void]$entries.Add([ordered]@{ path = "frag-a.bin"; size = $whole.Length; sha256 = (Get-Sha256 $whole); deleted = $false; fragmented = $true })
    [void]$entries.Add([ordered]@{ path = "filler.bin"; size = $fill.Length; sha256 = (Get-Sha256 $fill); deleted = $false; fragmented = $false })

    # Files that are created and then deleted.
    $del = @(
        @{ rel = "to-delete-1.bin"; data = (New-Bytes ($seedBase + 9) (200KB)) },
        @{ rel = "docs\to-delete-2.txt"; data = (New-Bytes ($seedBase + 10) (3KB)) },
        @{ rel = "deleted-dir\inner.bin"; data = (New-Bytes ($seedBase + 11) (50KB)) }
    )
    foreach ($d in $del) { Put $d.rel $d.data $true }
    Remove-Item (Join-Path $root "to-delete-1.bin") -Force
    Remove-Item (Join-Path $root "docs\to-delete-2.txt") -Force
    Remove-Item (Join-Path $root "deleted-dir") -Recurse -Force
    return ,$entries
}

# Each image: name, size (MB), partition style and partitions.
$images = @(
    @{ name = "mbr-fat32";  sizeMB = 64;  style = "mbr"; parts = @(@{ kind = "primary"; fs = "fat32"; label = "KFAT32" }) },
    @{ name = "gpt-exfat";  sizeMB = 64;  style = "gpt"; parts = @(@{ kind = "primary"; fs = "exfat"; label = "KEXFAT" }) },
    @{ name = "gpt-ntfs";   sizeMB = 64;  style = "gpt"; parts = @(@{ kind = "primary"; fs = "ntfs";  label = "KNTFS" }) },
    @{ name = "mbr-multi";  sizeMB = 160; style = "mbr"; parts = @(
        @{ kind = "primary";  size = 40; fs = "fat32"; label = "PFAT" },
        @{ kind = "extended" },
        @{ kind = "logical";  size = 40; fs = "ntfs";  label = "LNTFS" },
        @{ kind = "logical";  size = 40; fs = "exfat"; label = "LEXFAT" }) }
)

$manifest = [ordered]@{ generator = "fixtures/windows/make-fixtures.ps1"; os = [Environment]::OSVersion.VersionString; created = (Get-Date -Format o); images = @() }
$seed = 1000

foreach ($img in $images) {
    $vhd = Join-Path $OutDir "$($img.name).vhd"
    $raw = Join-Path $OutDir "$($img.name).img"
    foreach ($p in @($vhd, $raw)) { if (Test-Path $p) { Remove-Item $p -Force } }
    Log "=== $($img.name) ==="

    $formatted = @($img.parts | Where-Object { $_.fs })
    $dp = @("create vdisk file=`"$vhd`" maximum=$($img.sizeMB) type=fixed", "select vdisk file=`"$vhd`"", "attach vdisk", "convert $($img.style)")
    foreach ($p in $img.parts) {
        $size = if ($p.size) { " size=$($p.size)" } else { "" }
        $dp += "create partition $($p.kind)$size"
        if ($p.fs) {
            # Mount into an empty folder rather than a drive letter: no Explorer/AutoPlay pop-ups.
            $p.mount = Join-Path $OutDir "mnt\$($img.name)-$($p.label)"
            New-Item -ItemType Directory -Force $p.mount | Out-Null
            $dp += "format fs=$($p.fs) quick label=$($p.label)"
            $dp += "assign mount=`"$($p.mount)`""
        }
    }
    Invoke-Diskpart $dp

    $disk = Get-DiskImage -ImagePath $vhd | Get-Disk
    $vols = @()
    foreach ($p in $formatted) {
        $files = Add-TestFiles "$($p.mount)\" $seed
        $seed += 100
        $part = Get-Partition -DiskNumber $disk.Number | Where-Object { $_.AccessPaths -contains "$($p.mount)\" }
        if (-not $part) { throw "partition for $($p.mount) not found" }
        $vols += [ordered]@{ label = $p.label; filesystem = $p.fs; offset = $part.Offset; size = $part.Size; files = $files }
    }
    $partitions = @(Get-Partition -DiskNumber $disk.Number | Sort-Object Offset | ForEach-Object {
        [ordered]@{ number = $_.PartitionNumber; offset = $_.Offset; size = $_.Size; gpt_type = "$($_.GptType)"; mbr_type = $_.MbrType; is_extended = ($_.MbrType -in 5, 15) }
    })

    # Detaching flushes and dismounts every volume cleanly.
    Invoke-Diskpart @("select vdisk file=`"$vhd`"", "detach vdisk")
    foreach ($p in $formatted) { Remove-Item $p.mount -Force -Recurse -ErrorAction SilentlyContinue }

    # Fixed VHD = raw disk bytes + 512-byte footer.
    $fs = [System.IO.File]::Open($vhd, 'Open', 'ReadWrite')
    $footer = New-Object byte[] 512
    [void]$fs.Seek(-512, 'End'); [void]$fs.Read($footer, 0, 512)
    if ([System.Text.Encoding]::ASCII.GetString($footer, 0, 8) -ne "conectix") { $fs.Close(); throw "unexpected VHD footer" }
    $fs.SetLength($fs.Length - 512); $fs.Close()
    Move-Item $vhd $raw
    Set-ItemProperty $raw -Name IsReadOnly -Value $true

    $manifest.images += [ordered]@{
        file = "$($img.name).img"; size = (Get-Item $raw).Length; partition_style = $img.style
        disk_signature = $disk.Signature; disk_guid = "$($disk.Guid)"
        partitions = $partitions; volumes = $vols
    }
}

$manifest | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $OutDir "manifest.json") -Encoding utf8
Log "done $(Get-Date -Format o)"

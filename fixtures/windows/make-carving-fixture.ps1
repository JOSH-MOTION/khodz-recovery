<#
.SYNOPSIS
  Builds fixtures/out/carving.img: real files produced by real encoders (System.Drawing JPEG /
  PNG / GIF / BMP, System.IO.Compression deflate ZIP and an Office-style package) laid out at
  4 KiB-aligned offsets between random noise, plus deliberately fragmented and truncated copies.
  No administrator rights needed. Writes carving-manifest.json with each file's offset, length,
  SHA-256 and the status the carver must report.
#>
param([string]$OutDir = (Join-Path $PSScriptRoot "..\out"))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing, System.IO.Compression
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force $OutDir | Out-Null

$rng = New-Object System.Random(4242)
function Noise([int]$n) { $b = New-Object byte[] $n; $rng.NextBytes($b); ,$b }
function Sha([byte[]]$b) { (([System.Security.Cryptography.SHA256]::Create().ComputeHash($b)) | ForEach-Object { $_.ToString('x2') }) -join '' }

function Picture([int]$w, [int]$h, [int]$seed) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $r = New-Object System.Random($seed)
    $rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.Color]::FromArgb(11, 18, 32)), ([System.Drawing.Color]::FromArgb(34, 211, 238)), 35.0
    $g.FillRectangle($brush, $rect)
    for ($i = 0; $i -lt 40; $i++) {
        $c = [System.Drawing.Color]::FromArgb(180, $r.Next(256), $r.Next(256), $r.Next(256))
        $g.FillEllipse((New-Object System.Drawing.SolidBrush $c), $r.Next($w), $r.Next($h), $r.Next(20, 160), $r.Next(20, 160))
    }
    $g.DrawString("Khodz fixture $seed", (New-Object System.Drawing.Font 'Segoe UI', 28), [System.Drawing.Brushes]::White, 20, 20)
    $g.Dispose()
    return $bmp
}

function Encode($bmp, $format) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, $format)
    ,$ms.ToArray()
}

function Zip([hashtable]$entries) {
    $ms = New-Object System.IO.MemoryStream
    $za = New-Object System.IO.Compression.ZipArchive($ms, [System.IO.Compression.ZipArchiveMode]::Create, $true)
    foreach ($k in ($entries.Keys | Sort-Object)) {
        $e = $za.CreateEntry($k, [System.IO.Compression.CompressionLevel]::Optimal)
        $s = $e.Open(); $d = [System.Text.Encoding]::UTF8.GetBytes($entries[$k]); $s.Write($d, 0, $d.Length); $s.Close()
    }
    $za.Dispose()
    ,$ms.ToArray()
}

$fmt = [System.Drawing.Imaging.ImageFormat]
$p1 = Picture 800 600 1; $p2 = Picture 640 480 2; $p3 = Picture 320 240 3
$text = (1..400 | ForEach-Object { "Line $_ of a recoverable document with some varied text $($_ * 7919)." }) -join "`n"
$files = [ordered]@{
    "photo.jpg"     = @{ data = (Encode $p1 $fmt::Jpeg); kind = "jpeg"; expect = "complete" }
    "screenshot.png"= @{ data = (Encode $p2 $fmt::Png);  kind = "png";  expect = "complete" }
    "anim.gif"      = @{ data = (Encode $p3 $fmt::Gif);  kind = "gif";  expect = "complete" }
    "drawing.bmp"   = @{ data = (Encode $p3 $fmt::Bmp);  kind = "bmp";  expect = "unverified" }
    "archive.zip"   = @{ data = (Zip @{ "readme.txt" = $text; "data/values.csv" = ((1..2000 | ForEach-Object { "$_,$($_*$_)" }) -join "`n") }); kind = "zip"; expect = "complete" }
    "report.docx"   = @{ data = (Zip @{ "[Content_Types].xml" = '<?xml version="1.0"?><Types/>'; "word/document.xml" = "<w:document>$text</w:document>"; "_rels/.rels" = "<Relationships/>" }); kind = "docx"; expect = "complete" }
}
# Fragmented copies: the second half is displaced by foreign data. Truncated copy: cut short.
$jpg = $files["photo.jpg"].data; $png = $files["screenshot.png"].data; $gifb = $files["anim.gif"].data
function Fragment([byte[]]$f, [byte[]]$gap) {
    $cut = [int]([math]::Floor($f.Length / 2 / 4096) * 4096)
    $out = New-Object byte[] ($f.Length + $gap.Length)
    [Array]::Copy($f, 0, $out, 0, $cut); [Array]::Copy($gap, 0, $out, $cut, $gap.Length); [Array]::Copy($f, $cut, $out, $cut + $gap.Length, $f.Length - $cut)
    ,$out
}
$files["photo-fragmented-noise.jpg"] = @{ data = (Fragment $jpg (Noise 16384)); kind = "jpeg"; expect = "not_complete" }
$files["photo-fragmented-zeros.jpg"] = @{ data = (Fragment $jpg (New-Object byte[] 16384)); kind = "jpeg"; expect = "not_complete" }
$files["shot-fragmented-other.png"] = @{ data = (Fragment $png $gifb); kind = "png"; expect = "not_complete" }
$files["anim-fragmented.gif"]       = @{ data = (Fragment $gifb (Noise 8192)); kind = "gif"; expect = "not_complete" }
$cutJpg = New-Object byte[] ([int]($jpg.Length * 0.6)); [Array]::Copy($jpg, $cutJpg, $cutJpg.Length)
$files["photo-truncated.jpg"] = @{ data = $cutJpg; kind = "jpeg"; expect = "not_complete" }

$img = New-Object System.IO.MemoryStream
$lead = Noise 65536; $img.Write($lead, 0, $lead.Length)
$manifest = @()
foreach ($name in $files.Keys) {
    $f = $files[$name]
    $pad = (4096 - ($img.Length % 4096)) % 4096
    $n = Noise $pad; $img.Write($n, 0, $n.Length)
    $off = $img.Length
    $img.Write($f.data, 0, $f.data.Length)
    $manifest += [ordered]@{ name = $name; kind = $f.kind; offset = $off; length = $f.data.Length; sha256 = (Sha $f.data); expect = $f.expect }
    $gap = Noise (8192 + $rng.Next(4096)); $img.Write($gap, 0, $gap.Length)
}
$pad = (4096 - ($img.Length % 4096)) % 4096; $n = Noise $pad; $img.Write($n, 0, $n.Length)
$path = Join-Path $OutDir "carving.img"
if (Test-Path $path) { Set-ItemProperty $path -Name IsReadOnly -Value $false }
[System.IO.File]::WriteAllBytes($path, $img.ToArray())
Set-ItemProperty $path -Name IsReadOnly -Value $true
@{ image = "carving.img"; size = $img.Length; files = $manifest } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $OutDir "carving-manifest.json") -Encoding utf8
"wrote $path ($($img.Length) bytes, $($manifest.Count) files)"

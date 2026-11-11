# Supported Filesystems

Status as of **Stage 4 (2026-11-11)**. A capability is ✅ only when a test proves it on real
media; "verified" below means against disk images that **Windows itself** partitioned,
formatted and populated (`fixtures/windows/make-fixtures.ps1`), not against our own writer.

| Capability | FAT12/16 | FAT32 | exFAT | NTFS | HFS+ | APFS |
|---|---|---|---|---|---|---|
| Detect / identify | ✅ unit | ✅ verified | ✅ verified | ✅ verified | ✅ unit (spec) | ✅ unit (spec) |
| Volume label, cluster size | ✅ unit | ✅ verified | ✅ verified | ✅ verified | — | — |
| List live files with correct content | implemented, **no fixture yet** | ✅ verified | ✅ verified | ✅ verified | ❌ not implemented | ❌ not implemented |
| Fragmented live files | via FAT chain | ✅ verified | ✅ verified | ✅ verified (runlists) | ❌ | ❌ |
| Discover deleted entries | implemented | ✅ verified | ✅ verified | ✅ verified | ❌ | ❌ |
| Deleted names | LFN fully recovered; 8.3-only names lose 1st char (flagged) | same | ✅ intact | ✅ intact (see pending-delete note) | — | — |
| Deleted-file extents | start cluster + assumed contiguity | same, plus FAT32 high-word reconstruction | exact if `NoFatChain`, else assumed | exact runs | — | — |
| Recover deleted content | Stage 5 | Stage 5 | Stage 5 | Stage 5 | — | — |

Partition tables: **MBR** (incl. extended/EBR chains), **GPT** (512 and 4096-byte sectors, header
and entry-array CRCs, backup header fallback) and superfloppy volumes are ✅ verified.

## What the verification found on real Windows 11 media

- **TRIM erases deleted data.** On the fixture VHDs Windows issued TRIM for deleted clusters
  and the freed space reads back as zeros — the same behaviour as SSDs. Deleted entries are
  still found (names, sizes, locations), but their content is gone and the app says so
  ("erased") rather than producing empty files. Expect the same on SSDs and many USB sticks.
- **FAT/exFAT deleted folders:** a folder's listing lives in its own clusters. When those are
  TRIMmed, files inside a deleted folder cannot be listed by *any* tool; the folder itself is
  still found. NTFS keeps this information in the MFT and is unaffected.
- **NTFS pending deletes (Windows 10 1903+).** If a file is deleted while another program
  (antivirus, indexer) has it open, Windows moves it to `\$Extend\$Deleted` under an ID until
  the handle closes. Its data is intact; its original name is not stored there. Khodz reports
  these as deleted files with "name lost" and they were matched to the originals by content.
- **FAT32 deleted start clusters.** Windows clears the high 16 bits of a deleted entry's start
  cluster. Khodz reconstructs it — directories are verified through their own `.` entry; files
  must land on free clusters; if more than one candidate fits the extents are marked *partial*,
  never *exact*.

## Signature carving (deep scan, any filesystem or none)

| Format | Status when intact | How completeness is verified |
|---|---|---|
| JPEG | Complete | marker chain, entropy data free of invalid markers and of 512+ byte zero runs |
| PNG | Complete | CRC of every chunk through IEND |
| GIF | Complete | block chain + LZW decode: valid codes, exactly width × height pixels |
| PDF | Complete | `startxref` points at `xref` or an xref-stream object; incremental updates followed |
| ZIP, DOCX, XLSX, PPTX | Complete | every local header ↔ central directory entry ↔ end record (ZIP64: partial) |
| MP4, MOV, HEIC | Unverified | top-level box chain only; media payload cannot be verified |
| BMP, WAV, AVI, WebP | Unverified | header plausibility and declared length only |

Fragmented, truncated or overwritten files are reported **Partial** with a reason and the
length that validated; they are never reported Complete (regression-tested). Known limit: a
JPEG whose gap is filled with another JPEG's image data is not detectable without decoding.
Not carved yet: MP3, OLE (.doc/.xls/.ppt), SQLite, camera RAW, HEIF sequences, archives other than ZIP.

## Not supported / not yet implemented

- **HFS+ and APFS file listing** — detection only. Needs the B-tree catalog (HFS+) and
  object map / fs-tree (APFS) parsers, and fixtures built with `hdiutil` on a Mac.
- **Encrypted APFS (FileVault) deleted data** — not recoverable by design (keys + TRIM).
- **BitLocker** — detected and reported; must be unlocked in Windows first.
- **NTFS files whose runs continue in extension records (`$ATTRIBUTE_LIST`)** — listed, but
  marked *partial*; following attribute lists is not implemented yet. Very fragmented or very
  large files are affected.
- **NTFS damaged `$MFT` record 0** — the `$MFTMirr` fallback is not implemented yet.
- ext2/3/4, Btrfs, XFS, ReFS, ZFS, RAID/LVM/Storage Spaces/dynamic disks, E01/AFF/VHD(X) and
  compressed DMG images.

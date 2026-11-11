# Supported Filesystems

**Current state: nothing is implemented yet.** The table below is the *target*; a cell is only
changed to ✅ when a regression test on a fixture image proves it.

| Capability | FAT32 | exFAT | NTFS | HFS+ | APFS |
|---|---|---|---|---|---|
| Detect / identify | planned | planned | planned | planned | planned |
| List live files | planned | planned | planned | planned | planned (unencrypted only) |
| Discover deleted entries | planned | planned | planned | limited | very limited |
| Exact extents for deleted files | first cluster only¹ | yes if `NoFatChain`² | yes (data runs)³ | no⁴ | no⁵ |
| Recover resident/small files | n/a | n/a | yes (resident `$DATA`) | — | — |
| Signature carving fallback | yes | yes | yes | yes | yes (unencrypted only) |

## Expected limitations (by design of the filesystems, not of the app)

1. **FAT32** — deleting a file zeroes its FAT chain. Only the first cluster and size survive, so we
   recover by *assuming contiguity*. Such files are labelled "contiguity assumed" and verified with
   format validators where possible; fragmented files will be reported as possibly corrupt, never
   as "complete".
2. **exFAT** — if the stream extension's `NoFatChain` flag was set the file was contiguous and its
   extent is exact; otherwise same caveat as FAT32.
3. **NTFS** — deleted MFT records retain data runs until reused. Clusters may have been reallocated;
   we cross-check against `$Bitmap` and mark overlaps as "possibly overwritten".
4. **HFS+** — deleting a file removes its catalog record (B-tree node rebalanced). Recovery of names
   relies on stale node slack and the journal; primary recovery path is carving.
5. **APFS** — copy-on-write with checkpoints/snapshots; deleted-file recovery is opportunistic via
   older checkpoint object maps and snapshots. **Encrypted APFS volumes (default on Apple Silicon
   and T2 Macs' internal SSDs) are unsupported for deleted data**, and TRIM on SSDs usually erases
   freed blocks — no tool can recover those.

## General

- **SSD TRIM**: on any filesystem, TRIMmed blocks typically read back as zeros. The app detects
  all-zero extents and reports "data erased by TRIM" rather than producing empty files.
- **BitLocker / FileVault**: volumes must be unlocked by the OS first (see SECURITY.md).
- **Not supported**: ext2/3/4, Btrfs, XFS, ReFS, ZFS, RAID/LVM/Storage Spaces reassembly,
  E01/AFF images (raw `.img`/`.dd`/`.bin` images and `.dmg` raw images only, initially).

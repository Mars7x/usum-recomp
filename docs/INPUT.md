# Input contract

Implemented in `crates/usum-import`, exercised by `usumctl inspect`.

## Accepted

| Format | Typical names | Notes |
|---|---|---|
| Decrypted cartridge image (NCSD) | `.3ds`, `.cci` | Trimmed or untrimmed. Partition 0 must be present in full |
| Decrypted executable partition (NCCH/CXI) | `.cxi` | |

The file name and extension are **never** used to decide anything.
Detection is by magic (`NCSD`/`NCCH` at offset 0x100).

Not accepted, each with its own error:

| Input | Error |
|---|---|
| `.cia` (detected by its 0x2020 header size) | unsupported format, "not supported yet" |
| Encrypted NCSD/NCCH | "the dump is encrypted, decrypt it on your own console" |
| Update (`0004000E…`) or DLC (`0004008C…`) titles | named as such |
| Data-only partition (CFA) | not an executable |
| Extracted-file directories (ExeFS/RomFS unpacked) | not supported. They lose the hashes that validation relies on |
| Anything else | not a 3DS image |

## Where things are found

```
NCSD header (0x000)  partition table at 0x120: 8 × (offset, size) in media units
└─ partition 0 = game NCCH (USUM: at 0x4000)
   ├─ NCCH header (0x000)         program ID 0x118, product code 0x150, version 0x112,
   │                              flags 0x188, exheader SHA-256 0x160,
   │                              ExeFS offset/size/hash-region 0x1A0, its SHA-256 0x1C0,
   │                              RomFS offset/size/hash-region 0x1B0, its SHA-256 0x1E0
   ├─ exheader (0x200, 0x400 B)   segments (0x10/0x20/0x30), BLZ flag (0x0D), bss, stack,
   │                              dependencies (0x40), save size (0x1C0),
   │                              ACI program ID (0x200), services (0x250, 34 × 8 B)
   ├─ ExeFS                       10 entries + reversed SHA-256 table; ".code" = text|rodata|data
   └─ RomFS (IVFC)                level 3 at align(0x60 + master hash size, 2^L3 block):
                                  directory/file tables (UTF-16 names) and file data;
                                  CRO modules (*.cro) and static.crs at the root
```

Sources: 3dbrew NCSD, NCCH, NCCH/Extended Header, ExeFS, RomFS, CRO0.
Offsets are relative to the start of each structure.

## Validation, in order

1. The magic says which container it is. The NCSD partition 0 must be
   non-empty and must lie within the file (else *truncated*).
2. Title kind: application, update or DLC (from the program ID's high
   word).
3. The partition's declared size must lie within the file (*truncated*).
4. **Decryption test:** SHA-256 of the exheader must match the header's
   stored hash. Hashes are over plaintext, so a mismatch means
   *encrypted* when NoCrypto is clear, and *damaged* when it is set. The
   flags alone are not trusted.
5. The exheader's segments must be page-aligned, ordered,
   non-overlapping, and must fit their pages and the 32-bit space.
6. SHA-256 of the ExeFS hash region must match. `.code`'s own SHA-256
   must match. It is decompressed (bottom-LZ) if the exheader says so,
   with a 64 MiB output cap. Its size must lie between
   text+rodata pages+data bytes and all pages.
7. SHA-256 of the RomFS superblock (IVFC header + master hash) must
   match. Directory and file tables are walked with cycle, depth, name
   and bounds checks. Every `.cro`/`.crs` is read, hashed, and its CRO0
   magic checked.
8. Profile: program ID **and** SHA-256 of the decompressed code are
   looked up in `profile::PROFILES`. A known title ID with unknown code is
   reported as an unknown revision and is unsupported.

Not checked yet: RomFS IVFC level 1–3 hashes (a full ~3.5 GiB read) and
the ExeFS icon/banner hashes. Those parts are not read.

Every offset and length from the file is untrusted. Arithmetic is
checked, reads are bounds-checked against the real file size, and a
single read is capped at 64 MiB (`source::MAX_READ`). Tests: truncation at
every 0x100-byte boundary, single-byte corruption caught by each hash, and
3,000 deterministic mutation rounds without a panic.

## Region, revision, updates

Supported means *a recorded profile*: a program ID together with a
decompressed-code hash. Currently:

| Profile | Program ID | Product code | NCCH version | Code SHA-256 |
|---|---|---|---|---|
| `ultra-sun-usa-cart` | `00040000001B5000` | CTR-P-A2AA | 2 | `b5388f75…5148e9e09` |
| `ultra-moon-usa-cart` | `00040000001B5100` | CTR-P-A2BA | 2 | `0aa12056…1e999919b` |

Other regions, revisions and update titles are unsupported until measured
and added as profiles. Updates are separate titles. A USUM cartridge also
carries partitions 6 and 7 (system update data), which are not game
updates and are ignored.

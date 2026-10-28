# Roadmap

Each milestone ends only when its exit criteria are demonstrated by tests
or recorded measurements. Milestone criteria below include historical checkpoints. Current status as of
2026-10-28: Ultra Sun reaches early gameplay; software graphics are the
reference, Vulkan is experimental, and packaging is unfinished. See
[COMPATIBILITY.md](COMPATIBILITY.md) for the current validation limits.

## M1 — Feasibility and architecture ✅ done

Exit criteria: a feasibility report with sources and evidence levels; an
architecture decision for the recompiler, runtime and Flatpak toolchain.
→ [FEASIBILITY.md](FEASIBILITY.md), [adr/](adr/).

## M2 — Import/inspection CLI ✅ done (with the limits in INPUT.md)

Exit criteria:
- [x] parses NCSD/NCCH, exheader, ExeFS, RomFS; BLZ decompression
- [x] specific errors for encrypted, truncated, corrupt (hash), CIA,
      update/DLC and non-3DS inputs; tests for each
- [x] mutation and truncation tests never panic
- [x] synthetic fixtures; no game data in the repository (guard plus its tests)
- [x] identification by ID + code hash, never file name
- [x] both real USA dumps match their profiles; the code hashes agree with
      an independent implementation (3dsrecomp), and so do all 132 CRO hashes

## M3 — ARM/Thumb → Rust proof of concept ✅ done (2026-10-25)

Scope: original synthetic programs only (written for the tests, no game
code). Delivered: `usum-guest`, `usum-arm` (ARM and Thumb subset),
`usum-recomp` (discovery and emission) and `usum-difftest`. Evidence:
RESEARCH.md R-010 to R-014.

Exit criteria:
- [x] decoder checked against an independent encoder: unit-test
      encodings come from GNU as/objdump (`usum-arm` tests), and every
      random case is encoded by GNU as from text, never by our code. (The
      original wording asked for round-trips against an independent
      *decoder*. The semantic comparison against QEMU is stronger
      evidence, so no separate decoder round-trip was built.)
- [x] ≥ 20 synthetic programs: 19 hand-written programs (loops,
      recursion past the depth limit, both GCC jump-table forms, LDM/STM in
      all modes, halfword/signed/double loads, SVCs from nested calls,
      every condition code, function pointers, Thumb ALU/calls/memory,
      ARM↔Thumb interworking in every form) plus 8 generated random
      programs per run match QEMU byte for byte
- [x] ≥ 100k random single-instruction cases with zero mismatches:
      120,000 over three seeds
- [x] negative controls: injected bugs are caught (R-012)
- [x] compile-cost benchmark of the real emitter (R-014)

Known limits carried into M4: no VFP, coprocessor (MRC/MCR), exclusive
(LDREX/STREX), saturating/parallel arithmetic, SWP, or SETEND/CPS;
unaligned word access semantics unresolved (R-010); no Thumb jump-table
recognition; emitter output is verbose (358 bytes per instruction).

## M4 — First look inside the real executable (in progress)

Exit criteria and status (evidence: RESEARCH.md R-015 to R-021):
- [x] decoder coverage over the executable, with unknown encodings
      counted: `usumctl analyze`. Every real instruction that control
      flow reaches in both executables now decodes; the 18 / 17 remaining
      sites are data
- [x] function discovery compared with 3dsrecomp: 29,269 functions
      against its 29,718; 80.9 % of text decoded as code against its 94.8 %
- [x] real functions translated and checked against QEMU on many
      inputs: 4 functions × 2,000 cases, 0 mismatches (one of them weak
      evidence)
- [x] function-level comparison of the two executables (R-021)
- [x] emitter compaction and multi-crate output (R-022, ADR-0005); the
      whole Ultra Sun executable builds in release in 5 min 49 s
- [ ] the CRO modules: load, relocate, analyze
- [ ] written plan for the next path (entry point → first SVC)

Instruction support added in M4: TLS register, barriers, PLD, CLREX,
LDREX/STREX (all sizes), the whole of VFPv2 (including short vectors),
and the ARMv6 media/DSP instructions, each with GNU-as vectors, random
differential cases and negative controls.

## M5 — Runtime integration

Ordered, each step measured:
1. ✅ (Ultra Sun) The entry point runs translated code until its first
   SVC (0x21, CreateAddressArbiter), with the register state identical to
   QEMU's (R-023).
2. Kernel objects plus fs/cfg/apt/hid stubs: the game reaches its main
   loop. Untranslated addresses fail loudly and are counted (must reach 0).
   Done (R-025 to R-031): threads (ADR-0006), waits, srv, APT, RomFS,
   cfg, DSP start-up, GSP queue and VBlank, saves and extdata, CRO
   modules (ADR-0007). Ultra Sun reaches its main loop at the language
   menu.
3. GSP command lists decoded; **first game-originated frame** rendered
   (done: the language menu, R-032), to be compared with a capture from
   the user's own console (taken by the user).
4. Title screen, menus, input, saves, battles: each is a scripted input
   replay with expected checkpoints.

## M6 — Flatpak and usable workflow

Exit criteria: ADR-0003's five tests pass from an installed app;
portal-based import; build progress UI; desktop entry and AppStream
metadata (validated with `appstreamcli validate`); reproducible offline
build (`flatpak-builder` with vendored cargo sources). Flathub
requirements are reviewed then; acceptance is not assumed.

## Later

Ultra Moon parity tests (the shared pipeline is designed in from M3),
other regions and revisions as profiles, updates, enhancements (resolution,
layouts, remapping), aarch64.

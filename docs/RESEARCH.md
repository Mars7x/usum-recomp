# Research log

Short entries, newest last. Each says what was measured or read, how,
and the result, so it can be re-run. Game-derived artifacts from these
experiments were kept in a scratch directory outside the repository and
are not part of the project.

Environment for all entries: Fedora Silverblue host (kernel 7.2.11,
fc45), Ubuntu 24.04 toolbox, 16 cores, 31 GiB RAM, rustc 1.99.1 (host),
gcc 13.3 (toolbox). Inputs: the user's own decrypted dumps
"Pokemon Ultra Sun (USA) (En,Ja,Fr,De,Es,It,Zh,Ko).3ds" and the Ultra Moon
equivalent, each 4 GiB (untrimmed).

### R-001 2026-10-25 — Existing projects, at these revisions

- `fearkov/3dsrecomp` `9b2f21717ac7142acdf3c92ccfbe1adcfaa4b38c`. MIT
  (Cargo.toml `license`; no LICENSE file). ~12k lines of Rust. Generates
  C. ABI v4 documented in `docs/interface.md`.
- `fearkov/zakuro` `951e97875e83d0d64f7be60192332cb8522369d6`. MIT
  (LICENSE). Rust HLE emulator, ~67k lines. Its COMPATIBILITY.md (v0.2.29)
  lists 00040000001B5000/1B5100 USA as "Playable, recompiled": a claim,
  not verified by us.
- `clockwork-recompdev/clockwork` `fa61bc2195ac36f1b3d432b9f4da99a41e1620c8`.
  **No license file**, nothing found by grep for license/SPDX outside
  third_party. Yo-kai Watch only. ~73k lines of C/C++ runtime, ~6k lines
  of Rust tools.
- Web search for other 3DS static recompilers found nothing relevant
  beyond these (a PSP→3DS "P3P3DS" forum post, unverified).

### R-002 — Container headers (both dumps)

NCSD magic present. Partition 0 at 0x4000. NCCH flags[7] = 0x04
(NoCrypto). Program IDs 00040000001B5000 / 00040000001B5100. Product
codes CTR-P-A2AA / CTR-P-A2BA. NCCH version 2. Partitions 1, 6 and 7
present (manual, update data). Re-run: `usumctl inspect <dump>`.

### R-003 — 3dsrecomp analysis (measurement only)

`3dsrecomp analyze` ran in 0.47 s. Ultra Sun: 84,547 functions (29,718 in
the executable, 54,829 in modules), 93.2 % code / 4.2 % literals / 2.6 %
unreached, 41,941 indirect branches, 2,664 switches, 983 svc sites, 931
Thumb functions, 3,652,689 instructions decoded. Ultra Moon is nearly the
same (29,672 executable functions). 132 modules, process name "momiji".

### R-004 — 3dsrecomp C build and verify (measurement only)

`3dsrecomp build` on Ultra Sun: 662 C files, 831 MiB; `.so` 346 MiB;
717 s wall, 16 cores, gcc 13.3. 237 `INTERPRET(...)` sites in the
generated C. Opcode classes: many coprocessor encodings (cp0, cp3, cp8)
that user code does not use, so likely data decoded as code. That is a
hypothesis. `3dsrecomp verify ... 2000`: executable functions 0
mismatched (1,109 returned, 768 stuck within limit); module functions 0
mismatched. Its reference is Zakuro's interpreter, so the comparison is
not independent.

### R-005 — Our importer vs. the dumps; sharing between games

`usumctl inspect`: all stored SHA-256s match (exheader, ExeFS header,
`.code`, RomFS superblock). `.code` is **not compressed** in these dumps
(5,914,624 bytes stored = decompressed), so BLZ is tested only
synthetically. Code SHA-256 Ultra Sun `b5388f75…e9e09`, Ultra Moon
`0aa12056…919b`, both equal to the hashes from 3dsrecomp's independent
`rom::Title::code()`. `usumctl modules`: 132 CRO hashes equal 3dsrecomp's
listing for both games. Code size = (1210+173+61) pages exactly. Both
games: same 34 services, 33 dependencies, save size 0xFE000, 747 RomFS
files. 123 of 132 CROs identical; differing: FieldMenu, FieldRo,
KisekaeData, Live, MovieLib, SequencePlayer, Title, TrainerPass, Zukan.
`static.crs` differs. Text size 0x4B99F0 vs 0x4B99F8.

### R-006 — rustc cost of per-instruction Rust (synthetic)

A throwaway generator emitted functions of 40 instructions in 4 blocks
(wrapping adds with NZCV, checked loads and stores, rotates, conditional
ops, multiplies) into 64 modules of one crate. `cargo build --release`,
opt-level 2, codegen-units 256: 20k instructions 4.6 s / 0.53 GiB, 80k
18.4 s / 1.6 GiB, 320k 74.6 s / 5.7 GiB. Linear, ~0.23 ms per
instruction. Output density 71 bytes per instruction, vs ~228 for
3dsrecomp's C. Not representative of a real emitter; replaced by the M3
benchmark.

### R-007 — C compile inside a sandboxed SDK

`flatpak run org.freedesktop.Sdk/x86_64/26.08` (via `host-spawn`,
`--nofilesystem=host`, only the scratch dir shared): gcc 16.2.0 compiled
one generated file in ~11 s wall (8 s on the host with gcc 13.3);
`$HOME/Downloads` was not visible. Shows sandboxed compiling and
filesystem isolation work. Note: Flatpak 1.18 asks for confirmation when
running a runtime ref directly; `echo 1 |` answered it.

### R-008 — Current Flatpak versions, Rust in the sandbox

`flatpak remote-info --user flathub` on 2026-10-25:
`org.freedesktop.Sdk//26.08` = freedesktop-sdk-26.08.3;
`org.freedesktop.Sdk.Extension.rust-stable//26.08` = 1.98.0 (557.1 MB
download); GNOME Platform 49, 50 and 51 listed. Inside the SDK sandbox
(`--unshare=network`, no host filesystem), with
`/usr/lib/sdk/rust-stable/bin` on PATH: rustc 1.98.0, `cargo build
--release --offline` of R-006's 20k crate plus a binary took 5.25 s, and
the binary ran. Not yet tested from an app manifest.

### R-009 — Dependency audit

`cargo tree -e normal,build`: sha2 (no `asm` feature), digest,
block-buffer, generic-array, typenum, crypto-common, cpufeatures,
cfg-if, serde, serde_derive, syn, quote, proc-macro2, unicode-ident,
serde_json, itoa, memchr, zmij, version_check. No `links` keys, no `-sys`
crates, no build-time C. See DEPENDENCIES.md.

### R-010 2026-10-25 — Unaligned word accesses fault under QEMU's ARM11

A synthetic program storing a word at an address ≡ 2 (mod 4) died with
SIGBUS under `qemu-arm -cpu arm11mpcore` (QEMU 8.2.2). ARMv6 behavior
depends on SCTLR.A/U: unaligned word LDR/STR either fault, rotate
(legacy U=0), or work (U=1). What the 3DS kernel configures for
applications is **unknown**, and it decides what `GuestMemory`/`ops` must
do. Until it is established (3dbrew, or a test on the user's own console),
test programs keep word accesses aligned. `Memory::read_u32` currently
assumes U=1 (byte-wise unaligned access), and LDM/STM/LDRD/STRD require
alignment (`ops::read_aligned`), as in every ARMv6 mode.

### R-011 2026-10-25 — Recursive descent alone misses table-reached code

`pointers.s` reached functions only through a table of addresses in data,
and a return point only through `adr lr, 1f`. Recursive descent from the
entry point did not find them, and the run failed with
`NoTranslation { address: 0x0010024c }`: the fail-loud path behaving as
designed. Adding pointer seeds (aligned words in the image that point into
the code range, odd ones as Thumb, plus ARM `adr` results) fixed it. This
is the same signal 3dsrecomp lists ("pointers in data"). In real code it
also yields false seeds, which cost unused output only.

### R-012 2026-10-25 — Negative controls for the differential harness

Each deliberate bug, injected and then reverted, was caught:

| Injected bug | Caught by |
|---|---|
| `shift_imm` ASR #32 carry from bit 30 instead of 31 | random ARM cases, but **only after** biasing shift amounts toward 1/31/32: at uniform amounts, 2,000 cases missed it |
| LDMDA/STMDA start address off by 4 | `blocks.s` and random memory cases |
| Thumb `adcs` decoded as `adds` | `thumb_alu.s` and random Thumb cases |

Lesson: random testing needs edge-biased operand distributions. Passing a
uniform random corpus is weak evidence.

### R-013 2026-10-25 — One crate of ~350k translated instructions does not fit in memory

A single differential run of 110k random cases (one generated crate,
125 MB of Rust, 349,420 instructions) was killed by the kernel OOM killer:
one rustc process held 13.0 GB RSS in the dev profile (journal:
`Killed process 3694809 (rustc) ... anon-rss:13031604kB`). Generated code
must be split across several crates sized by memory, not just by file.
Large test totals are now run as several seeds (`USUM_DIFFTEST_SEED`).

### R-014 2026-10-25 — Milestone 3 results and the real emitter's compile cost

Differential runs (`cargo test -p usum-difftest`, QEMU 8.2.2
`arm11mpcore`, GNU binutils 2.42), three seeds × default corpus:

| Seed | Programs | Translated instructions | Mismatches | Wall | Peak RSS |
|---|---|---|---|---|---|
| 0 | 27 | 134,008 | 0 | 181 s | 9.1 GB |
| 1 | 27 | 133,713 | 0 | 173 s | 9.0 GB |
| 2 | 27 | 134,041 | 0 | 173 s | 9.0 GB |

That is 120,000 random single-instruction cases (60,000 ARM data
processing/multiply/CLZ/extend/reverse, 30,000 loads/stores/LDM/STM/LDRD,
30,000 Thumb, all with random flags and conditions) plus 19 hand-written
programs per run, with zero differences in registers, flags or memory.

Compile cost of the generated crate alone (seed 2, 134,041 instructions,
emitted at 358 bytes of Rust per instruction), `cargo build -p
usum-difftest --lib`, rustc 1.99.1, 16 cores:

| Profile | Wall | CPU (user) | Peak RSS | Wall per instruction |
|---|---|---|---|---|
| dev (opt-level 0) | 15.4 s | 12.3 s | 5.2 GB | 0.11 ms |
| release (opt-level 3, debug=1) | 183 s | — | 9.6 GB | 1.37 ms |
| release, debuginfo off | 179 s | 342 s | 9.3 GB | 1.34 ms |

Within one crate only ~1.9 cores were busy. Extrapolating linearly to the
~3.65M instructions 3dsrecomp found in Ultra Sun (R-003) gives about 9,300
CPU-seconds in release: about 10 minutes on 16 cores **if** the code is
split into many crates that build in parallel. At ~70 KB peak memory per
instruction per crate, crates must stay around 20k instructions (~1.4 GB
each), and parallelism must be limited by available memory. Caveats: the
corpora are dominated by `bl` (two of every three instructions), the most
verbose emission, so real code may be cheaper per instruction. Run-time
speed of opt-level 0 output has not been measured.

### R-015 2026-10-25 — First analysis of the real executables with our own discovery

`usumctl analyze <dump>` runs `usum_recomp::analyze` in memory from the
entry point (text start 0x00100000), with the exheader's text segment as
the code range. Ultra Sun / Ultra Moon, 0.25–0.5 s each:

| | Functions (call / pointer / entry) | Text decoded as code | Undecodable sites reached by control flow |
|---|---|---|---|
| unsupported = end of path | 9,569 / 17,285 / 1 → 26,855 | 59.5 % | 1,369 |
| non-branching unsupported falls through | 11,416 / 17,602 / 1 → 29,019 | 80.1 % | 36,522 |
| + TLS, barriers, PLD, CLREX, LDREX/STREX | same | 80.1 % | 35,526 |

(Ultra Moon is within 0.3 % on every figure.) 3dsrecomp found 29,718
executable functions and 94.8 % code (R-003). Stopping at every
undecodable instruction hid most of the code: VFP instructions sit in the
middle of functions. Classes that cannot write the PC now fall through in
discovery; translation still emits a fault at each site.

What the undecodable sites are (`--detail`, disassembled by GNU objdump):
the CP15 sites were `mrc p15,0,rX,c13,c0,3` (TLS, 355) and `mcr
p15,0,rX,c7,c10,5` (DMB, 15); exclusives were LDREX/STREX; media were
UADD8/USUB8/UHADD8/SEL/UXTB16/SMLAD/SMUAD; VFP were VPUSH/VPOP,
VLDR/VSTR, VMOV, VMRS APSR_nzcv, VMUL/VNMUL, VCVT.F32.S32/U32 and back.
Remaining after increment 1: VFP ~33,000, media ~2,500, signed halfword
multiplies 56, and 11 "other" sites whose encodings are ASCII text
(control flow running into data, probably after calls that never return).

### R-016 2026-10-25 — Exclusive accesses: what is and is not tested

LDREX/STREX{,B,H,D} use a one-core local monitor (`Cpu::exclusive`): STREX
stores and returns 0 exactly when the last LDREX marked the same address,
and always clears the monitor. Not tested, deliberately: a plain store
between LDREX and STREX (whether it clears the monitor is IMPLEMENTATION
DEFINED; QEMU user mode implements STREX as a compare-and-swap against the
loaded value, so it would fail where hardware may succeed), STREX to a
different address than the LDREX, and SVCs in between (the kernel clears
the monitor on context switches, which the runtime will have to do).

### R-017 2026-10-25 — VFP: what is implemented, and what is still open

Implemented (`usum_guest::vfp`, decoder `usum_arm::vfp`): all VFPv2 data
processing, loads/stores, transfers, VMRS/VMSR, both precisions; FZ and DN;
ARM NaN rules (signaling first, operand 1 first, quieting); conversions in
all four rounding modes; short vectors (LEN/STRIDE) through a slow path
that each arithmetic site selects with one FPSCR bit test.

Open, with evidence so far:
- **The initial FPSCR of a 3DS thread is unknown.** Zakuro
  (`zakuro-core/src/kernel/thread.rs`) starts threads with 0. Other
  emulators are recalled (not verified) to use 0x03C00000 (DN, FZ,
  round toward zero). If the real value selects round toward zero, every
  arithmetic site needs directed rounding, which currently returns
  `Fault::UnsupportedFp` (loudly, never a silently wrong value).
- **Short vectors.** Zakuro's VFP code states that "the SDK does its
  matrix math in vector mode" (a claim). The executable reaches exactly
  one `vmsr fpscr` site, which fits one shared helper that sets FPSCR.
- **Cumulative exception flags** (FPSCR bits 7, 4:0) are not maintained,
  and trap enables never trap; tests mask the flags.
- **FZ tininess for double precision** is checked after rounding rather
  than before: only results exactly at the smallest-normal boundary can
  differ.

Analysis after VFP decoding (`usumctl analyze`): undecodable sites
reached by control flow went from 35,526 to 2,543 (Ultra Sun) and 2,542
(Ultra Moon); functions 29,233 / 29,267; text decoded as code 80.4 % /
80.5 %.

Differential results: 10,000 random VFP cases (edge-biased inputs:
signed zeros, infinities, quiet and signaling NaNs with payloads,
subnormals, boundaries; random FZ, DN, NZCV, rounding mode where
supported, legal LEN/STRIDE) plus `vfp_memory.s`, with zero mismatches in
the default corpus (31 programs, 169,845 translated instructions, 224 s,
10.7 GB peak). Negative controls: quiet NaNs beating signaling ones was
caught (`vmla.f32`); disabling input flushing was caught (`vcvt.f64.f32`).

### R-018 2026-10-25 — Two bugs in the reference (QEMU 8.2.2) in VFP short vectors

The differential test disagreed with QEMU in vector mode. Reading QEMU's
source (`target/arm/tcg/translate-vfp.c`, v8.2.2) located both:

1. Single-precision vector ops compute the register step as
   `delta_d = s->vec_stride + 1` from the raw FPSCR.STRIDE field, so
   STRIDE = 0b11 steps by 4. The ARM ARM and the VFP11 documentation
   define 0b11 as stride 2, which is what the double-precision path uses
   (`(s->vec_stride >> 1) + 1`). Zakuro also uses 2.
2. `do_vfp_2op_dp` advances with `vd = vfp_advance_dreg(vm, delta_m);`
   where `vm = ...` was meant, so element 1 of a two-operand
   double-precision vector op (VMOV/VABS/VNEG/VSQRT) is written to the
   advanced *source* register. The same line is still in QEMU master
   (checked 2026-10-25).

Our implementation follows the architecture. These two cases are excluded
from the QEMU comparison and are covered by unit tests in `usum-guest`
whose expected values are derived by hand from the ARM ARM rules. This is
the first measured limit of the independent reference. Short vectors are
deprecated and rarely exercised, and an independent oracle can be wrong
too. By the user's decision (2026-10-25), nothing is reported upstream.

### R-019 2026-10-25 — Media/DSP instructions; the executables now decode completely

Added: all 36 parallel add/subtract instructions (GE for the S/U forms),
SEL, PKHBT/PKHTB, SSAT/USAT/SSAT16/USAT16 (Q), the extend-and-add and
dual-byte extends, SMLAxy/SMULxy/SMLAWy/SMULWy/SMLALxy, SMUAD/SMLAD/SMUSD/
SMLSD/SMLALD/SMLSLD, SMMUL/SMMLA/SMMLS (with rounding), USAD8/USADA8,
QADD/QSUB/QDADD/QDSUB, UMAAL. Decoder vectors: 32 encodings from GNU as.
Differential: 3,000 random media cases, 0 mismatches. Negative controls:
an off-by-one in the unsigned GE threshold (caught on `uadd16eq`) and
never setting Q (caught on `smlawb`).

Analysis afterwards (`usumctl analyze`): undecodable sites reached by
control flow are **18** in Ultra Sun and **17** in Ultra Moon, down from
36,522 at the start of M4. Disassembled, all 18 are data: ASCII text
(`::Da`, `bjec`, `lMod`, `Fiel`, `Tria`), tables of code addresses, and
encodings this CPU does not have (`ldrht`, ARMv6T2; `vldmia {d16-d31}`,
VFPv3-D32). So every real instruction that control flow reaches in
either executable now decodes. Discovery still runs into data, probably
after calls that never return; that costs dead output only.

Default differential corpus after media: 33 programs, 200,565 translated
instructions, 0 mismatches, 271 s, 12.2 GB peak RSS (the default sizes
should not grow further on a 31 GB machine; use seeds).

### R-020 2026-10-25 — First real game functions verified against QEMU

Game mode (`USUM_DIFFTEST_DUMP`, `USUM_DIFFTEST_FUNCTIONS`): the build
script inspects the user's dump (a recorded profile is required), writes
the decompressed `.code` only to OUT_DIR under target/, links it at its
real address 0x00100000 (`crates/usum-difftest/game.ld`) beside the
harness at 0x00800000, and calls each function with random core and VFP
register inputs. Only the called functions are translated (the code range
for pointer seeds is the harness).

Candidates came from `usumctl analyze --leaves` (603 pure leaves in Ultra
Sun). Four were verified with 2,000 cases each, 0 mismatches:

| Address | What it appears to be (from its disassembly) | Distinct results |
|---|---|---|
| 0x005b9944 | soft-float double helper (exponent field 0x7FF arithmetic) | 336 r0, 1,313 d0 |
| 0x001178b8 | soft-float conversion helper | 921 r0 |
| 0x004d32b8 | VFP double routine (exponent compare with 20; floor/round family) | 749 d0 |
| 0x00117e8c | internal soft-float routine entered with a private register convention | 2 r0: **weak evidence** |

Negative controls in game mode: swapping the word order of VLDR.F64
broke `0x004d32b8` at case 0. Two other injections were **not** caught:
a VSUB.F64 error (the path is rarely taken) and treating equal VCMP.F64
results as greater (random doubles are almost never equal). Random inputs
need directed values (equal operands, boundary values) for those paths.

### R-021 2026-10-25 — How much executable code Ultra Sun and Ultra Moon share

`usumctl compare <us> <um>`, over the functions reached by control flow
(11,437 / 11,440): 5,590 functions (49.0 % of code bytes) are
byte-identical at the same address, 1,854 (8.2 %) identical at another
address, and 3,993 (42.8 %) have no byte-identical counterpart. 32 % of
all `.code` bytes differ at equal offsets, mostly in single-byte runs:
BL offsets and literal-pool addresses that changed because code moved.
Conclusion: translated executables are produced per game; sharing is for
the CRO modules (123 of 132 identical, R-005) and for the translator and
runtime. Whether the 43 % differ only in position-dependent fields
(which a relocation-aware comparison would show) is still open.

### R-022 2026-10-25 — Emitter compaction, split output, and the first full-size build

Compaction: BL became one call to `usum_guest::call` (depth check, call,
return-state check, exit propagation); block budgets call
`usum_guest::spend`; data processing without flags and with a simple
operand is one statement. On the R-014 corpus (seed 2, 138,593
instructions):

| | Before (R-014) | After |
|---|---|---|
| Rust per instruction | 358 bytes | 230 bytes |
| Release build, wall | 1.34 ms / instruction | 0.74 ms |
| Release build, CPU | 2.55 ms / instruction | 1.61 ms |
| Release peak memory | ~70 KB / instruction | ~46 KB |

`call` is deliberately not `#[inline]`: with it inlined, CPU time per
instruction went up to 3.23 ms. On the full default corpus (200,565
instructions) the test run fell from 265 s / 12.3 GB to 149 s / 7.9 GB,
with 0 mismatches.

Split output (ADR-0005), tested by `split.s` (7 units, 11 cross-unit table
calls) against QEMU. A deliberately wrong table index was *not* visible
in the output, since a wrong callee hands control back and the host finds
the right code. A debug assertion in `call` (the callee spent no budget)
now catches it.

**Full-size build.** `usumctl translate` on Ultra Sun: 29,269 functions,
1,758,331 instructions (code shared by overlapping functions is emitted
once per function), 90 crates of up to 20,000 instructions, 232 MB of
Rust, in 1.3 s. `cargo build --release -j 12` (rustc 1.99.1, 16 cores):
**5 min 49 s wall, 3,519 s CPU** (2.0 ms per instruction), largest rustc
process 2.7 GB, system memory in use at peak 20.2 GB (about 8.5 GB of it
in use before the build). No compile errors: every generated function is
valid Rust. This replaces the extrapolations in FEASIBILITY.md R2.

### R-023 2026-10-25 — Ultra Sun's entry point runs natively to its first SVC, matching QEMU

`usum_runtime::boot` maps text and rodata read-only and data+bss
read-write at their exheader addresses, the stack (0x40000 bytes) below
0x10000000, and a TLS block (0x1FF82000, a choice: the kernel's real
address is not established). Nothing else is mapped, so any other access
faults loudly. The generated `probe` program runs the translation from
0x00100000 until the first SVC:

    ran            197122 instructions in 0.000 s
    stopped at     svc 0x21, next instruction 0x00105560
    registers      r0=0fffffe0 ... r4=0069ca5c ... r13=0fffffdc r14=00104b9c

SVC 0x21 is CreateAddressArbiter in 3dbrew's SVC table; r0 points just
above the stack pointer, consistent with an output handle.

Cross-check: the same code, layout and initial state (TLS register,
zeroed registers, sp, flags) under `qemu-arm -cpu arm11mpcore
-one-insn-per-tb -d cpu,nochain` (scratch files, deleted afterwards)
reached the same SVC at 0x0010555C with **identical r0-r15 and NZCV**.
QEMU logged 147,847 instructions in game code. Our count (197,122) is an
upper bound: a block's full length is charged on entry even when a branch
leaves it early. Memory contents were not compared.
- Ultra Moon: the same probe reaches the same svc 0x21 with the same registers (its startup code is identical); not cross-checked against QEMU separately. Build: 5 min 52 s, 3,466 CPU-seconds, 29,303 functions, 1,761,960 instructions.

### R-024 2026-10-25 — More real functions verified; masked controls; long-running leaves

Directed inputs were added to game mode (equal operands in a quarter of
cases; doubles drawn from integer/rounding boundaries, powers of two and
specials). The two controls R-020 left uncaught are **masked, not
missed**: `0x004d32b8` is fdlibm's `ceil`, and its VADD/VSUB/VCMPE belong
to the `huge ± x > 0.0` idiom that only raises the inexact flag. With
|x| < 2^52 the sign cannot change, so no input can make those bugs
visible through this function.

15 candidates per game from `--leaves` (Ultra Sun 0x005b9944,
0x001178b8, 0x00279608, 0x00176db0, 0x00117d50, 0x00139c68, 0x003ae990,
0x00179c14, 0x005b43f0, 0x00143dfc, 0x003202e4, 0x00320378, 0x003203f8,
0x004881b0, 0x003e0710; the Ultra Moon equivalents, some at shifted
addresses), 1,000 cases each:
- 0x002ef858 was dropped: a counting loop that runs too long for QEMU's
  60 s limit on random inputs.
- 0x004881b0 (both games) crashes QEMU itself (SIGSEGV): not
  self-contained, despite the heuristic. Game mode now reports and skips a
  candidate whose reference run fails, instead of failing the build.
- 0x005b43f0 first "hung" in our run. It computes (r2 - r1)/6 through a
  multiply by 0x2AAAAAAB, then counts up by 2 to it, so random inputs run
  for hundreds of millions of iterations. With a larger instruction budget
  for game programs it **matches** QEMU (668 s in the unoptimized test
  profile, against QEMU's under 60 s).
- All other functions match in both games: 14 of 14 runnable functions
  in Ultra Sun and 13 + 1 not re-run in Ultra Moon (0x005b43f8 needs the
  larger budget too).

### R-025 2026-10-25 — Kernel bring-up: the SVCs Ultra Sun makes first, and the decisions taken

Method: the probe runs until an SVC (or IPC command) is not implemented;
each one is then implemented from three sources and nothing else: the
game's own SVC wrapper (disassembled), libctru's `svc.s`/`srv.c` (read for
register conventions and message formats only, nothing copied), and
3dbrew's pages (SVC, Configuration Memory, Memory layout). In order:

| # | Call | Implemented as | Source |
|---|---|---|---|
| 1 | svc 0x21 CreateAddressArbiter | new handle in r1 | game wrapper = libctru |
| 2 | svc 0x02 QueryMemory ×2 (0x0FFFF000, 0x0FFBF000) | block or free gap: r1 base, r2 size, r3 perm, r4 state, r5 0 | libctru; the caller walks down the stack |
| 3 | svc 0x2B GetProcessInfo(current, 20) | 0x20000000 - LINEAR base = 0xF0000000 | 3dbrew SVC page |
| 4 | svc 0x2D ConnectToPort("srv:") | session handle | libctru; name read from the game's rodata |
| 5 | svc 0x32 SendSyncRequest: srv RegisterClient (0x00010002) | reply 0x00010040, 0 | libctru srv.c |
| 6 | read of 0x1FF80040 (APPMEMALLOC) | configuration memory and shared page mapped read-only | 3dbrew Configuration Memory, Memory layout |
| 7 | svc 0x38 GetResourceLimit, 0x3A CurrentValues(type 1 commit), 0x23 CloseHandle | see below | libctru enum, 3dbrew |
| 8 | svc 0x01 ControlMemory ALLOC 0x425000 at 0x08000000, ALLOC LINEAR 0x4184000 | mapped, zeroed, commit-checked | libctru `MEMOP_*` |

Facts read from the dumps for this (`usumctl inspect`): exheader kernel
version 2.50 (so the LINEAR base is 0x30000000 and the new memory
layout applies); Old 3DS system mode 3 (80 MB APPLICATION region); New
3DS system mode 1; the ARM11 kernel capability words.

Decisions (choices, not facts):
- The runtime presents an **Old 3DS retail unit**: APPMEMTYPE taken from
  the exheader's Old 3DS system mode (3: APPLICATION 0x05000000, SYSTEM
  0x01C00000, BASE 0x01400000), ENVINFO retail, UNITINFO Prod,
  RUNNING_HW product; kernel/FIRM version 2.58.0 (chosen; must not be
  older than the exheader's 2.50).
- Commit accounting counts code, data/bss, stack, heap and LINEAR
  allocations, not TLS (3dbrew: TLS comes from BASE). This is a reading
  of the kernel, not a measurement.
- LINEAR allocations are placed after the memory already committed, so
  each FCRAM page is used once; the real allocator is not reproduced.
- Handle values are not the real kernel's encoding (any non-pseudo value).
- Undocumented resource-limit values are not invented: only type 1
  (commit) is answered (limit = APPLICATION size).

Then execution reached `0x005014D8` with no translation: the R1 risk,
caught loudly. The caller (Thumb, 0x00100EF0) walks a table of 635
**self-relative** entries (0x00665F5C-0x00666948) and calls `entry +
*entry` for each, which is the compiler's static-constructor table. A new
seed kind (aligned words outside the text segment whose address plus
value lands in code) takes Ultra Sun from 29,269 to 31,545 functions and
from 80.9 % to 87.8 % of text decoded as code.

### R-026 2026-10-25 — Kernel bring-up continued: handles, the clock, sync objects; stable units

After R-025 the probe ran on through:

| Call | Implemented as | Source |
|---|---|---|
| svc 0x27 DuplicateHandle(CURRENT_THREAD) | a new handle to the same thread | game wrapper; 3dbrew pseudo-handles |
| svc 0x28 GetSystemTick | host monotonic clock × 268,111,856 Hz | libctru `SYSCLOCK_ARM11` |
| svc 0x13 CreateMutex, 0x15 CreateSemaphore, 0x17 CreateEvent and their release/signal/clear | objects in the handle table | 3dbrew SVC signatures; game wrappers |
| srv EnableNotification (0x00020000) | a semaphore handle (maximum not documented; any positive value) | libctru srv.c |

Supporting changes:
- Translated code stores the address of an indirect branch in
  `cpu.from` before taking it, so a `NoTranslation` stop reports where
  the jump came from as well as which pointers in the image name the
  target.
- Units are fixed address windows (ADR-0005 amended in practice): a
  discovery change no longer moves every function to another unit, so a
  rebuild recompiles only the windows whose code changed.

The probe then stopped at svc 0x08 CreateThread (R-027).

### R-027 2026-10-25 — Threads, waits, APT start-up, the first services

**Threads.** CreateThread (r0 priority, r1 entry, r2 argument, r3 stack
top, r4 processor; r1 = handle) is implemented with the scheduler of
ADR-0006: guest threads are saved `Cpu` contexts on one host thread.
3dbrew "Multi-threading" confirms the model this needs: priorities
0x00-0x3F (lower is more urgent; 0x18-0x3F for applications), "a thread
will be interrupted (preempted) if and only if a higher priority thread
is woken up", and "no timeslice-based scheduling is performed". The
game's first thread has priority 0x18 against the main thread's 0x30,
so it runs at once, as on hardware.

**Waits.** WaitSynchronization1 (r0 handle, r2:r3 timeout),
WaitSynchronizationN (r0:r4 timeout, r1 handles, r2 count, r3 wait-all;
r1 = index; register use read from the game's wrapper at 0x00107564),
SleepThread (r0:r1) and ArbitrateAddress (r0 arbiter, r1 address, r2
type, r3 value, r4:r5 timeout) types 0 SIGNAL, 1 WAIT_IF_LESS_THAN, 3
with timeout. Timeout result 0x09401BFE (3dbrew error codes). A wait
that is satisfied takes a semaphore count, mutex ownership or a one-shot
event's signal; releases wake the highest-priority waiter first. When
every thread waits and one has a timeout, the host sleeps until it.
Not yet: DECREMENT_AND_WAIT (types 2 and 4). 3dbrew says the value is
decremented "before the comparison"; that reading is to be checked
against the game's own lock code when it first uses them.

**Threads seen** (Ultra Sun): 2, priority 0x18, waits on srv's
notification semaphore (the notification handler); 3, priority 0x2F,
is the APT thread, waiting on APT's notification and resume events and
a third event of the game's own.

**APT start-up.** GetLockHandle (lock = a mutex; 3dbrew does not say),
Initialize (two events; the resume event starts signaled, as 3dbrew
says it is "triggered at start of execution"), GlanceParameter,
ReceiveParameter, Enable, NotifyToWait (a stub in NS per 3dbrew),
AppletUtility 7 UnlockTransition and 4 SleepIfShellClosed.
**Hypothesis:** an application finds a Wakeup parameter (command 1)
from the Home Menu (0x101) pending after Initialize. 3dbrew does not
say what is pending, and the game treats a failed Glance as fatal; its
dispatcher (0x00106E14) handles commands 2, 5, 8 and 9 specially and
carries on for any other. With a Wakeup the game continues; that is
consistent with the hypothesis, not proof of it. What NS replies when
nothing is pending is unknown, so that case stops the run.

**Settings table.** Commands that only hand over a setting are listed
one by one in `services::settings` (reply: the result; the value is
recorded): APT SetScreenCapturePostPermission, Enable, NotifyToWait;
ndm:u OverrideDefaultDaemons(0xF), SuspendDaemons(6); fs:USER
SetPriority(0). fs InitializeWithSdkVersion (SDK 0x0B0401C8) also
carries a process-ID descriptor and is handled in `services::fs`.

**Sessions opened**, in order: srv:, APT:A (several times), ndm:u,
cfg:u, ptm:u, fs:USER. The probe now runs about 0.4M instructions and
stops at fs:USER OpenFileDirectly on archive 3 (SelfNCCH: the game's
own RomFS).

Tool: `usumctl extract-code <dump> <file>` writes the decompressed
executable for disassembly with GNU objdump (refused inside a git work
tree, as it is game data).

### R-028 2026-10-25 — RomFS, cfg, DSP start-up: Ultra Sun reaches its first CRO

Ultra Sun now runs 2.94M instructions (an upper bound) on eight guest
threads. It reads 1.5 MB from its RomFS in 157 reads, starts audio, and
stops at `ldr:ro` Initialize: the CRO modules, NEXT_STEPS item 4.

**RomFS.** fs:USER OpenFileDirectly on archive 3 (SelfNCCH), empty
archive path, 12 zero bytes of binary file path, read-only. This is
the main RomFS, which 3dbrew says FS exposes as the level 3 image for
the process to parse. Reads are served from the user's dump in place
(`services::fs::Image`, positioned reads), never copied out. The
game's first reads are the level 3 header (0x28 bytes), then four ranges
at 0x28, 0xE4, 0x5C0 and 0x117C, i.e. the header's metadata tables. That
is a check that the image's offset is right: a wrong base would not
give offsets that chain this way.

**SD card.** The emulated console has one. Ultra Sun's code after
IsSdmcDetected (wrapper 0x0016C334) throws its own constant 0xC88044AA
(FS module, description 170) as a fatal error through err:f when no
card is reported. A first run with "no card" showed exactly that. A
mapping of SDMC paths to a per-user directory comes when the game opens
one.

**cfg.** `services::cfg::Console` holds the emulated console's settings
as explicit values: region from the recorded profile (USA → 1; the
`Region` field is new in usum-import's profiles), language English (1),
sound output stereo (1). SecureInfoGetRegion and GetConfigInfoBlk2 for
blocks 0x000A0002 and 0x00070001 so far. Unknown blocks stop the run.

**err:f.** ConnectToPort("err:f") is accepted. Any request on it stops
the run and prints the error words (that is how the SD case above was
caught).

**Memory mappings from the exheader.** The ARM11 kernel capabilities'
range descriptors (3dbrew "NCCH/Extended Header": paired
`0b11111111100x`, end exclusive, bit 20 read-only / static) are decoded
(`ExHeader::mappings`) and mapped at boot. Ultra Sun gets
0x1FF50000-0x1FF58000 and 0x1FF70000-0x1FF78000 read-write (the DSP
shared memory regions 3dbrew describes) and 0x1F000000-0x1F600000
read-only (VRAM).

**DSP (audio HLE, start-up only).**
- LoadComponent: accepted and identified, not run. The firmware is
  49,756 bytes; its hash is printed by the probe and not recorded here
  (game data).
- RegisterInterruptEvents, GetSemaphoreEventHandle, SetSemaphoreMask
  and SetSemaphore are implemented, as are FlushDataCache and
  InvalidateDCache (no-ops on coherent host memory).
- The game writes a u16 state 0 to pipe 2, raises semaphore 0x4000,
  waits on the pipe-2 interrupt event, then reads 2 bytes (a count),
  then count × 2 bytes. Its destination table has room for 15 entries,
  which are 3dbrew's 15 structures. It converts each address twice:
  `a` (region 0) and `a | 0x10000` (region 1).
- What the HLE answers is a **choice**. The firmware decides the
  addresses and 3dbrew does not list them, so ours are the structures
  in 3dbrew's order from DSP word 0x8000 (ARM 0x1FF50000), with
  3dbrew's sizes where given and 0x800 bytes otherwise; they fit one
  0x8000-byte region. ConvertProcessAddressFromDspDram uses 3dbrew's
  "typical" `(a << 1) + 0x1FF40000`.
- Not yet: audio frames. Thread 7 (priority 0x24) waits on the pipe
  interrupt for the next frame; nothing signals it, so no sound
  progresses.

**Requests so far:** 157 file reads, 144 DSP cache flushes, 30 DSP
address conversions, 15 GetServiceHandle (APT:A several times, ndm:u,
cfg:u, ptm:u, fs:USER, dsp::DSP, ldr:ro).

### R-029 2026-10-25 — GPU queue, VBlank, saves: Ultra Sun reaches its first CRO (LangSelect)

Ultra Sun now runs 20.4M instructions (an upper bound) on nine guest
threads, and the game itself handles them all. It reads 5.4 MB from
the RomFS, runs 22 GX commands (9 GPU command lists, 8,326 register
writes, no draws yet), sees 10 VBlanks and presents 4 framebuffers. It
stops at `ldr:ro` LoadCRO_New for **LangSelect.cro**, the language menu
a multi-language cartridge shows on a first boot with no save.

New since R-028:
- **CRO/CRS parsing** (`usum_import::cro`). Every module of both games
  parses: 133 each, 102,473 relocations, all of known types and inside
  their segments. Measured: every module has five segment entries,
  unused ones empty, at 0, of type 0. The CRS's segments are the
  executable's (text 0x00100000, data 0x00667000, end 0x006D4000).
- **RO Initialize and LoadCRR.** The CRS is mapped where the game asks.
  The address agrees with 3dbrew's rule (buffer − (0x08000000 − end of
  the executable)): 0x081DE000 → 0x008B2000. The buffer is moved
  (`Memory::remap`), and the blocks become Aliased/AliasCode. The CRR is
  made read-only, as 3dbrew says RO does; its RSA signature is not
  checked (the dump's hashes are verified at import).
- **Exheader mappings** decoded and mapped (R-028).
- **GSP.**
  - AcquireRight, RegisterInterruptRelayQueue (shared memory 0x1000; the
    game maps it with that size), SetLcdForceBlack, and the cache
    commands.
  - GX queue processing: FlushCacheRegions; ProcessCommandList, decoded
    by `pica` into register writes and raising P3D at FINALIZE;
    MemoryFill (16/24/32-bit, PSC0/PSC1); DisplayTransfer tiled to
    linear (8×8 Z-order tiles, crop, flip, the five formats; PPF).
    Scaling, texture copy and DMA are not implemented yet.
  - VRAM is written as a device (`Memory::device_write`): the process
    maps it read-only.
- **Hardware timeline.**
  - VBlank every 16,713,680 ns (about 59.83 Hz; the exact rate is not
    established). At VBlank GSP takes new framebuffer settings from
    shared memory and queues PDC0 and PDC1.
  - An audio frame every 4,888,780 ns (160 samples at 32,728 Hz; the
    rate is an assumption) signals the DSP pipe event.
  - Events wake waiters and preempt when they wake a more urgent thread.
  - The first presented framebuffers are RGB8, stride 0x2D0 (240 × 3).
    Their pixels are uniform: nothing is drawn yet. That is not a frame.
- **Idle sleeps.** One run slept for 90 minutes: the host followed the
  game's timeouts (ADR-0006), with every thread waiting on hardware
  that did not exist yet. The probe now caps total sleep at 2 s and
  reports it.
- **Discovery.** A new seed: `ldr rX, [pc, #n]` then `add rX, pc` (the
  Thumb PIC idiom for a function pointer; the game's sscanf passes its
  getc that way). It adds 17 functions.
- **cfg.** Blocks 0x00050005 (stereo constants as 3dbrew lists them),
  0x000B0000-3 (country 49, United States; English name in each
  language slot; no state), 0x000A0000/1 (user name "Player", birthday
  1/1), 0x00030001 (user time offset 0), 0x000C0000/1 (parental
  controls off). Also SecureInfoGetRegion, GetRegionCanadaUSA (3dbrew's
  rule) and GenHashConsoleUnique (3dbrew's construction over a fixed
  console value: a choice).
- **frd** (offline, no account), **boss** (storage registration kept,
  nothing run), **hid** GetIPCHandles, **ndm** daemons.
- **Settings tables**: a second one for result-only commands whose only
  translate parameter is the process ID (frd SetClientSdkVersion, boss
  InitializeSession).

Save data, first boot. The probe's save root is a scratch directory
beside its binary, emptied every run: never the user's saves. Results
are **hypotheses** from the game's own handling:
- Save data (archive 4) unformatted: GetFormatInfo and OpenArchive
  return 0xC8A04554 (FS, description 340). Ultra Sun's save code
  (0x00455C58) treats descriptions 340-359 as a state of its own, not
  an error.
- Missing extdata (archive 6, SD, ID 0x1B50): 0xC8804478, 3dbrew's
  "Object doesn't exist" (description 120). The game groups FS
  descriptions 100-179 (the most common range check in its code: 163
  module-17 checks were surveyed). Given that, it carries on without
  creating the extdata.

### R-030 2026-10-25 — CRO modules load, link and run translated (LangSelect), first draw calls

**RO, measured.** All 102,473 relocations of both games (133 modules
and the CRS each) are R_ARM_ABS32. Inside code segments they are
literal-pool words. RO therefore never rewrites an instruction, which
shaped ADR-0007 (accepted): modules are translated position-
independently at virtual bases.

**RO HLE** (`usum_runtime::ro`):
- LoadCRO_New maps the module where 3dbrew's rule says (LangSelect: heap
  0x08009000 → 0x006DD000), puts .data and .bss into the game's buffers,
  applies internal relocations, links imports in both directions (named,
  indexed, anonymous), and binds unresolved imports to OnUnresolved.
- LangSelect resolves all 169 of its imports; the CRS resolves 2 of its
  536 against it.
- **Hypothesis:** RO rebases the mapped header's offsets to addresses
  (and the segment table to segment addresses, magic "FIXD" for fix level
  1). After LoadCRO, Ultra Sun's loader (0x00172FF4) reads header field
  0xB8 (.data offset) and uses it as an address in the mapped image.
  Without the rebase it copied from 0x0000AFF4 and faulted.
- That loader also shows what "size after fixing" means to the game:
  the part of the buffer past it stays readable at its heap address and
  is copied from there. We answer the full size, so all of it is mapped.

**Module translation.**
- `usumctl translate` now also translates the 132 CROs: 60,178
  functions, 2.69M emitted instructions, 201 crates, about 11 minutes in
  release for everything together.
- Seeds come from the module tables (exports, entry points, internal
  relocations into text) and from every anonymous import of the module.
  The CRS's anonymous imports matter: the first run missed LangSelect
  0x3BA8, which only the executable imports.
- **The executable gains 3,539 functions** (31,575 → 35,114) from the
  CRS's exports and the anonymous imports of `|static|`. That is code
  that nothing in the executable itself points to, called only from
  modules. The first such stop was 0x002EE724.

**Result.** With LangSelect translated and dispatched through
`module_delta`, Ultra Sun runs 30.4M instructions. The GPU has seen 130
GX commands, 69 command lists, 27,058 register writes and **14 draw
calls**; 35 VBlanks and 20 presented framebuffers. Nothing is
rasterized yet, so those frames are not pictures.

### R-031 2026-10-25 — First-boot save setup, extdata, and the main loop at the language menu

After R-030, the game carries out a first boot's save setup on its own:

| Step | Request | Implemented as |
|---|---|---|
| format the save | FormatSaveData(4: block 0x200, 1 dir, 1 file, 3+3 buckets, duplicate) | directory emptied, format recorded beside it |
| open it | OpenArchive(4) → handle | archive table |
| clean up | DeleteFile("/main") → not found | 0xC8804478 |
| create | CreateFile("/main", 0x6CC00) | zero-filled host file |
| write | 40 Write calls, 435,466 bytes | host file, positioned writes |
| commit | ControlArchive(action 0) ×2 | no-op (writes are already on the host) |
| extdata | CreateExtSaveData(SD, 0x1B50, 15 dirs, 682 files, icon 0x36C0) | directory, icon and format beside it |
| | GetFormatInfo, OpenDirectory("/"), Read (empty), CreateDirectory | host directory |
| files | OpenLinkFile, SetPriority | another session; result only |

Choices and hypotheses:
- **Hypothesis:** the save archive's total size in GetFormatInfo is the
  exheader's save data size (0xFE000). FormatSaveData carries no size,
  and 3dbrew says the save's identity comes from the exheader.
- **Choices:**
  - Commit atomicity is not modelled: writes land at once. A real save
    store needs commit semantics, decided before saves are kept for the
    user.
  - Directory entries' 8.3 names are truncated upper-case forms.
  - Files are marked "archive".
- **Path safety:** guest paths (ASCII or UTF-16, absolute) map to host
  paths under the archive's own directory. ".", "..", empty components,
  NUL and backslash are refused. Format records and icons live beside
  the archive directory, never inside it, so no game file name can
  collide with them.

**Result: the main loop.** Ultra Sun now runs 61.3M instructions in 4.6 s
of wall time, up to the probe's 2 s idle cap. In that time: 269 VBlanks,
944 audio frames, 1,072 GX commands, 543 command lists, 761,336 GPU
register writes, **6,485 draw calls**, and 176 presented frames. Every
thread waits between frames; two worker threads (10, 11) have exited.
LangSelect is the module loaded; the game made 502 RomFS reads in
all. This is the language menu of a first boot, waiting for input that
does not exist yet. Nothing is rasterized, so no frame is a picture
yet.

### R-032 2026-10-25 — A software PICA200 renderer: the first correct frames of Ultra Sun

`usum_runtime::pica` now renders. The layers:
- **`shader`:** an interpreter of the PICA shader ISA from 3dbrew
  "GPU/Shader Instruction Set". It covers formats 1/1i/1u/1c/2/3/5/5i,
  swizzles and negation, relative addressing on constants only, the IF,
  CALL and LOOP stacks, and CMP with its conditions. Arithmetic is f32
  with 3dbrew's special cases (0 × x = 0 unless NaN, GLSL MIN/MAX,
  rcp/rsq(0) = +inf); the hardware's 24-bit precision is an
  approximation not modelled. Code, operand descriptors and uniforms
  arrive through the FIFO registers; the geometry shader is not done.
- **`draw`:**
  - Vertex loading: 12 loaders, the 3dbrew "GPU/Pitfalls" alignment
    rules, fixed attributes, index buffers.
  - The output map: enabled output registers consume map slots in order.
  - Triangles, strips and fans; clipping against w > 0, |x|, |y| <= w
    and -w <= z <= 0; viewport and depth.
  - Rasterization with perspective-correct interpolation.
  - The six TEV combiner stages with the combiner buffer; alpha test,
    depth test and writes, blending, write masks.
  - 8×8 Z-order tiled color and depth buffers.
- **`texture`:** units 0-2, all fourteen formats including ETC1/ETC1A4,
  the four wrap modes, nearest and bilinear filtering.

**Findings against the documentation, from the game's own data:**
- **float24 uniforms** (and fixed attributes) are one 96-bit number with
  the *first word most significant*, holding w, z, y, x from the top.
  This is the layout of 3dbrew's GPUREG_FIXEDATTRIB_DATA0-2 table. The
  reading first taken from the uniform page's "ZZWWWWWW" diagram was
  wrong. Ultra Sun's c5, uploaded as 0x3E000040 0x00003F00 0x00000000,
  decodes to exactly (0, 1, 2, 0.5) this way and to 1.000946 the other.
  With the wrong reading, every one of 147,732 shaded vertices had
  w <= 0, so nothing was drawn.
- **float32 uniforms** are in reverse order, as 3dbrew says (first word
  w), and selector 0x1B is .xyzw, as 3dbrew says. Nintendo's compiler
  reads uniforms with .wzyx (0xE4), consistent with an SDK that uploads
  x first. A disassembly of the language menu's sprite shader under
  these rules is coherent: an orthographic projection for the rotated
  screen in c0-c3. A trial with float32 in forward order gave positive w
  but meaningless positions, and was reverted.
- **Texture choices** 3dbrew does not settle, now supported by the
  game's frames:
  - memory row 0 is a texture's top;
  - textures use the same 8×8 Z-order tiling as framebuffers;
  - 4-bit texels have the even texel in the low nibble;
  - ETC1 blocks are four 4×4 blocks per tile in Z order, as
    little-endian u64s, with ETC1A4's alpha column-major.
  - Rendered rows count from the framebuffer's bottom.
  The language menu's text and ETC1 artwork come out right with these;
  other formats are not yet exercised by anything checked.

**Result.** The probe's presented frames show Ultra Sun's first-boot
language menu correctly:
- the top screen's artwork, the "Play the game in" header and the
  language list with its cursor;
- the bottom screen's instructions in white and red text.
Those are the first frames of the game produced by this project. They
are game-derived images and stay in the private cache, never in the
repository. The user, who knows the game, looked at both and judged
them right (2026-10-25). That is a visual check, not a pixel
comparison; a capture from the user's own console is still to come.

**Cost.** Rendering in software on one thread makes the run take 36 s
of wall time for about 4.5 s of game time: 81.7M fragments over 48,000
triangles. This is the next bottleneck.

Debugging aid: `USUM_GPU_TRACE=n` prints the first n draws' state,
shaded vertices and an instruction-level trace of one shader run.

### R-033 2026-10-26 — Input, a virtual clock, faster rendering: from the language menu into the intro

**Input.**
- Buttons go into HID shared memory as 3dbrew "HID Shared Memory" lays
  it out: header ticks, latest index, current PAD, and 8 ring entries of
  current/pressed/released/circle pad. The touch section is advanced
  with empty entries.
- The PAD events are signaled at each VBlank. The real update rate is
  not established; once per VBlank is a choice.
- The process maps this memory read-only, so HID writes as a device.
- Input comes from a script, `USUM_INPUT="400=A,410=,700=A,710="`
  (buttons held from a VBlank on), for repeatable runs.

**Virtual clock.**
- The probe now runs on a virtual clock: time advances one ARM11 cycle
  per instruction executed (a simplification), and jumps to the next
  event when every thread waits, instead of the host sleeping.
- GetSystemTick, timeouts, VBlank, audio frames and HID all use it.
- Before this, wall time drifted against game time (rendering was ~8×
  slower than real time), so a scripted press landed before the menu
  existed.
- Two runs of 8 game seconds now produce identical frames and counts.
  GetSystemTick still read the host clock until it was found by this
  comparison.

**Renderer speed.**
- The texture configuration is resolved once per draw (`Bound`, reading
  texels from a borrowed slice of guest memory).
- The framebuffer is copied out once per draw, only the tile rows the
  triangles touch, and rasterized in parallel bands of tile rows on all
  cores. Each band sees every triangle in order, so per-pixel draw order
  is kept.
- `Memory::device_write` gained a fast path. It had checked every byte
  separately, and that per-byte check was most of the remaining time.
- 8 s of game time: 93 s of wall time → 13.8 s, with identical frames
  and fragment counts. Rasterizing is still about 75% of the time.

**GPU, continued.**
- Command buffer jumps (GPUREG_CMDBUF_ADDR/SIZE/JUMP0-1, 3dbrew): a list
  that ends in a jump continues in the other buffer.
- The geometry shader, implemented from 3dbrew:
  - EMIT/SETEMIT with a 4-vertex buffer, winding and primitive flags;
  - the geometry unit's registers at 0x280;
  - VSH_COM_MODE 0 copies vertex shader configuration to it.
  - **Hypothesis:** the geometry shader's input buffer holds each
    vertex's enabled output registers in order, and one run takes as
    many vertices as its input count covers. 3dbrew says only that the
    input buffer is "the vertex shader output buffer". It has not been
    exercised yet: the draw that first used primitive mode 3 had no
    geometry shader.
- **Inference:** primitive mode 3 without a geometry shader, with
  GEOSTAGE_CONFIG bit 8 ("drawing triangle elements") set, is a triangle
  list. This is what 3dbrew's "Drawing elements" sequence sets for
  indexed triangles.
- GX TextureCopy (3dbrew: a raw copy of `size` bytes in lines with gaps,
  widths in 16-byte units).

**RO, continued.**
- UnloadCRO (0x000500C2: mapped address, 0, buffer).
- The memory moves back to its heap buffer; imports resolved into the
  module go back to OnUnresolved.
- **Hypothesis, now supported from both sides:** RO restores the
  header's offsets on unload as it rebased them on load. After
  UnloadCRO, Ultra Sun adds the buffer address to a header field it
  reads back (0x001731D4), so it expects an offset there again.
- Memory regions and kernel blocks that touch and match are merged
  again, so a later module buffer can span earlier seams.

**Where the game is.** With A pressed every two seconds after the menu:
- the language confirmation, "Your settings have been saved.", and
  "You have a message from the Alola region's own Professor Kukui!";
- LangSelect unloads and **AppDemo** (the intro) loads, linking 54 of
  its 70 imports;
- the next module load is what this entry left in progress.
The user judged the first frames (the language menu) right on sight
(R-032).

### R-034 2026-10-26 — Through the intro to name entry: RO details, system data, a table-return helper

Scripted input now carries Ultra Sun from the language menu through
Professor Kukui's video call (3D, rendered), character selection and
name entry, into the next story scene. Along the way:

- **RO: import status (3dbrew-documented).** RO sets byte 6 of the first
  entry of each import's relocation list to 1 once it resolves ("set to
  1 on the first entry if all symbols loaded"). Ultra Sun's module
  manager checks this after loading with linking (0x00173A60, called
  from 0x0034D0C4) and **panics** (the SDK panic at 0x001074BC) if any
  list is unmarked. That was the halt after StrInput loaded. The runtime
  now marks and unmarks it in the module's image, the CRS's included.
- **RO: .data relocations go into the image.** After LoadCRO the game
  copies .data from the mapped image into its own buffer (0x00172FF4).
  Relocations applied to the buffer were overwritten by unrelocated
  data, which left a null function pointer in SequencePlayer. They are
  now patched in the image, while references *to* .data still use the
  buffer address.
- **Finding the panic.** A watch on direct calls (`USUM_WATCH=0x1074bc`,
  in `usum_guest::call`) reports the return address of the last call to
  a target; it named 0x0034D0F8 at once. A heuristic stack scan in the
  probe lists code addresses on the stack.
- **System data.** The name entry opens title 0x000400DB00010302 (3dbrew
  "Title list": the NGWord bad-word list) through archive 0x2345678A
  ("title access"). That is console system data, not in the game dump,
  and must never ship. It is answered "not found" (0xC8804478). The game
  carried on: it rejected the name "!!!!!!!!!!!!" ("You can't enter
  that name.") and accepted "aaa". Whether the real list changes what
  names are accepted is open. Support for the user's own system data,
  supplied by them, is a later option.
- **Discovery: a helper that returns into a table.** ARM code at
  0x005B9590 starts `ldr r4, [lr], #4`: it reads a word placed after
  the call, then returns to `lr + index × 4` (`add ip, lr, r4, lsl #2;
  bx ip`). This is the compiler's floating-point classification idiom.
  Its callers continue at call + 8 + 4k (k = 0..3). Discovery now
  recognizes a BL to a function that starts by loading through `[lr],
  #4` and follows those four successors instead of call + 4. The first
  miss was 0x005B98C0.
- **Renderer:**
  - Per-row spans: the same output, little faster, since shading dominates.
  - A per-draw combiner plan: pass-through stages are skipped, and only
    textures some stage reads are sampled.
  - Rasterizing stays the cost: about 0.8 µs per fragment per thread.
- **Discovery: halfword-aligned pointers.** After name entry, the next
  scene called 0x004C5450 through a pointer the game took from a packed
  table in its data at 0x005E749E: a word at a 2-byte-aligned address,
  which the 4-aligned scan never sees. Discovery now also takes absolute
  pointers from halfword-aligned words outside the code. That adds
  1,207 functions (87.8% → 88.7% of text decoded). Some are surely false
  seeds, mostly Thumb from odd values, which cost translation size, not
  correctness.
- **Renderer:**
  - A decoded-texture cache across draws, keyed by address, format and
    size, reused only while the texture's bytes are unchanged (compared
    with a stored copy).
  - `Memory::read_range` takes one slice when it can.
  - The 70 s intro run: 126 s → 102 s, every snapshot byte-identical to
    the version before (A/B compared).
- **GX RequestDMA (command 0).** First used once the overworld modules
  were loaded. Implemented as a copy of `size` bytes from the source
  address to the destination as a device write (the destination can be
  VRAM or read-only guest memory), followed by the DMA interrupt.
- **Renderer:** `encode_pixel` no longer allocates (it ran per fragment;
  malloc/free was about 2.7% of the profile).
- **GSP SetAxiConfigQoSMode (0x00100040), argument 0x203.** Sent
  after the overworld loaded. Treated as result-only: no bus contention
  is modelled.
- **DisplayTransfer, tiled to tiled with a 2x2 downscale (flags
  0x02000020: RGBA8 → RGBA8).** Implemented following 3dbrew's
  transfer-engine bits:
  - bit 5 is tiled-to-tiled;
  - bits 24-25 are a box-filter downscale (2x1 or 2x2).
  The output holds input ÷ scale pixels per axis, capped by the output
  dimensions given. Averages round to nearest; that rounding is a
  choice, since the hardware's rounding is not documented. Linear-to-tiled
  and 32x32 tiling are still errors.
- **NDM EnterExclusiveState (0x00010042), state 2 (local
  communications).** Sent as the overworld starts. Treated as
  result-only: no daemons run to be held off. On the way there:
  - VBlank ~10,800: the title screen renders;
  - VBlank ~12,000: the aerial view of Melemele Island.
- **Local wireless at the overworld's start.** The order:
  1. NDM EnterExclusiveState(2).
  2. `CreateMemoryBlock` over 0x14000 bytes of the game's own memory, with
     my permission 0 and other permission 3. Implemented for a nonzero
     address: the block is the caller's memory, already mapped.
  3. srv GetServiceHandle("nwm::UDS").
  4. cfg block 0x000A0000 (user name).
  5. cfg block 0x00090000, the current local friend code (3dbrew "Config
     Savegame"). Answered 0, because no friends account exists; that is
     a choice that matches frd GetMyFriendKey.
  6. nwm::UDS InitializeWithVersion(0x14000, NodeInfo, version 0x400,
     the block). Answered with success and a status event that is never
     signalled: no other console is ever in range.
- **The game hosts a local network.** After SetProbeResponseParam
  (0x00210080, result-only), the game calls nwm::UDS CreateNetwork2 to
  begin hosting a UDS access point. This is Ultra Sun's passive local
  presence, which starts as the overworld loads.
  - Modelled as a host that no one joins:
    - status 6 ("connected as host");
    - node 1 the only node;
    - maximum nodes from the NetworkStruct (offset 0x1D).
  - The status event fires, as for any status change. That is a choice:
    3dbrew does not say whether hosting signals it.
  - GetConnectionStatus returns the 0x30-byte structure (services/uds.rs).
- **Host, then scan.** After about 69 VBlanks of hosting, the game calls
  DestroyNetwork and then StartScan (0x000F0404): it looks for other
  consoles' beacons with a 0x4000-byte buffer.
  - DestroyNetwork goes back to idle and fires the status event.
  - StartScan writes the 3dbrew BeaconDataReply header with no entries:
    the max size, 0xC written, 0 entries.
  - The scan's event is signalled right away, since the scan finishes
    at once. That is a choice.
  - Finalize turns UDS off.

### R-035 2026-10-26 — Into the story: local wireless, and the DSP keeps time

- **Where it got to.** The probe passes the title screen (VBlank
  ~10,800) and the aerial view of Melemele Island (~12,000), then
  reaches the player's bedroom (~15,000): Meowth wakes the player. The
  first dialog box, "Mwaaawwwrrrrr!!!" (Meowth's cry), then stayed from
  VBlank ~18,000 to ~32,000. A was pressed every 120 VBlanks, and
  only Meowth's idle animation changed.
- **Diagnosis, measured.** Dumping both DSP regions at the stuck
  dialog (the new `USUM_DUMP`) shows:
  - Ultra Sun configures 22 of the 24 sources, and it increments both
    regions' frame counters itself (63,414 and 63,413 after 63,410
    audio frames; region 0 even, region 1 odd).
  - Every source status is zero: the HLE DSP never wrote one, so no
    sound ever started or ended.
  - The text box waits on the cry's sound (hypothesis, tested below).
- **Word order, measured from the game's writes:**
  - Addresses and sample counts are two little-endian halfwords, high
    first: `96 23 e0 f1` is 0x2396F1E0, in FCRAM, and `00 00 00 38`
    is 0x3800 samples.
  - The rate multiplier is a plain little-endian float (`00 00 80 3f`
    is 1.0).
  - The dirty flags read as a plain little-endian u32. Every flag set
    seen fits named bits: 0x00010000 is enable; 0x20010000 is enable
    and reset; 0x0E010000 is enable and the three gains; 0x7E6F0014 is
    a full voice start (ADPCM coefficients, enable, interpolation,
    rate, buffer queue, play position, filters, gains, sync, reset,
    embedded buffer).
  - Format 0x0009 is mono ADPCM.
  - The game only sets the flags; the dirty bits stay set until
    something clears them.
- **Not documented on 3dbrew, taken from Citra's HLE (read as
  documentation; no code used):**
  - the dirty bit assignments;
  - that the DSP reads the configurations from the current region
    (the higher counter) and writes the statuses into the other one;
  - that the DSP clears the dirty flags;
  - that buffers play lowest ID first.
  The bit assignments agree with every flag set above.
- **Implemented (`audio.rs`): the DSP's frame, timing only.** Each
  audio frame:
  - Reset clears the source.
  - Enable sets it on or off.
  - The embedded buffer, and the queued buffers named by the queue's
    dirty bitmap, are queued by ID.
  - Play advances 160 × rate samples, buffer by buffer; a looping buffer
    restarts.
  - The status is written: enabled, the "a later buffer started" flag,
    sync, position, current buffer ID.
  - A source whose buffers have all played disables itself. That is how
    the end of a sound is signalled; it is a choice, to be checked
    against the game's behaviour.
  Samples are not decoded or mixed yet.
- **Result: the DSP frame works, but the dialog still sticks.** With
  statuses written, the per-source log (`dsp source` lines in the probe
  report) shows ordinary audio:
  - The streamed music (sources 0 and 1) plays 14,336-sample ADPCM
    buffers in about 90 frames each (14,336 / 160 = 89.6), and the
    game queues the next buffer (dirty bit 19 with the queue bitmap)
    as each finishes.
  - A three-voice sound with per-frame pitch changes (rate and
    interpolation dirty, 0x00060000, every other frame) starts at
    audio frame ~55,000. That is VBlank ~16,090, before the dialog
    appears (~16,800); likely Meowth's cry. One of its voices is reset
    by the game after three frames.
  - Idle sources get periodic gain updates (0x0E010000, then
    0x00010000).
  The frames after the dialog appears are byte-identical with and
  without the DSP frame, so whatever holds the dialog does not read
  source statuses. Reporting the last finished buffer ID at status +10
  made no difference either (kept: harmless, and it matches Citra).
  Next: test input (B presses; fast A taps) at the dialog.
- **Input is not what holds it.** Two runs from the same state, one
  pressing B every 60 VBlanks and one tapping A every 30, produced
  byte-identical frames. The earlier dialog boxes each closed right
  after an A press: the box at VBlank 14,131 at the press at 14,360;
  the box at 15,241 at the press at 15,320. This box (from ~16,381)
  ignores A.
- **The cry is the dialog's sound, timed exactly.** Probe reports now
  stamp each IPC with its VBlank, and `usumctl romfs <dump>
  [offsets]` names the RomFS files at given image offsets.
  - At VBlank 16,347 the game reads `data/sound/niji_sound.bcsar` (the
    sound archive) and two GARC files (a/0/4/2, a/0/7/4), then flushes
    the sound data for the DSP. That VBlank is audio frame ~55,888.
  - Source 18 starts its 20,567-sample sound at audio frame 55,893.
  - The box appears at ~56,004.
  - By our timing the sound ends at 56,014. The game never touches
    source 18 again, and its only file reads after that are the music
    stream (`bgm_nj_e11_02.dspadpcm.bcstm`).
  So the box waits for the cry, and the game does not take our status
  as "ended".
- **Next hypothesis:** at the end, report `current_buffer_id_dirty` = 1
  and `current_buffer_id` = 0, besides `is_enabled` = 0. This is how
  Citra's HLE reports a source running out of buffers, as recalled;
  not yet checked against its source.
- **Confirmed: that is how the end of a sound must be reported.** With
  `current_buffer_id_dirty` = 1 and `current_buffer_id` = 0 at the end:
  - The cry (now on source 2: 20,567 samples from audio frame 55,890)
    ends at 56,047.
  - The game resets that source on the next frame (0x20010000).
  - The dialog closes and the scene moves on: the player asleep on the
    bed, the Pikachu doll beside them.
  The periodic gain updates on idle sources seen before (0x0E010000,
  roughly every 128 frames on sources 8, 11, 12, 15, 16, 17) stop. They
  were the game still updating sounds it believed were playing.
- **Next stop:** that shot holds from VBlank ~16,800 to at least
  23,400. Between snapshots only ~700 pixels change (the player
  breathing, Meowth idling). After the cry no sound starts, and the
  only file reads are the music stream. Input tests follow.
- **The new wait polls an input device that never sees our presses.**
  - Profiling guest calls (`USUM_PROFILE`) over VBlanks 20,000–20,060
    and over 16,350–16,410 differs, in FieldRo, by three calls that
    only the idle window makes. Each comes once per game frame, from
    FieldRo 0x006F5ABC.
  - That function returns `F(G(H(), 0)) != 0`. The imports resolve (from
    the stub words, dumped at run time) to:
    - 0x00104A40: returns the global `*0x006A5518`;
    - 0x004B4A70: `obj->[0x18 + 4 × index]` for index < 3, here index 0;
    - 0x004B4474: field 12 of the state that 0x001054DC selects
      (offset 0x30, 0x34 or 0x44, by two flag bytes).
  - The neighbouring code turns stick floats into direction bits. So
    0x006A5518 is plausibly the input manager, the scene waits for
    device 0 to report something, and A, B and held A did not do it
    (byte-identical frames).
  - Next: dump the manager, the device and its states during an A
    press, with the new pointer syntax in `USUM_DUMP`.
- **Device 0 is the circle pad.** Dumped at VBlank 18,128 (inside an A
  press):
  - The manager (`*0x006A5518`, class from its C++ type info:
    `gfl2::ui::CTR_DeviceManager`) has one device, at +0x18; slots +0x1C
    and +0x20 are null.
  - That device's class is `gfl2::ui::VectorDevice`. Its selected state
    is all zeros, while HID shared memory shows A held in every ring
    entry.
  - The neighbouring code turns stick floats into direction bits.
  So the idle shot likely waits for the player to move: the game polls
  the circle pad's state at field 12 (likely a trigger), and the input
  scripts could not move the stick.
  - Added circle-pad input: `CUP`, `CDOWN`, `CLEFT` and `CRIGHT` set PAD
    bits 28-31 and the stick position, full deflection ±156 (a choice:
    3dbrew does not give the range). The position goes as two s16 into
    each ring entry (+0xC) and the raw field (+0x20).
  - The game never sent HID EnableAccelerometer or EnableGyroscope; its
    only HID request is GetIPCHandles.
- **Confirmed: the player has control.** With the stick pushed down,
  up, left and right in turn from VBlank 17,000, the player gets out of
  bed and walks around the bedroom, and the camera follows. After an A
  press beside the bed, a "......" message appears. The D-pad moves
  nothing (frames identical to no input). So the overworld runs: the
  "stall" after the cry was the game waiting for the player.
  The circle-pad range (±156) is still a choice; the game accepted it.

### R-036 2026-10-26 — A window to play in

- **`play`**, generated beside `probe` by `usumctl translate`, opens a
  window (winit, softbuffer; audit in DEPENDENCIES.md, all Rust and no
  compiled C) and runs the game live:
  - Screens: the top screen above the bottom one (400×480 canvas), at
    the largest whole scale that fits.
  - Keyboard: arrows for the circle pad, X/Z for A/B, and so on
    (BUILDING.md). The window writes the held keys as PAD bits to the
    kernel's `live_pad`, which takes the input script's place.
  - Game thread: runs the same loop as the probe (`run_with`, a hook at
    each budget refill), with a 256 MiB stack (translated calls recurse
    on the host stack).
  - Frames: at each new VBlank the hook copies the newest framebuffer of
    each screen out (`framebuffer_rgb`) and wakes the window.
  - Clock: the virtual clock, throttled so the game never gets ahead of
    real time. A slow host gives slow motion, not dropped frames or
    skipped game logic.
  - Saves: `$XDG_DATA_HOME/usum-recomp/saves/<program ID>/`, apart from
    the dump and the generated code.
- **Checked:** the window's own pixels, captured with
  `USUM_PLAY_SNAPSHOT`, show the first-boot language menu, both screens
  correct, at 2×; the game created its save data in the data directory
  (redirected to a scratch directory for the test). Keyboard input
  cannot be pressed from the agent's session: untested beyond the
  shared PAD path the scripted runs use.
- **Speed:** the renderer runs this part of the game at about 0.55× real
  time (300 s of game time in ~540 s), so play is in slow motion until
  the renderer gets faster (NEXT_STEPS).

### R-037 2026-10-26 — Renderer speed: early depth, a worker pool, a decoded plan

Measured on the 70 s intro scenario (name-entry input script, 101,092
draws, 1.5 billion fragments). Every snapshot was compared byte for byte
with a baseline taken before the changes: all 14 identical after each
step.

| Change | Wall time |
|---|---|
| baseline | 102 s |
| depth test before texturing and combining; the stored color decoded only for blending or masked channels | 93 s |
| distinct vertices shaded in parallel (threads started per draw) | 91 s |
| a rayon pool for bands and vertices instead of threads started per draw | 78 s |
| combiner stages, buffer and blend registers decoded once per draw (`TevPlan`) | 76 s |
| bands from 1,024 pixels of area (was 4,096; 256 gains nothing more) | 70 s |

- Early depth is exact: the depth test's outcome does not depend on the
  fragment's color, and a failing fragment changes nothing (no stencil
  yet). The alpha test still comes first for writes: a fragment that
  passes depth but fails alpha writes nothing, as before.
- Starting OS threads per draw cost more than it saved. 101,092 draws
  each started up to 16 threads. rayon is pure Rust (DEPENDENCIES.md).
- Profile after (samples over all threads): `raster_band` 58%, texture
  sampling 18%, the vertex shader 8%. Remaining ideas: per-fragment
  edge/perspective arithmetic would change float results (needs an
  accuracy decision, not just A/B); texture sampling; overlapping the
  GPU with the CPU (the CPU part is only ~12 of the 70 s).
- Kernel profiling is unavailable (perf_event_paranoid 2, a system
  setting left as is).
- End to end, 300 s of game time to the bedroom: 534 s → 380 s (0.55× →
  0.79× real time).

### R-038 2026-10-26 — The touch screen, and what HID's ticks mean

- **Touch input:** scripts take `Tx:y` (bottom-screen pixels), and the
  window maps the left mouse button over the bottom screen. HID writes
  each entry of the touch ring as x, y (u16 pixels) and a u32 whose low
  byte is 1 while touched, all zero otherwise (3dbrew). The raw entry at
  +0x18 gets the same pixel values (a choice: 3dbrew does not give the
  raw units).
- **First test, on the name-entry keyboard:** taps of 1, 2, 4 and 60
  VBlanks each typed their key twice; an 8-VBlank tap typed it three
  times. So it was not key repeat: every touch-down was seen twice.
- **Cause:** 3dbrew describes the section's tick fields as the time HID
  updated entry 0 of the ring, with the previous such time at +8. We
  wrote both ticks at every update. A reader that infers the update
  rate from them would conclude entries arrive about 8× faster than
  they do, read older entries again, and see each transition twice.
  Buttons were unaffected because PAD entries carry explicit
  pressed/released bits.
- **Fix:** both sections (PAD and touch) now change their ticks only
  when the index wraps to 0. After it, taps of 1, 8 and 60 VBlanks each
  type exactly one letter ("qwy"). How the SDK actually reads the ticks
  is inferred from this behaviour, not read from its code.

### R-039 2026-10-26 — Sound

- **Mixing (`audio.rs`).** Each source's buffers are decoded when they
  start playing:
  - PCM8 and PCM16, mono or stereo.
  - DSP-ADPCM, the GameCube/Wii format: 8-byte frames of a header (the
    coefficient pair in bits 4-6, the scale exponent in bits 0-3) and
    14 nibbles. The source's 16 coefficients come from the ADPCM
    coefficient structure when dirty bit 2 is set. The history carries
    from buffer to buffer unless a buffer brings its own, which keeps
    streams continuous.
  Sources play at their rate with linear interpolation, and are mixed
  by their gains for intermediate mix 0 (front left/right) into 160
  stereo samples per frame, at 32,728 Hz. Mixes 1 and 2 (effect sends)
  are left out rather than added dry, a choice. Filters and the master
  volume are not applied yet.
- **Found by listening to the numbers:**
  - The first WAV (the probe's new `USUM_WAV`) was noise: ~14,000 zero
    crossings a second at 32,728 Hz. `USUM_DSP_TRACE` showed every
    buffer decoded as format 0 (PCM8) while its bytes were plainly
    ADPCM: valid headers, coefficients loaded, and the file is
    `…dspadpcm.bcstm`.
  - Ultra Sun never sets dirty bits 0 or 1. Every flag set seen in 300
    s is one of 0x00080000, 0x00060000, 0x0E000000, 0x0E060000,
    0x20010000, 0x5E670014, 0x5E2F0010, 0x0E470004, or zero. It always
    sends the format with the embedded buffer (bit 30), so the format is
    now taken then too.
  - After that, 1,000-3,400 zero crossings a second, RMS 1,600-4,300, no
    clipping (peak 22,839): music, numerically. Only a listener can say
    whether it is right.
  - Fields are read only when their dirty bit is set (format,
    coefficients, rate, gains). The two regions hold different, partly
    stale copies (R-035's dump showed a rate of 0 in the region the
    game had not just written).
- **Output (`sound.rs`, the window only).** The PulseAudio native
  protocol in pure Rust (crate `pulseaudio`) over the session's socket,
  which PipeWire also serves. No C, unlike cpal with ALSA, whose build
  needs ALSA's development files. The server clocks the stream; it gets
  what the game has mixed, at most a quarter of a second queued, and
  silence when the game is behind. A 20 s run of `play` sent 22.1 s, of
  which 2.2 s was silence.
- The per-sample stepping changed the DSP's timing slightly, so the 70 s
  intro's instruction count moved (1,161,691,899 → 1,159,290,469).
  Snapshots are taken at the first budget refill after each 600
  VBlanks, so some now fall on other VBlanks (601 instead of 607, 1,205
  instead of 1,201). Those taken at the same VBlank as before (1,801 and
  3,601) are byte-identical.

### R-040 2026-10-26 — Renderer speed, second round

Same scenario and check as R-037 (70 s of the intro; 14 snapshots,
byte-identical after each step, against a new baseline taken after
R-039's timing change):

| Change | Wall time |
|---|---|
| baseline (R-039 build) | 70.6 s |
| texture sampling: a byte-to-float table, floor without the library call, power-of-two repeat as a mask | 70.2 s (noise) |
| the vertex attribute layout decoded once per draw, each vertex's data read as one slice | 67.2 s |
| one raster band per tile row (work stealing balances them) | 64.3 s |
| shading in parallel from 16 distinct vertices (was 64) | 62.7 s |
| shader: a generic per-component helper, a reused flow-control stack | no change |
| bilinear sampling wraps its two columns and two rows once (was once per texel, eight times), with inlined helpers | 62.1 s (bands 34.7 → 32.7 s) |

- New probe lines: GPU time by phase, and shader runs and instructions.
  The intro runs 2.0 billion shader instructions (16.9 million runs,
  118 each); 300 s to the bedroom runs 16.8 billion.
- Phases for 300 s to the bedroom (326.6 s wall, 0.92× real time; was
  0.79×):
  - bands 144.7 s;
  - vertices 71.1 s (of which shading 51.6 s);
  - textures 6.8 s;
  - buffer copies 5.5 s.
- With one worker thread (`RAYON_NUM_THREADS=1`) the intro takes 262 s:
  shading 20.7 s (about 10 ns per shader instruction), bands 218 s.
  The pool gives about 3× on shading and 6× on bands.
- Single-threaded, the first 25 s of the intro spend 25% of samples in
  texture sampling and 0.6% in the shader (the heavy shaders come
  later), so the sampler mattered more than the profile of the whole
  run suggested.
- What is left in `raster_band` (perf annotate): the combiner, inlined,
  with operands picked from stack arrays by run-time indices and
  register bits tested per fragment. Next candidates: a shader program
  decoded once per upload, and a combiner specialized per draw.
- `floor` in texture sampling now avoids the library call. It is tested
  bit-equal to `f32::floor`, including -0.0, NaN and values beyond 2^23.

- **Latency (2026-10-26, user report: "audio seems delayed").** The
  stream left the buffer size to the server, which picks about 2 s, and
  our queue held up to 0.25 s more. The stream now asks for a 60 ms
  target with `adjust_latency` (sink latency included), and the queue
  keeps at most 80 ms. A 12 s check played 12.1 s, 3.5 s of it silence:
  a smaller buffer underruns more while the game is behind.
- **Then it crackled (user report).** At 60 ms the queue kept running dry
  for an instant, because the game mixes in bursts at each budget
  refill. Each gap was filled with hard zeros, heard as clicks. Now:
  - a 100 ms target, and at most 200 ms queued;
  - a gap fades the last sample out over 64 samples instead of cutting
    to zero;
  - after running dry, playback waits until 40 ms is queued again, so
    there are fewer, longer gaps rather than a click at every burst.

### R-041 2026-10-26 — Combiner arguments, decoded operands

Still byte-identical on all 14 snapshots of the 70 s intro.

- **Combiner (exact):**
  - Each stage's registers are decoded once per draw (`Stage`: sources,
    operands, modes, scale factors, the constant color).
  - A stage computes only the arguments its modes read: Replace one;
    Modulate, Add, AddSigned, Subtract, Dot3 two; Interpolate,
    MultiplyAdd, AddMultiply three; alpha none under Dot3_RGBA.
  - A texture is sampled only if a read argument names it; it used to
    be sampled when any argument did.
  Bands, intro: 32.7 → 28-29 s.
- **Shader operand descriptors** are kept decoded in the unit, updated on
  every descriptor write: swizzle indices, and negation as a flip of the
  sign bit, exactly unary minus. Tested bit for bit against the old
  per-component decode for all 256 selectors, both signs, -0.0, NaN and
  infinity. Shading did not get faster: 7.2-7.8 s against 6.8 s, inside
  this machine's run-to-run spread of about ±4 s on the whole run. The
  branches it removed were evidently well predicted. Kept because it is
  exact and simpler to read.
- **Measuring:** a `play` session running alongside made one
  measurement read 92 s. Timings are only compared with nothing else
  running. Correctness checks (frames) do not depend on load.
- **End to end, 300 s to the bedroom: 326.6 s → 304.5 s, 0.99× real
  time.** Bands 144.7 → 119.7 s; shading unchanged at 53.6 s, now the
  largest single item after the bands.

### R-042 2026-10-26 — Even sound in play; a rejected raster change

- **Why the sound came in bursts:** with the virtual clock, an all-idle
  stretch moved the clock straight to the next event without waiting
  for the wall clock. `play`'s pacing ran only at budget refills, at
  most 50 ms at a time. The game ran ahead, and the audio frames mixed
  meanwhile reached the sound queue in one burst at the next refill.
- **Fix:**
  - The kernel's `pace` (set by `play` only) makes idle jumps of the
    virtual clock wait for the wall clock.
  - The DSP pushes each 4.9 ms frame straight into the sound queue as it
    is mixed (`Dsp::sink`).
  - The probe keeps its unpaced, deterministic clock.
  30 s of `play`: 0.2 s of silence (start-up), against 3.5 s in 12 s
  before. That let the buffer shrink: a 50 ms server target, at most
  120 ms queued, playback resuming after a gap at 30 ms. Still 0.2 s
  of silence in 30 s.
- **Rejected:** a reciprocal of the area instead of three divisions per
  pixel, and one reciprocal for the perspective weights. No speedup
  (60.8 s against 58-62 s), and frames changed: at most 3 levels on 24
  pixels of one snapshot, over the bar set for such changes (at most 2
  levels, on at most 0.5% of pixels). Reverted; the frames are
  byte-identical to before.

### R-043 2026-10-26 — The Rockruff close-up; input recording

- **User report:** in `play` the sound stuttered once Rockruff came out,
  in Professor Kukui's video call (inside the intro script). The probe's
  new `USUM_SPEED=n` reports each n VBlanks' speed against real time
  and, per window, where the wall time went: outside command lists (the
  game's own code mostly), between draws, inside draws by phase. Every
  window of the intro ran at 1.2-2.0× except VBlanks 3,023-3,927, the
  Rockruff close-up: 0.89×, 0.67×, 0.95×.
- **What that window spent (7.2 s of wall time for 5 s of game time):**
  - triangle setup outside the measured phases: 1.54 s (0.36 s in a
    normal window);
  - shading: 1.37 s;
  - other vertex work: 0.58 s;
  - bands: 2.1 s (about as elsewhere).
- **Fixes, all byte-identical on the 14 snapshots:**
  - Clipping is skipped for a triangle whose corners are all inside
    every plane (it would return the triangle unchanged) or all outside
    one plane (it would return nothing). The clipper allocated a vector
    per plane per triangle.
  - Distinct-index slots come from an array over the indices' range
    instead of a SipHash `HashMap`.
  - The index buffer is read as one slice, and vertices are loaded in
    parallel from 64 distinct ones.
  The Rockruff window goes from 0.65× to 0.92× real time (5.46 s for
  5 s); the intro takes 50.2 s (56.1 before these). The rest is spread
  over shading (1.1 s), bands (2.1 s) and the game's own code (1.1 s).
- **Input recording (`play`):** the kernel records the live input as HID
  read it, one step per change. At exit `play` writes it as an input
  script to `~/.cache/usum-recomp/recordings/play-<time>.txt`, beside a
  copy of the saves as the session started (`.save/`). The probe
  replays a session exactly with `USUM_SAVE_FROM=<that>.save
  USUM_INPUT="$(cat <that>.txt)"`: the clock is virtual, so the same
  input at the same VBlanks gives the same run. Not yet checked
  against a real session.
- **Replaying a real session.** The user's session (13:08, about 2,700
  VBlanks, starting from their saves) replays in the probe with
  `USUM_SAVE_FROM` + `USUM_INPUT`. Snapshots show the same story
  moment they reported: Cutiefly flies in while Rockruff fills the
  foreground. The recording was missing at first: `play` exited before
  its game thread wrote it. `play` now joins that thread.
  - The replay runs at 1.35-2.1× real time except at the end: VBlanks
    2,430-2,583 at 1.13×, 2,583-2,733 at 0.96×. Per 2.6 s of wall time:
    bands 1.0 s, shading 0.5 s, the game's own code 0.5 s.
- **`play` keeps its game thread lighter.** It copies the raw
  framebuffers (`probe::framebuffer_raw`), and the window thread
  converts them after releasing the shared lock (`raw_to_rgb`); the
  conversion used to run per VBlank on the game thread. The window's
  pixels are checked unchanged (snapshot).
- **Dynamic rate control.** Where the game runs a few percent behind
  real time, the sound has to give way. It now plays slightly slower
  instead of in pieces:
  - the rate follows the queue's smoothed fill: 1.0 at a 30 ms
    setpoint, down to 0.9 when empty, up to 1.02 when over-full;
  - samples are linearly interpolated, and pass through unchanged at
    rate 1.0;
  - fade-and-refill stays as the last resort.
  20 s of `play`: 0.5 s of silence (start-up).

### R-044 2026-10-26 — The sustained slow stretch: per-fragment cost

- **The user's session with the new sound** (a private recording, 4,659
  VBlanks) replayed: after VBlank 3,181 the game holds 0.87-0.88× real
  time for about 25 s. That is the rest of Kukui's call, with Rockruff
  and Cutiefly on screen. It sits just below dynamic rate control's 0.9
  floor, so the sound still ran dry (5.1 s of 81.6 s).
- **Measured in one 5 s window there** (`USUM_SPEED`, extended with band
  CPU time and fragment counts):
  - 98 million fragments shaded against 85 million in an ordinary
    window, about 20 million a second (fur drawn as layers);
  - the alpha test rejects almost none (50,000);
  - bands 2.1 s of wall time but 19-20 s of CPU, so 8.8 of 16 threads
    busy: about 200 ns of CPU per fragment;
  - shading 1.3 s; the game's own code 1.0 s.
- **Tried, all byte-identical on the 14 snapshots:**
  - Splitting a busy tile row into runs of 8 tile columns (contiguous in
    memory): threads busy 8.8 → 9.7, the stretch 0.88 → 0.90×.
  - `std::array::from_fn` and `.map` replaced on hot paths by inlined
    literals (`arr3`, `arr4`). The profile had shown the standard
    helpers out of line (`try_from_fn_erased`, about 12% of samples).
    Total time did not move: the cost was the work inside them, now
    attributed to the callers.
  - The blend destination converted through the byte table, and the
    pixel codecs inlined: no change.
- **Where the time is now:** per fragment, the work itself is spread
  over interpolation, two or three bilinear samples, up to six combiner
  stages, and blending. Only a structural change is left: process each
  row span stage by stage (or 4 to 8 pixels at a time), so the
  per-stage setup is paid once per span instead of per pixel. That is a
  rewrite of the hottest code, with the snapshots as its check.

### R-045 2026-10-26 — The combiner over spans

- **The rewrite.** For each triangle's run of a row, the rasterizer works
  in passes instead of finishing one pixel before starting the next:
  1. coverage and the depth test, collecting the surviving pixels with
     their depth and perspective weights;
  2. primary color, and each texture the combiners read, for all of
     them;
  3. the combiner, stage by stage over the span (`combine_span`): each
     stage chooses its sources, operands and modes once, then runs a
     tight loop with them fixed;
  4. alpha test, blending and writes, per pixel.
  Pixels of a span are distinct, so the passes read the same stored
  depth and color a pixel-at-a-time loop would.
- **Exactness:** per fragment the arithmetic is the old combiner's, the
  same expressions in the same order (shared, always-inlined
  `rgb_operand`, `alpha_operand`, `apply`, `dot3`). A unit test runs 400
  random configurations against the old per-fragment `combine` (kept
  for tests only): every stage mix, Dot3, pass-through stages, buffer
  updates, all operands and modes. Every fragment is bit-identical. The
  14 snapshots are byte-identical.
- **The shader interpreter's closures** (`float`, `source`, `write`) are
  now always-inlined functions; one had been compiled out of line,
  6.6% of samples.
- **Measured:**
  - The intro: 48.4 → 46.4 s, bands 25.1 → 23.2 s.
  - The user's Kukui-call session: slowest windows 0.86-0.88× → 0.90-0.93×;
    shading there 1.35 → 1.12 s per 5 s window. Band CPU there barely
    moved (about 18 s per window): in that scene the per-fragment cost
    is spread over interpolation and texture sampling, with no line
    above about 2%.
- **Sound:** dynamic rate control's floor goes from 0.9 to 0.85, so the
  slowest stretch (0.90×, plus `play`'s own overhead) plays about 8-10%
  slow instead of running dry.
- **Left for later:** processing 4 to 8 pixels at a time (SIMD-style)
  through interpolation and sampling. Overlapping the GPU with the CPU.

### R-046 2026-10-26 — Source filters; walking the bedroom

- **The game's voices only use intermediate mix 0.** Gains of all 382
  buffers started in the intro (`USUM_DSP_TRACE`): mixes 1 and 2 are
  zero everywhere, so leaving them out (R-039) drops no sound. Streams
  are mono sources panned hard left and right (0.441 each).
- **Filters implemented.** Fields (3dbrew):
  - flags u16 at +58: bit 0 simple, bit 1 biquad;
  - simple filter at +60: b0, negated a1 (s1.15);
  - biquad at +64: negated a2, negated a1, b2, b1, b0 (s2.14).
  Dirty bits 22-24 (filters enabled, simple, biquad), from Citra's HLE as
  documentation; they fit the flag sets seen (0x5E670014 sets 22). The
  filters run per output sample after interpolation, simple first (also
  Citra's order, not on 3dbrew).
  The coefficients Ultra Sun sends make sense under that reading, which
  supports the layout:
  - a biquad high-pass (b0 = b2 = 0.915, b1 = -1.831; a1 = -1.938, a2
    = 0.941: stable, and the b coefficients sum to 0, rejecting DC);
  - a one-pole low-pass (b0 = 0.321, feedback 0.679: DC gain 1.000).
  16 of 382 buffers use both. The intro's WAV after: no clipping, RMS
  1,800-4,200, 1,500-3,200 zero crossings a second.
- **Not done:** the master volume and the effect returns. 3dbrew does not
  describe the DSP configuration structure that holds them.
- **Scripted walking in the bedroom works.** With the stick, the player
  walks and the camera follows. Pressing A at the stand by the bed opens
  "It's the new coatrack your mom bought for you!". The exit was not
  found blind (down: the room's front edge; left: a wall with a white
  panel). That is better done by the user playing, with the session
  recorded for replay. 300 s of game time to the bedroom now take about
  250 s: 1.2× real time.

### R-047 2026-10-26 — Saving; the window showed under 10 frames a second

- **Saving (user report: the game froze when saving).** The user's
  recording replayed to the stop: `ssl:C` 0x00010002 (SSLC:Initialize,
  process ID only), then 0x00110042 (GenerateRandomData: 8 bytes into a
  mapped buffer), then `fs:USER` 0x086E00C0, all right at the save.
  - SSLC:Initialize: result-only (no connection is ever made).
  - GenerateRandomData: bytes from a fixed-seed xorshift64*. Runs and
    replays then repeat exactly; a real console's bytes differ every
    time (a choice).
  - SetThisSaveDataSecureValue (slot, u64): kept in the save directory
    as `secure-value-<slot>`, beside the save it guards (3dbrew: the
    value lets a game notice a save rolled back). Ultra Sun uses slot
    0x1000. Get… will read it back when the game first asks.
  With these the save writes 435,466 bytes, commits, and the game
  carries on in the new house in Alola.
- **The window showed fewer than 10 frames a second** (user report: "the
  title screen seems laggy even though it looks like its running full
  speed"). `play` published frames from its budget-refill hook. With
  idle time paced to the wall clock (R-042) the game spends most of a
  frame asleep inside the kernel, where the hook does not run, so most
  VBlanks were never shown: 191 redraws in 20 s.
  - The kernel now copies each screen's newest framebuffer at every
    VBlank into `FrameOut` and wakes the window, as the LCDs scan out:
    1,202 redraws in 20 s, 60 a second.
  - The window also scales each source row once and copies it to the
    rows it covers: 1.2 ms a redraw at 2×.
- **Where the sound still gives way:** the replay of that session runs
  below the 0.85 floor in two big cutscenes, Lillie with Nebby (0.78×)
  and the arrival at the house in Alola (0.85×). The title screen runs
  at 1.5-1.8×, and `play` there loses only start-up (0.5 s in 20 s).

### R-048 2026-10-26 — Pitch kept in tune; compiling for the user's CPU

- **User report: the music seemed out of tune.** That was the dynamic
  rate control of R-044/R-045: playback could slow to 0.85 (about 2.8
  semitones), varying with how far behind the game was. Now ±0.5%,
  which is inaudible and absorbs drift between game and sound device, as
  emulators usually do. A scene slower than that has short gaps again,
  until rendering is faster. 20 s of `play`: 0.2 s of silence, 60
  redraws a second.

### R-049 2026-10-26 — Watertight coverage (user report: a diagonal line)

- **The line:** a quad is two triangles sharing a diagonal. Coverage was
  tested in f32 per triangle, so the two triangles evaluated the shared
  edge with different rounding. Pixel centres on or near it were drawn
  by neither or by both, a visible line from corner to corner.
- **Fix:** corners snapped to 1/16 pixel, the edge functions evaluated in
  i64 at pixel centres, and a pixel exactly on an edge owned by one
  triangle only. The top-left rule: E = A·px + B·py + C with the inside
  positive owns its zeros when A > 0, or A = 0 and B > 0. A shared edge
  has opposite (A, B) in its two triangles. The weights come from the
  same integers (E / twice the area).
- **Test:** a square split along its diagonal, both windings, at 0, 0.25
  and 0.5 pixel offsets (centres exactly on the diagonal): every pixel
  inside covered exactly once, none twice.
- **Effect on frames:** the snapping moves interpolation weights by tiny
  amounts. The intro snapshots differ in up to 7% of pixels, almost all
  by a few levels in textured areas; larger differences only at a few
  edge pixels. They look the same. New baseline.

### R-050 2026-10-26 — Loading a save: secure value, Y2R, hand-written jump tables

- **Loading:** the game reads the secure value back with
  `GetThisSaveDataSecureValue` (0x086F0040: result, u8 exists, u8 is a
  game card, u64 value), from the file `SetThis…` wrote. It is a
  cartridge dump, so "game card" is 1.
- **`play` seemed to hang** after a stop: by design it keeps the window
  open to show why the game stopped. That was also the session with
  12.7 s of 12.8 s silent.
- **y2r:u (YUV-to-RGB, video), all 45 commands of 3dbrew's table**, as
  the module `services/y2r.rs`:
  - settings stored and returned;
  - sending and receiving buffers with their transfer units and gaps;
  - the transfer-end event;
  - StartConversion done at once: YUV 4:2:2/4:2:0 planar (8- or 16-bit
    samples) or packed, to RGBA8/RGB8/RGB5A1/RGB565, linear or 8x8
    blocks in Z order, then the event fires.
  3dbrew documents only the table. The arithmetic and the four standard
  coefficient sets follow Citra (its authors call it bit-exact as far
  as they tested): fixed-point coefficients, >> 3, + offset + 0x18,
  >> 5, clamped. SetPackageParameter's layout is also Citra's.
- **Code discovery: two hand-written patterns** in the video decoder
  (0x00133600-0x00136000):
  1. Tables of offsets relative to their own address, reached by `add
     pc`:
     ```
     sub rB, pc, #n
     ldr rX, [rB, rI, lsl #2]
     add pc, rX, rB
     ```
     Entries are taken while they land on decodable, word-aligned code
     (at most 256). Some tables have negative entries and sit right
     after their targets. The base can be set 8 instructions before the
     jump.
  2. Jump targets computed from the PC by immediate adds:
     ```
     add r4, pc, #0x2400
     add r4, r4, #0x248
     movle pc, r4
     ```
     Evaluated by looking back at most 8 instructions; any other write
     to the register stops it.
  Translation: 36,255 → 36,441 functions, 2.27M → 2.49M instructions.
  Tested with small hand-assembled images.

### R-051 2026-10-26 — Fragment lighting (user report: black grass)

- **Measured first:** in the user's 11-minute session 560,149 draws had
  fragment lighting on (GPUREG_LIGHTING_ENABLE0 bit 0), and 549,969
  read its results (combiner sources 1, 2): nearly every 3D draw.
  Source 1 had been treated as the vertex color and source 2 as black,
  so models looked plausible but unlit, and surfaces colored mostly by
  lighting (the grass) came out black. No draw reads texture 3, the
  procedural texture (source 6, which was treated as white).
- **Implemented (`pica/lighting.rs`):**
  - Registers: 3dbrew GPUREG_LIGHT*_* and GPUREG_LIGHTING_* (colors,
    float1.5.10 positions, fixed1.1.11 spot directions, float1.7.12
    attenuation, configuration, the disable bits per table and per
    light, the abs/select/scale fields, the light permutation, 24
    tables of 256 entries uploaded through LUT_INDEX/LUT_DATA).
  - Vertices carry the quaternion (output map semantics 0x04-0x07) and
    the view vector (0x12-0x14), interpolated perspective-correct like
    the other attributes.
  - Per fragment: the normal is (0, 0, 1) rotated by the normalized
    quaternion. For each light:
    - the light vector: position + view, or the direction for a
      directional light; then the half vector;
    - N·L, absolute for two-sided lights, otherwise clamped at 0;
    - distance attenuation (DA table), spotlight (SP table);
    - specular: D0 × specular0 + D1 × reflection(RR, RG, RB) ×
      specular1, with geometric factors and clamped highlights;
    - Fresnel sets the alphas.
    Then the global ambient is added and both colors clamped.
  - Source of the equations, table indexing (abs: v × 256; signed:
    floor(v × 128) as two's complement; value/4095 + difference/2047 ×
    the fraction) and which tables each light environment
    configuration allows: Citra's software renderer, read as
    documentation; 3dbrew does not give them.
  - Not emulated: shadows, bump and tangent maps, the CP input (0).
  - Sources 1 and 2 are zero when lighting is off, as in Citra.
  - Tests: number formats, a directional light on a surface facing it
    and away from it, table interpolation. The combiner equivalence
    test now includes random lighting inputs.

### R-052 2026-10-27 — The grass replay passes; Vulkan backend bring-up

- **The remaining video-decoder stop was a chained return address.** At
  0x00133928 the hand-written decoder computes LR with `add lr, pc,
  #0x80; add lr, lr, #0x38`, then reaches 0x001339ec on return. R-050's
  discovery rule only recognized the first PC-relative instruction.
  Return discovery now reuses the bounded PC-constant recovery for
  immediate add/sub chains. Its regression uses the two-instruction
  form. All 123 existing workspace tests pass.
- **Saved video check:** 17,179,766,148 instructions, 3,827 VBlanks and
  7,632 presented framebuffers ran without an untranslated-code fault;
  the opening video rendered. The run ended only at the probe's ordinary
  instruction budget.
- **Full recorded grass/lighting replay:** 650 s of game time completed
  at its requested limit in 758.559 s wall time: 38,889 VBlanks,
  1,537,499 draws, 561,217,399 triangles and 16,214,650,791 fragments.
  560,149 draws enabled fragment lighting and 549,969 read its results.
  Captures at VBlanks 27,001 and 30,600 show the lit grass, foliage,
  characters and shadows instead of black grass. Saves made three clean
  commits. Rendering occupied 572.034 s (338.011 s rasterizing and
  211.284 s in the vertex phase), making the GPU backend the next large
  speed step.
- **`usum-render` started.** The new isolated crate uses wgpu 30 with
  only its native Vulkan/WGSL features. Its first check creates an
  off-screen RGBA target, compiles WGSL vertex/fragment stages, builds a
  graphics pipeline and submits a triangle. On the host this selected
  `AMD Radeon RX 9070 XT (RADV GFX1201)`, reported `DiscreteGpu`, and
  completed successfully. This is a real Vulkan draw, not a software
  framebuffer upload; PICA state and buffers are not connected yet.

### R-053 2026-10-27 — First PICA-facing Vulkan raster path

- `usum-render::RasterVertex` is the first boundary from the existing
  emulation into the host GPU: a PICA-style clip-space position and the
  vertex color after the emulated vertex/geometry stages. The Vulkan
  vertex stage passes those through and the hardware rasterizer performs
  coverage and perspective color interpolation into RGBA8.
- The target is copied back through a MAP_READ buffer. Rows honor
  Vulkan/WebGPU's 256-byte copy alignment and are unpacked to ordinary
  tightly packed top-to-bottom RGBA8, ready for draw-by-draw comparison
  with the software reference. Vertex uploads use mapped safe-Rust
  buffers; no pointer casts or new unsafe code were introduced.
- The smoke check now uses red, green and blue corners. At the target
  centre it requires roughly 1/4 red, 1/4 green and 1/2 blue, so it
  checks raster coverage, interpolation, command completion and readback
  rather than submission alone. It passes both llvmpipe and the host's
  RX 9070 XT/RADV Vulkan driver. A second, separate white-triangle
  submission must appear without erasing the first: color targets persist
  across draws, as the game's framebuffers require. The shader and
  graphics pipeline are created once with the renderer, not once per draw.
- Pure tests cover the 32-byte two-vec4 vertex layout and reject empty or
  malformed triangle lists and zero-sized targets before touching a GPU.
  R-054 continues this path with depth targets, viewport translation and
  culling. TEV/textures, lighting, blending and command-list integration
  remain before game use.

### R-054 2026-10-27 — Persistent depth, PICA viewport and culling on Vulkan

- Each persistent color target now owns a matching `Depth32Float` image.
  Draws load both attachments instead of clearing them, so depth ordering is
  preserved across separate submissions just as color was in R-053. The
  initial path writes depth and uses `LessEqual`.
- PICA clip depth is `-w..0`, while WebGPU/Vulkan expects `0..w`; the vertex
  shader negates clip-space z without changing x, y or w. A hardware check
  submits far, near, then farther geometry and verifies that the final draw
  cannot overwrite the nearer triangle.
- `Viewport::from_pica` converts the PICA bottom-origin viewport rectangle to
  wgpu's top-origin coordinates. Pure tests cover the conversion and the GPU
  check verifies that a partial viewport changes its centre while leaving a
  target corner untouched.
- The PICA cull register maps 0 to none, 1 to front, 2 to back and the reserved
  value 3 conservatively to none. Three pipelines are cached at renderer
  creation, so changing cull mode does not build a pipeline during a draw.
  The GPU check confirms a counter-clockwise triangle survives back culling
  and disappears under front culling.
- The complete smoke check passes on the host's AMD Radeon RX 9070 XT with
  RADV/Mesa 25.2.8. It now validates draw persistence, viewport, culling,
  rasterization, interpolation, depth and readback through the real Vulkan
  backend. Game command lists are still not connected.

### R-055 2026-10-27 — Register-driven fragment state, textures, TEV and batching

- The Vulkan fixed-function pipeline now decodes all eight PICA depth
  comparisons, independent depth writes, the four color-channel masks, alpha
  testing, all blend equations and blend factors, and the dynamic blend
  constant. `PicaRasterRegisters` converts the relevant raw register bundle
  into one `RasterState`. In particular, channel enables correctly come from
  GPUREG_DEPTH_COLOR_MASK bits 8-11 while register 0x113 is the framebuffer-
  wide write permission. The one fixed-function case Vulkan cannot represent
  (simultaneous RGB constant-color and constant-alpha factors) is rejected
  explicitly rather than approximated.
- Pipelines are cached lazily by culling, depth, color-mask and blend state.
  This avoids constructing the 768 possible cull/depth/mask combinations at
  startup. Per-draw alpha and TEV data use a read-only storage binding.
- `RasterVertex` now carries all three perspective-interpolated texture
  coordinate pairs. Three independently sampled decoded RGBA8 textures feed
  TEV sources 3-5, with nearest/linear filtering and clamp-to-edge, repeat or
  mirrored-repeat addressing. Arbitrary PICA clamp-to-border colors are
  rejected explicitly until manual shader sampling implements them.
- The fragment shader executes all six raw TEV stages: source and operand
  selection, Replace/Modulate/Add/AddSigned/Interpolate/Subtract/Dot3/
  MultiplyAdd/AddMultiply, 1x/2x/4x scale, constants, previous results and the
  shared combiner buffer. Lighting sources still resolve to the unlit zero
  values; fragment lighting is the next major shader input.
- `RasterBatch` records many PICA draws in one Vulkan command buffer and makes
  one submission/wait. The convenience one-draw API remains for diagnostics.
  This removes the per-draw submission design that would have been unusable
  for the 1.5 million-draw grass replay.
- The RX 9070 XT/RADV hardware smoke now validates batching, persistent
  color/depth, viewport, culling, interpolation, every fixed state above,
  nearest/repeated and linear texture sampling, all three texture bindings,
  chained TEV constant/previous/scale behavior and readback. The backend is
  still isolated: it has not replaced or modified the software renderer.

### R-056 2026-10-27 — Fragment-lighting data reaches Vulkan and TEV

- `RasterVertex` now includes the PICA quaternion and view vector alongside
  its clip position, primary color and three texture coordinates. The host GPU
  perspective-interpolates them for each fragment; the vertex upload remains
  explicit safe Rust and is now 84 bytes per vertex.
- `LightingState` and `LightingLight` form a decoded boundary which retains all
  eight possible lights' diffuse, ambient and two specular colors, position,
  spot direction, directional/two-sided/geometric flags, attenuation values,
  global ambient and lighting configuration words. `GpuLighting` uploads that
  state independently of the per-draw TEV buffer, leaving room for separately
  cached LUT data.
- The first lighting shader increment normalizes the interpolated quaternion,
  rotates the +Z surface normal, evaluates directional or positional light
  vectors, two-sided diffuse, ambient, the two base specular terms, geometric
  factors and the CONFIG0 back-face specular clamp. Its primary and secondary
  results feed TEV sources 1 and 2.
- Hardware validation uses one white directional light. Identity quaternion
  produces white through TEV source 1; a 180-degree X rotation produces black.
  Both pass on the RX 9070 XT/RADV path without changing the software renderer.
- This is deliberately not called complete PICA lighting yet. Distance and
  spotlight attenuation, D0/D1 distribution, reflection, Fresnel and LUT
  input selection/scaling still need the 24×256 lookup-table binding and
  shader implementation before grass-scene comparison is meaningful.

### R-057 2026-10-27 — Complete lighting LUTs and a raw-register boundary

- The lighting storage buffer now has a fixed layout for the global header,
  all eight light records and all 24 256-entry PICA lighting LUTs. LUT entries
  preserve the packed 12-bit value and signed 12-bit delta so the shader can
  interpolate between entries rather than reducing the table to scalar values.
- WGSL lighting now evaluates the configured input selector and signed or
  absolute index mode, applies LUT scaling, and implements D0/D1 distribution,
  red/green/blue reflection, per-light distance attenuation, spotlight
  attenuation and Fresnel alpha selection. The base ambient, diffuse,
  specular, geometric and two-sided terms from R-056 remain in the same path.
- The RX 9070 XT/RADV smoke check exercises a nontrivial D0 entry with a signed
  delta and verifies the resulting half-intensity secondary lighting value.
  This checks the packed-table interpolation through an actual Vulkan draw,
  not only the CPU upload layout.
- `LightingState`, `TevState` and `PicaRasterRegisters` can now be constructed
  from immutable raw PICA register snapshots at the renderer boundary. Pure
  tests cover representative raster, TEV, light color, position and flag
  decoding. The software renderer and runtime remain untouched; the next seam
  is an explicitly enabled capture/mirror path that keeps software output
  authoritative while Vulkan results are compared.

### R-058 2026-10-27 — Guest framebuffer snapshots can seed Vulkan safely

- A persistent Vulkan target can now be initialized from tightly packed RGBA8
  color and normalized depth snapshots. This closes a correctness hole in a
  future draw mirror: beginning from cleared attachments would give misleading
  comparisons whenever the guest framebuffer already contains earlier draws.
- WebGPU forbids direct queue copies into `Depth32Float`. Depth snapshots are
  therefore uploaded to `R32Float` and transferred by a depth-only full-screen
  render pass using an always-write depth pipeline. Values and buffer lengths
  are checked before submission.
- The RX 9070 XT/RADV smoke test seeds magenta color at depth 0.25, verifies a
  farther triangle cannot change it, then verifies a nearer green triangle can.
  Color readback confirms both the uploaded color and depth ordering.
- `PicaDrawState` now decodes one immutable raw register/LUT snapshot into the
  checked viewport, fixed raster state, all six TEV stages and optional complete
  lighting state. This is the next runtime handoff unit; it does not hold guest
  memory or mutate the software renderer.

### R-059 2026-10-27 — Feature-gated runtime-to-Vulkan draw handoff

- `usum-runtime` has an off-by-default `vulkan-compare` feature. With it built,
  callers explicitly budget the number of subsequent draws to capture. The
  ordinary default build has no renderer dependency or changed behavior.
- Capture runs after vertex and optional geometry shading and primitive
  assembly, while the original software draw continues normally. Each owned
  snapshot contains the triangle-list vertices, complete decoded draw state,
  pre-draw tiled color/depth converted to host rows, all enabled decoded
  textures and sampler state. Register writes and guest-memory changes after
  capture cannot alter it.
- A snapshot can execute independently through `usum-render`: it seeds the
  guest color/depth, creates its textures and lighting buffer, submits the draw
  and returns RGBA8 without accessing or modifying guest memory. This is a
  comparison path only; it cannot replace the software framebuffer.
- The cross-crate smoke rendered such a snapshot on the RX 9070 XT/RADV and
  returned centre pixel `[61, 72, 122, 255]`. The feature-enabled runtime has
  49 passing tests and passes strict Clippy. Real game draw comparison and
  unsupported clamp-to-border/target formats remain before replacement is safe.

### R-060 2026-10-27 — Real-game Vulkan comparison, including lit grass

- Captures now retain the immediate post-draw software framebuffer and report
  exact per-pixel differences against an isolated Vulkan replay. Standard PICA
  transparent-black, opaque-black and opaque-white clamp-to-border modes map to
  native Vulkan sampler border colors; unsupported arbitrary colors still fail
  safely rather than silently sampling incorrectly.
- The first eight startup draws compare exactly: every one has 0 differing
  pixels out of 131,072. Draws around 100 differ only by one channel value at
  a small number of coverage pixels. A later two-triangle quad exposed a
  localized coverage-edge disagreement rather than a TEV-wide color error.
- A deterministic 470-second replay captured eight real fragment-lit draws
  around guest draw 1,000,001, including 1,154- and 748-triangle draws in the
  outdoor grass region. One draw was exact; the others had only 3 to 377
  differing pixels out of 131,072, with only 1 to 22 pixels over a one-channel
  delta. The full run completed 1,007,890 draws without a renderer error.

### R-061 2026-10-27 — Experimental live Vulkan color path

- `USUM_VULKAN_LIVE=1` opts the generated player into command-list replay.
  The software renderer and its depth buffer remain active as a correctness
  reference and automatic fallback; Vulkan batches the captured draws by
  target, reads the resulting color image back, and writes it into the guest's
  tiled framebuffer before presentation.
- A guarded two-second launch and a measured ten-second real-game launch both
  completed on the RX 9070 XT/RADV without fallback or a renderer error. The
  latter replayed 366 draws across 1,111 command lists and spent 0.654 seconds
  in Vulkan replay. It produced 197 VBlanks and 199 window redraws in 10.1
  seconds; the live path is functional, but it is not yet an acceleration
  because software rasterization and GPU readback are deliberately retained.
- Live capture now shares one immutable pre-list color/depth seed per target
  and omits per-draw post-software comparison images. This removes two full
  framebuffer decodes/copies from later draws without weakening the normal
  comparison mode. The same ten-second startup benchmark improved from 197 to
  210 VBlanks (6.6%) after this change.

### R-062 2026-10-27 — Persistent live resources and depth validation

- Decoded software textures now use shared immutable RGBA storage. Vulkan
  snapshots reuse the software cache's byte-validated allocation, and live
  replay retains a bounded cache of the corresponding Vulkan images across
  command lists. Color/depth target images also persist across command lists;
  the authoritative software snapshot is still uploaded before each replay,
  preserving fallback behavior while removing repeated texture/target
  allocation and clear submissions. The deterministic startup used 4,015
  texture bindings but required only 679 GPU texture uploads (83% reuse).
- Fragment-lighting LUTs are also copy-on-write shared between PICA state,
  software draw state and Vulkan snapshots. A LUT register write creates a new
  immutable version only while an older draw still owns the prior version.
  Live replay caches the corresponding GPU lighting buffer by the small light
  configuration plus LUT identity, avoiding a 24×256-word clone and upload on
  each unchanged lit draw.
- Vulkan depth targets can now be read back as normalized `f32`. The hardware
  smoke verifies a drawn depth value, and real comparison quantizes it into the
  guest's 16- or 24-bit units. The first eight real startup draws are exact in
  both color and depth: 0 of 131,072 values differ for every draw.
- `USUM_VULKAN_LIVE_DEPTH=1` separately opts into tiled guest-depth writeback;
  D24S8 writeback preserves the stencil byte. A deterministic headless run
  replayed all 3,695 draws across 357 command lists with color and depth enabled
  and no fallback or fault. The ordinary `USUM_VULKAN_LIVE=1` remains
  color-only while depth evidence is expanded into later scenes.
- For the identical 51,085,751-instruction startup, software alone took 1.470
  seconds, the color mirror took 2.136 seconds (0.412 in replay), and the
  initial color+depth mirror took 2.880 seconds (0.713 in replay). Combining
  color/depth readback, merging snapshot upload with the draw submission, and
  retaining Vulkan textures across lists ultimately reduced color replay to
  0.390 seconds and color+depth replay to 0.434 seconds. The complete
  color+depth run fell to 2.164 seconds. The next speed step is therefore to
  avoid the remaining synchronous per-list upload/readback and then remove
  redundant software rasterization only for validated draw classes.

### R-063 2026-10-27 — Guarded Vulkan raster acceleration

- `USUM_VULKAN_ACCELERATE=1` is the first mode that replaces software
  rasterization rather than mirroring it. It implies color+depth writeback,
  checkpoints PICA command-list state, and detects a later texture read from
  an earlier target in the same list. A dependency reruns that list in
  software; a Vulkan failure disables acceleration and also reruns it. Vulkan
  results are committed to guest memory only after every target in the list
  renders and reads back successfully, so partial GPU failure cannot leave a
  half-updated guest framebuffer.
- Skipping software initially exposed a useful cache coupling: decoded texture
  identity had been populated by software rasterization. Capture now populates
  the same byte-validated cache directly. In the two-second startup this
  reduced Vulkan texture uploads from all 4,015 uses to 63 uploads, while all
  357 command lists accelerated with zero fallbacks and zero software
  fragments.
- A ten-game-second deterministic run completed 126,973,963 instructions,
  25,307 draws and 174,912 triangles. All 1,797 command lists accelerated,
  with zero feedback dependencies and zero software fallbacks; only 65 of
  27,307 texture uses required upload. It took 5.382 seconds versus 8.054
  seconds for the matching software run, a 33% end-to-end speedup despite
  retaining snapshot upload and synchronous color/depth readback.
- The dependency guard now compares full guest-memory ranges rather than only
  exact base addresses. A texture that partially aliases an earlier color or
  depth target therefore sends the entire command list through the software
  fallback as well. Focused overlap tests cover both sides of a target and
  the adjacent non-overlapping boundaries.
- Accelerated color/depth readback is now encoded into the draw command buffer
  and uses the draw submission's device wait, instead of submitting and
  waiting once for drawing and again for readback. The RX 9070 XT hardware
  smoke exercises this combined path and validates its color and depth output.
- Each persistent GPU target now also owns reusable color and depth staging
  buffers. Synchronous readback remaps them after each use instead of allocating
  two new buffers per target replay. The renderer smoke validates repeated
  remapping, and an end-to-end two-second game probe completed 3,695 draws and
  357 accelerated lists with zero fallbacks after the change.

### R-064 2026-10-27 — Broader accelerated replay and lighting reuse

- A deterministic 60-game-second replay of a recorded interactive session
  reached 1,047,731,985 translated instructions, 82,574 draws and 21,086,424
  triangles while the older interactive player remained open. The guard sent
  1,349 dependent command lists through software after detecting 8,094 texture
  reads overlapping earlier color or depth targets. The other 35,515 command
  lists and 78,527 draws completed through Vulkan without a renderer failure.
  This is the first broad evidence that render-to-texture dependencies are
  common after startup and that the transactional fallback is doing real work.
- That replay initially uploaded a new GPU lighting buffer for all 17,356 lit
  draws. The game was installing byte-identical LUT contents into distinct
  copy-on-write allocations, so pointer identity alone could not recognize
  reuse. The cache now first uses the cheap identity path and then accepts
  equal LUT contents when the smaller light configuration also matches. A
  focused test covers distinct-but-equal allocations.
- Repeating the identical replay reduced lighting uploads from 17,356 to 289
  while preserving every instruction, draw, triangle, dependency and fallback
  count. Under the same concurrent-player load, Vulkan replay time fell from
  20.653 to 19.446 seconds and total time from 71.823 to 66.077 seconds. These
  timings are intentionally provisional because the interactive player was
  consuming CPU, but the 98.3% upload reduction is deterministic.

### R-065 2026-10-27 — Exact depth-feedback barriers

- Full-range dependency classification on the same 60-second replay found
  that all 8,094 reads were exact depth-target aliases: no color or partial
  aliases. Every case sampled a D24S8 target through texture format RGBA8.
  This means normalized host depth is not an equivalent substitute: the game
  observes the preserved stencil byte followed by the three depth bytes in
  PICA's reversed RGBA component order.
- A specialized segmented replay now splits a command list before those
  dependencies, preserves prior GPU target state across segments, converts
  D24S8 results into the exact bottom-first RGBA8 texture representation, and
  defers every guest-memory commit until all segments succeed. A focused test
  verifies depth quantization, row orientation and stencil preservation.
- The exact path completed the deterministic replay with the same
  1,047,731,985 instructions, 82,574 draws, 21,086,424 triangles and 3,589
  VBlanks as the guarded run. All 36,864 command lists stayed on Vulkan, with
  zero software fallbacks and zero software fragments. This exercises the
  dependency model without proving pixel equivalence; the first CPU-mediated implementation took 269.034
  seconds: 10,576 target segments and 9,867 feedback texture uploads made it
  much slower than the 66-second guarded-fallback run.
- The CPU-mediated path is therefore diagnostic and explicitly opt-in through
  `USUM_VULKAN_CPU_DEPTH_FEEDBACK=1`. Ordinary acceleration keeps the faster
  transactional software fallback. Reconstructed texture allocations are now
  shared for all reads of one target version, while the production direction
  is GPU-side D24S8-to-RGBA8 conversion without intermediate readback.


### R-066 2026-10-27 — GPU depth-feedback conversion and resident segments

- Added a Vulkan D24S8-to-RGBA8 conversion pass. It samples rendered depth,
  combines the preserved guest stencil, and reverses rows into texture order.
  Runtime draws bind the converted GPU texture directly. An RX 9070 XT smoke
  test checks every texel of a patterned depth/stencil image against expected
  byte values, including zero and maximum depth and varied stencil values.
- Intermediate segments retain their GPU color/depth targets instead of
  reading back and re-uploading them. Final segments combine draw and readback
  in one submission; targets no longer used by the final segment are read back
  separately. All results are collected before committing guest buffers.
- Before residency was added, the 30-second recorded replay completed with
  465,800,055 instructions, 37,858 draws, 3,397,586 triangles and 1,794 VBlanks.
  All 14,398 command lists accelerated, including 2,706 depth feedback reads,
  with no software fallbacks. The initial resident version preserved those
  counts. Both runs took about 22 seconds under variable host load; these are
  execution checks, not full-game pixel equivalence or stable benchmarks.
- `USUM_VULKAN_DEPTH_FEEDBACK=1` opts into this experimental path; the old
  CPU_DEPTH_FEEDBACK variable is retained as an alias. Normal acceleration
  still falls back for feedback. CPU waits remain at submissions, and the
  guest stencil plane is still uploaded. Outdoor comparison remains needed.

- The final resident/fused-readback build also completed the 60-second replay:
  1,047,731,985 instructions, 82,574 draws, 21,086,424 triangles, 3,589 VBlanks,
  36,864 accelerated lists, 8,094 depth reads and zero fallbacks. Decoded texture
  uploads fell from 9,867 in R-065 to 1,426 (conversion passes are separate from
  this upload counter). Total elapsed time was 63.678 s, versus the historical
  269.034 s CPU-mediated run; host loads differed, so this is not a controlled
  speedup measurement. Vulkan replay accounted for 23.224 s.

- A frame captured at VBlank 1,793 of the 30-second replay was compared with
  software using identical input: the bottom screen was byte-identical;
  top-screen RGB mean absolute error was 0.1051/255. Of 96,000 pixels, 8,266
  differed, but only 144 had any channel differ by more than 8 (maximum 195).
  This is one intro frame, not outdoor validation. The matching software run
  took 21.079 s versus 22.237 s for Vulkan, so this test does not demonstrate
  an overall acceleration benefit despite removing software fallbacks.

- Validation: workspace tests with window/Vulkan features passed (149 passed,
  six ignored); strict renderer/runtime Clippy across all targets passed;
  formatting passed. Both generated release executables (`play` and `probe`)
  were rebuilt with the Vulkan feature. No interactive window was launched.


### R-067 2026-10-27 — Queue dependent work without intermediate CPU waits

- `RasterBatch::submit` queues intermediate segments and checks validation
  without waiting for GPU completion. Depth-feedback conversion similarly
  relies on ordering within the same queue. Final readback remains the
  completion boundary before guest-memory commit.
- The hardware smoke check now queues a patterned depth upload, conversion,
  and sampling without intermediate waits, then checks all texels after
  readback. It passed on the RX 9070 XT.
- The 30-second replay preserved all instruction/draw/triangle/VBlank counts
  and zero fallbacks. Both captured screens at VBlank 1,793 were byte-identical
  to the previous Vulkan build. GPU replay time was 7.808 s versus 8.178 s
  previously; total time was 21.556 s versus 22.237 s. These single runs under
  variable host load suggest only a modest benefit, not a controlled benchmark.

- Full outdoor validation exposed a previously untested live-rendering defect:
  terrain becomes black in the island establishing shot (VBlank 10,801) and
  under the grass in later outdoor scenes (23,401). The 10,801 frame has RGB
  MAE 9.0499/255 against software. Replaying with depth feedback disabled gives
  a byte-identical defective frame, ruling out feedback conversion for this
  mismatch. The opt-in path must not be described as visually validated yet.
  The long diagnostic was stopped after capture 25,201 once this failure was
  reproduced, to focus resources on the corrected replay; it did not complete
  the requested 650-second limit.

### R-068 2026-10-27 — Match the guest's integer depth-test domain

- The software rasterizer truncates depth to D16/D24 before comparison, while
  Vulkan previously compared unquantized floats against uploaded quantized
  depths. An equality pass at depth 0.25 against an uploaded guest buffer
  therefore failed. The shader now quantizes fragment depth according to
  GPUREG_DEPTHBUFFER_FORMAT before Vulkan's depth test and write.
- GPU reciprocal approximation also prevented exact D24 equality. For D24,
  normalizing a nonzero integer q by 2^24-1 is the next representable float
  above q/2^24. D16 uses a power-of-two expansion. An exhaustive host check
  compared every D16 and D24 code with CPU division with zero mismatches;
  hardware equality checks cover zero, maximum, near-zero and fractional
  depths in both formats. These tests pass; the outdoor replay is the separate
  test of whether this fixes the observed terrain defect.

- The 182-second depth-corrected replay completed 4,654,983,548 instructions,
  334,050 draws, 95,267,049 triangles and 10,888 VBlanks with 118,021 accelerated
  lists and no fallbacks. However, the 10,801 frame was byte-identical to the
  defective pre-fix output. Integer-depth correctness is a separately verified
  fix, not the explanation for the terrain defect.

### R-069 2026-10-27 — Preserve texture-unit 2 coordinate routing

- Static comparison with the software rasterizer found that TEXUNIT_CONFIG
  bit 13 redirects texture unit 2 to coordinate set 1. Vulkan snapshots had
  always bound coordinate set 2. The snapshot now resolves this routing from
  the draw's register state, preserving it across subsequent register changes.
- The regression uses different coordinates in sets 1 and 2 and checks both
  register settings, plus immutability of the previous snapshot. Targeted
  renderer/runtime tests and strict Clippy passed; the outdoor replay checks
  whether this missing route caused the terrain mismatch.

- The routing-corrected 182-second replay again completed with identical
  execution counts and no fallbacks. Its outdoor frame remained byte-identical
  to the defective output, so routing is another independently demonstrated
  omission but not the cause of this terrain mismatch. Individual-draw
  differential capture is the next diagnostic, rather than further visual
  guesses. Full workspace tests pass (149 passed, six ignored).

- A software-reference replay with `USUM_VULKAN_CAPTURE=64` and
  `USUM_VULKAN_SKIP=328000` captured guest draws 328,001–328,064. It completed
  with the same 4,654,983,548 instructions and 334,050 draws. Of 64 isolated
  draws, 42 match exactly. Draw 328,028 (2,550 triangles, lighting enabled)
  is the strongest lead: 11,005 pixels differ by more than one channel level;
  10,583 depth values differ, with maximum depth delta 443,296. The maximum
  color error at target [72,214] is Vulkan [33,25,11,255] versus software
  [14,118,255,0]. Draw 328,003 also has 53,585 differing depth values.
  The diagnostic log is retained locally and is not distributed.
  These isolated failures show the issue is present without live batching;
  inspect those draws' geometry, clipping and raster/depth state next.


### R-070 2026-10-28 — Observe TEV combiner-buffer update latency

- Comparing the software TEV stage loop with WGSL exposed different update
  timing. Software reads the old combiner buffer while evaluating a stage,
  then latches the previous stage's RGB/alpha. WGSL latched those values before
  the stage evaluated its inputs, exposing the newer values one stage early.
- A focused hardware regression starts with a green/alpha-64 buffer and red/
  alpha-191 primary color. With stage-1 buffer writes enabled, a stage-1 read
  incorrectly returned red/191. Moving the latch after stage evaluation makes
  that read green/64 and a subsequent stage-2 read red/191. Both checks pass.
- Added opt-in `USUM_VULKAN_CAPTURE_DIR` diagnostic export for private draw
  geometry, state, textures and before/software/Vulkan buffers. This provides
  concrete inputs for investigating isolated differences without repeatedly
  relying on final-screen symptoms. The outdoor replay separately checks
  whether the TEV timing change fixes the observed terrain problem.

- The exported failing draw confirms buffer reads in stages 2/3 and update
  bits 8/9 (`buffer_update = 773`). With the timing fix, the 182-second live
  Vulkan replay restores the previously black terrain in the island shot.
  At VBlank 10,801, top-screen RGB MAE falls from 9.0499 to 0.5004/255;
  pixels with any channel differing by more than eight fall from 12,869 to
  1,800 of 96,000. The bottom screen is byte-identical. Visual inspection
  confirms green terrain matching the reference; residual differences remain.
- Execution is unchanged: 4,654,983,548 instructions, 334,050 draws,
  95,267,049 triangles, 10,888 VBlanks; 118,021 accelerated lists with no
  software fallbacks. Total time was 236.728 s under concurrent diagnostic/
  build load; this is correctness evidence, not a speed benchmark.
- Validation: 149 workspace tests passed (six ignored), strict renderer/
  runtime Clippy passed, and hardware smoke including the new buffer-latency
  regressions passed. Both generated release executables are rebuilt. The
  later close-up grass captures and full 650-second replay still need to be
  rerun with this fix. Private comparison image:
  retained locally (Vulkan left, software right); not distributed.

### R-071 2026-10-28 — Extend live Vulkan replay validation through grass

- Replayed the same private input and initial saves with acceleration and GPU
  depth feedback enabled. The first attempt requested 650 seconds but hit the
  default instruction cap at 30,626 VBlanks: 17,179,701,101 instructions,
  1,117,523 draws, 430,590,079 triangles, 310,721 accelerated command lists,
  zero software fallbacks, and 56,028 exact D24S8 feedback reads. This is a
  partial replay, not evidence of completing the requested duration.
- Both previously unchecked grass close-ups render correctly on visual
  comparison. At VBlank 23,401, top-screen RGB MAE is 0.1551/255 with 240 of
  96,000 pixels differing by more than eight in any channel. At 27,001,
  MAE is 0.2397 with 842 pixels above that threshold. Both bottom screens are
  byte-identical to software. The earlier corrected island view repeats at
  MAE 0.5004; small coverage/shading differences remain.
- Added `tools/compare-probe-frames.py` for reproducible, pixel-free JSON
  measurement reports. It checks dimensions and payload lengths, matches
  filenames exactly, and reports missing captures with a nonzero exit status.
  Synthetic checks cover whitespace-valued pixels, thresholds, truncated
  payloads, missing captures and periodic-only selection.
- Quiet probe reports now include elapsed/requested game time, and an
  instruction-cap stop explains the `USUM_MAX_REFILLS` override. A one-refill
  private probe run confirms the warning at 0.004 seconds against a requested
  650 seconds. Runtime tests (59), formatting and strict Clippy pass.
- The second run, with `USUM_MAX_REFILLS=65536`, completes the same 650-second
  sequence as software: 22,740,054,712 instructions, 38,889 VBlanks,
  1,537,499 draws and 561,217,399 triangles. Vulkan accelerates 402,408 lists
  with zero software fallbacks and 84,699 exact D24S8 feedback reads.
- All 42 periodic captures (21 per screen) are present. Top-screen RGB MAE
  averages 0.1743/255 across captures, with maximum 0.5004. Sixteen bottom
  captures are byte-identical; the largest bottom MAE is 0.1983 in the final
  animated Pokémon scene, with 126 of 76,800 pixels above eight channel levels.
  Visual checks cover the island, both grass close-ups, later outdoor dialogue
  and the final top/bottom scenes. No black-terrain regression is visible.
  The first 26 repeated screen captures were checked byte-identical to the
  capped Vulkan run. Private captures remain outside the source tree.
- Wall time is 1,010.486 seconds (about 0.64x real time), including 359.207
  seconds in live Vulkan replay. The historical software run took 758.559
  seconds, but these were not controlled side-by-side speed measurements.
  This validates the recorded sequence's rendering and execution, not a
  performance improvement or complete game compatibility. Readback/upload
  synchronization and residual rasterization differences remain next work.

### R-072 2026-10-28 — Reduce Vulkan snapshot and upload overhead

- Ran sequential 182-second replays from the same private saves/input, with
  acceleration and depth feedback enabled, on the Ryzen 7 5800X3D/RX 9070 XT.
  The unchanged baseline takes 208.445 seconds. Excluding host resource caches
  from rollback checkpoints alone takes 215.394 seconds: no demonstrated speed
  win. It remains necessary to avoid copying the new resident framebuffer cache.
- Live targets retain their last successful color/depth readback. The next
  command list still takes a fresh guest snapshot and skips re-upload only if
  dimensions and all color/depth values match. Any target modification invalidates
  this reuse marker until a successful readback; software fallback restores guest
  state while preserving independent host caches, and a GPU error clears them.
  This avoids 9,239 of 23,962 initial target uploads (38.6%) in the replay.
  The residency-only run takes 202.548 seconds, with 70.960 seconds in live replay.
- New preparation timers identify 44.079 seconds in framebuffer snapshot decoding,
  versus 4.825 seconds in textures and 1.834 seconds in vertex packing. Specialized
  RGBA8/D24S8 loops eliminate per-pixel variable-length copies, format dispatch
  and vector growth checks. Other formats retain the generic conversion.
- The final candidate takes 170.404 seconds: 18.25% less elapsed time, or 22.32%
  higher throughput than the unchanged baseline. Snapshot decoding falls to
  8.437 seconds; live replay remains 72.358 seconds. All runs execute the same
  4,654,983,548 instructions, 10,888 VBlanks, 334,050 draws and 95,267,049 triangles,
  accelerating 118,021 lists with zero fallbacks. Both outdoor captures at
  VBlank 10,801 are byte-identical to the baseline. These are sequential local
  measurements, not a controlled hardware benchmark or a game-wide FPS claim.
- A coarser Rayon vertex-batching experiment takes 209.006 seconds and increases
  shading time; it is removed. The original vertex scheduling remains unchanged.
- Regression checks cover rollback preservation/disable paths, exact resident
  reuse invalidation by color/depth/dimensions/NaNs, and specialized decoding
  against generic conversions across tile boundaries. All 62 Vulkan-feature
  runtime tests and 49 default runtime tests pass, as do formatting and strict
  Clippy. Private `play` and `probe` executables are rebuilt. The full 650-second
  validation in R-071 predates these optimizations and must be repeated.
- Next performance work: separate final readback waits from CPU writeback cost,
  then reduce command-list round trips with explicit guest-memory coherence.
  GPU vertex-shader execution and translated-CPU hotspots need separate profiles.
  Uncapping presentation alone cannot produce hundreds of unique game frames;
  simulation/audio timing must also remain correct when increasing frame rate.

### R-073 2026-10-28 — Separate transfers and accelerate CPU writeback

- Added separate timers for final submit/readback (including mapping and host
  copying) and CPU guest-framebuffer writeback. In a 60-second replay of the
  same private recording, submit/readback costs 4.367 seconds, while CPU
  writeback costs 9.224 seconds. This makes CPU conversion the larger immediate
  target; these timers do not claim to isolate pure GPU execution time.
- Specialized RGBA8 and D24S8 writeback with fixed-size loads/stores, preserving
  the generic path for other formats. Depth conversion keeps the existing
  clamping/truncation and leaves each guest stencil byte intact. Byte-for-byte
  regression checks include multiple tile sizes, normalized depths, out-of-range
  values, infinities and NaNs. All 63 Vulkan-feature runtime tests and 49 default
  runtime tests pass, along with strict Clippy and formatting.
- The matching short run takes 40.124 seconds versus 44.587 seconds before this
  change. CPU writeback falls to 3.741 seconds (59.4% less); submit/readback is
  effectively unchanged at 4.410 seconds. Both captures match byte-for-byte,
  as do execution counts: 1,047,731,985 instructions, 3,589 VBlanks, 82,574 draws,
  21,086,424 triangles, 36,864 accelerated lists and zero fallbacks.
- Vertex shaders also gain a read-only fast check of the debug-trace flag before
  its atomic swap. With tracing disabled, shader workers no longer perform a
  shared atomic write for every vertex. A true flag still has one consuming
  swap. Its performance effect is not independently measured.
- The full 650-second replay validates all R-072/R-073 changes together.
  It completes in 771.221 seconds versus 1,010.486 seconds in R-071: 23.68%
  less elapsed time, or 31.02% higher throughput across the recent optimizations.
  These sequential local runs are not controlled hardware benchmarks, and this
  full-run comparison does not isolate today's writeback change.
- All 42 periodic screens are byte-identical to R-071. Execution remains
  22,740,054,712 instructions, 38,889 VBlanks, 1,537,499 draws and 561,217,399
  triangles. All 402,408 lists accelerate with zero fallbacks and 84,699 exact
  D24S8 feedback reads. Guest time reaches 649.999 seconds with every thread
  waiting at the time limit. Private play/probe executables are rebuilt.
- Live replay costs 273.652 seconds, including 77.144 seconds in final
  submit/readback and 54.374 seconds in CPU writeback. There are 62,673 resident
  reuses and 56,788 guest target uploads. Capture preparation costs 40.556 seconds
  for target decoding, 19.001 for textures and 10.229 for vertex packing.
- Overall speed is 0.843x real time. The dense grass intervals remain at
  0.57–0.59x, with roughly 13.5 seconds of CPU shading per 30 seconds of guest
  time. Complete reporting intervals total 155.37 seconds in CPU shading and
  63.49 seconds in other vertex work. Framebuffer transfer optimization alone
  is insufficient: GPU vertex processing, graphics-command overhead and work
  outside command lists require further profiling/optimization before uncapping.

### R-074 2026-10-28 — Batch shader diagnostics and fuse vertex processing

- Vertex shader workers return their step counts locally and publish accumulated
  run/instruction totals once per draw. Previously each invocation performed two
  shared atomic increments; R-052's full replay recorded about 600 million shader
  invocations. Geometry shaders and direct diagnostic calls retain their original
  counting path. Failed shader executions still contribute their steps, invalid
  input loads do not count as shader executions, and first-error order is retained.
- Loading and shading now share one indexed worker pass. Inputs remain local to
  each worker, avoiding the per-draw array of 16 input registers per vertex and
  a second worker-pool synchronization. Ordinary draws retain mapped vertices
  without a second array of raw output registers; geometry draws retain both.
  Timing reports now label the combined work as vertex processing rather than
  reporting it as shader execution alone.
- A guarded display-transfer path directly converts tiled RGBA8 to linear RGB8
  when no downscale is requested. It preserves cropping and vertical flips;
  filtering, tiled output and other formats continue through the generic path.
  New display-transfer timing includes input gathering and guest output writes.
- In the 60-second replay, display transfer costs 2.821 seconds before the direct
  conversion and 0.531 seconds afterward (81.2% less). Combined elapsed time
  changes only from 40.176 to 39.411 seconds; the short test does not demonstrate
  a large overall speedup. Both captures and all execution counters match,
  including 15,445,286 shader runs and 1,830,407,951 shader instructions.
- Tests compare fused outputs bit-for-bit with the original loader/interpreter
  across serial/parallel sizes, repeated indices, NaNs, signed zero and multiple
  memory errors. Display tests compare the direct path against generic conversion
  across tile sizes, cropped/empty outputs, flips and short input. All 66 runtime
  tests with Vulkan and 52 default runtime tests pass, as do formatting and strict
  Clippy. Private play/probe binaries are rebuilt.
- Full replay validation completes with all 42 periodic screens byte-identical
  to R-073. Counters remain 22,740,054,712 guest instructions, 38,889 VBlanks,
  1,537,499 draws, 561,217,399 triangles, 402,408 accelerated lists, zero fallbacks,
  and 84,699 depth-feedback reads. Shader diagnostics match R-052 exactly:
  600,578,761 runs and 64,030,698,169 instructions. This confirms that batching
  did not lose counts or change execution. Its 818.915-second elapsed time is
  excluded from performance comparisons because background encoding and a lower
  replay priority affected the run. Earlier contended timing attempts are also
  excluded; no overall speedup is claimed from them.
- A separate recent user recording provides a shorter gameplay comparison.
  With 40 seconds requested and 16 workers, the R-073 build takes 53.458 seconds
  and the new build takes 47.813 seconds (11.81% higher throughput). Eight workers
  take 46.530 seconds; that smaller difference requires repetition before choosing
  a hardware-specific setting. All 14 captures in each comparison are identical,
  as are 2,563,325,860 guest instructions, 2,392 VBlanks, 107,036 draws and
  30,242,739 triangles. Both worker settings report 26,257,158 shader runs and
  3,019,280,245 shader instructions.
- Repeating in reverse order gives 54.864 seconds for the old build, 48.222
  for the new build at 16 workers, and 47.980 at eight workers. Two-sample means
  are 54.161, 48.0175 and 47.255 seconds respectively. At the unchanged 16-worker
  setting this is 11.34% less elapsed time / 12.79% higher throughput on the recent
  recording. All 14 captures in every repeat remain byte-identical. Eight workers
  average only 1.61% higher throughput than sixteen, below the spread between its
  individual samples, so no universal worker cap is introduced. These are local
  samples, not an isolated hardware benchmark or a game-wide FPS guarantee.
- The short-run benchmark waits for encoders to finish. If encoding starts during
  a sample, it interrupts only its own probe and retries after encoding completes.

### R-075 2026-10-28 — Execute PICA vertex programs in the hardware vertex stage

- Added an in-memory PICA-to-WGSL compiler for arithmetic, relative uniforms,
  integer address registers, comparison, structured calls/branches/loops/breaks
  and forward conditional jumps. Backward/unstructured control flow, recursion
  and geometry emission are rejected for CPU fallback. Expansion limits and a
  conservative uniform-dependent instruction budget bound GPU execution.
- The renderer now accepts raw input registers, per-draw uniforms and triangle
  indices. It executes the translated program in a real graphics vertex stage,
  feeds its outputs directly into existing Vulkan raster/TEV/lighting, and uses
  indexed drawing to reuse transformed vertices. Shader modules and pipelines
  are cached. There is no shaded-vertex readback in the gameplay path.
- Runtime snapshots own immutable inputs/uniforms; unsupported programs and
  geometry draws retain the CPU path. Program caching checks code, descriptors
  and entry point, including direct public-field mutations. Enable only with
  `USUM_VULKAN_VERTEX=1` alongside acceleration. The CPU path remains the default.
- Original synthetic hardware programs match the CPU interpreter for arithmetic,
  matrix DP4, MOVA/relative MOV/MAD, loops, conditional branches, calls and breaks.
  An indexed hardware triangle is byte-identical to the CPU-fed raster path.
  A diagnostic compute harness evaluates the same translated function for these
  comparisons; gameplay uses the vertex stage directly.
- The recent 40-second recording offloads all 107,036 draws and 26,257,158 distinct
  vertex inputs, with zero CPU shader runs, no geometry/program fallbacks and zero
  software raster fallbacks. Guest execution remains 2,563,325,860 instructions,
  2,392 VBlanks and 30,242,739 triangles. All 14 frames are present. Maximum RGB
  MAE versus CPU vertex processing is 0.00003125/255, and no pixel differs by more
  than eight channel levels. Very small float/rounding differences remain.
- This first pass takes 48.084 seconds, comparable to the CPU-vertex R-074 mean
  of 48.0175 seconds. Offload is functional, but no overall speed gain is yet
  established. Next specialize output mapping to eliminate dynamic register-array
  traversal and measure remaining submission/readback costs. This is not evidence
  of achieving the 200 FPS target.
- Validation: 16 renderer and 68 runtime tests with Vulkan, 52 default runtime
  tests, formatting and strict Clippy pass. Tests cover program cache invalidation,
  immutable captured inputs/uniforms and per-program CPU fallback. The complete
  650-second GPU-vertex replay remains pending. Generated game shader source and
  captures stay in memory or private diagnostic locations, outside this repository.

### R-076 2026-10-28 — Specialize GPU outputs and amortize raw input loading

- Hardware vertex pipelines now specialize output assignment for the immutable
  enabled-register mask, semantic maps and texture-unit-2 routing. The mapping
  participates in both shader-module and render-pipeline cache keys. This removes
  the dynamic 16-register/4-component traversal and lets the GPU compiler eliminate
  unused register outputs. Per-draw float/integer/bool uniforms remain dynamic.
- Raw input loading uses workers only at 256+ distinct vertices, with at least
  64 vertices per split. This applies only to the hardware vertex path; CPU
  reference processing retains its existing scheduling.
- Local 40-second samples take 46.902 seconds after output specialization and
  43.527 seconds with coarser loading, versus 48.084 for the initial hardware
  path. These single samples are provisional performance evidence. All offload,
  guest execution and triangle counts remain unchanged, with zero CPU vertex runs
  and zero software fallbacks. All 14 captures retain the same maximum RGB MAE
  0.00003125/255 versus the CPU reference and no pixels above eight channel levels.
- Hardware checks now also cover all comparison selectors, conditional calls,
  forward jumps, nested loops with signed address steps, MADI, special float
  values, sparse output masks and both UV routes. The mapped indexed raster
  tests are byte-identical to the CPU-fed path. Unit tests and strict Clippy pass.
- Probe diagnostics count game framebuffer updates and report their wall/game
  clock rates. They also count consecutive PICA lists per GX-queue drain, to
  determine whether batching can reduce final readback waits safely. These are
  measurements toward the 200 FPS objective, which remains unmet.

### R-077 2026-10-28 — Full hardware-vertex validation and frame-update metrics

- The complete 650-second recording offloads all 1,537,499 draws and 600,578,761
  distinct vertex inputs to hardware. CPU vertex/geometry runs, unsupported-program
  fallback draws and software raster fallbacks are all zero. Guest execution is
  unchanged: 22,740,054,712 instructions, 38,889 VBlanks, 561,217,399 triangles,
  402,408 accelerated command lists and 84,699 exact D24S8 feedback reads.
- All 42 periodic captures are present. Maximum RGB MAE versus R-074 CPU vertex
  processing is 0.00003473/255; no pixels differ by more than eight channel levels.
  Hardware vertex shaders are now the frontend default when Vulkan acceleration
  is requested. `USUM_VULKAN_VERTEX=0` retains the CPU vertex reference path.
- Elapsed time is 556.597 seconds, versus the earlier R-073 CPU-vertex run's
  771.221 seconds. This is a local sequential comparison, not an isolated hardware
  benchmark. Live replay costs 285.996 seconds, with 107.769 in submit/readback and
  54.504 in CPU writeback. Snapshot capture costs 40.230 seconds for targets and
  18.166 for textures; display conversion costs 6.547 seconds for 47,323 copies.
- There are 19,483 top-screen framebuffer updates: 35.00 updates/s wall throughput
  in the unthrottled probe, but 29.97 updates/s against the game clock. This is
  not 200 FPS at normal game speed; the performance objective remains active.
- Every PICA group contains exactly one command list (402,408 groups). Merely
  batching the existing synchronous GX-queue drain therefore cannot remove the
  readback stalls. Next implement safe deferred device writes/coherence across
  guest submissions, preserving CPU-read visibility and rollback behavior.

### R-078 2026-10-28 — Deferred readback and memory coherence foundations

- Added immutable per-submission color/depth staging tickets. Submitting the
  ticket checks GPU validation but does not wait for execution or map results;
  the first reader waits and caches the result. Later target reuse cannot change
  the frozen snapshot, and concurrent readers share the same materialization.
- Added deferred device writes to guest Memory, with checked logical reads and
  exact-range identity lookup. Full overwrites discard tickets without reading;
  partial writes preserve untouched bytes. Failed writes preserve logical data,
  clones retain their frozen versions, and remapping resolves pending bytes.
  No unsafe memory access or change to guest read/write permissions is required.
- Validation: 26 guest, 16 renderer and 68 runtime unit tests pass; strict Clippy
  passes. A hardware example on RX 9070 XT matches synchronous color/depth bytes
  after target reuse/destruction, including padded rows and concurrent readers.
  A 2,000-operation mixed CPU/device sequence matches eager memory throughout.
- These are foundations, not yet the gameplay path. They establish the coherence
  needed to remove per-list stalls; no new gameplay speedup is claimed here.

### R-079 2026-10-28 — Preserve contiguous reads across deferred aliases

- A contiguous mapped slice can span both RAM and pending device ranges. Preserve
  its original readable-slice guarantee by lazily caching the combined region;
  invalidate that cache on CPU/device writes, overlay replacement and remapping.
  This prevents texture and DMA callers from mistaking valid mixed data for an
  unmapped range. Ordinary RAM reads and exact device slices keep their fast paths.
- The 2,000-operation eager/deferred differential check now also compares whole
  mapped slices after every operation. All 26 guest tests and strict Clippy pass.

### R-080 2026-10-28 — Experimental deferred Vulkan replay integration

- `USUM_VULKAN_DEFER_READBACK=1` connects immutable GPU readback tickets to guest
  device memory for non-overlapping RGBA8/D24S8 targets with nonzero depth. Both
  exact color/depth provider identities must match the current GPU target before
  reusing it without importing guest bytes. A CPU write removes that proof; old
  Memory clones still read their original frozen staging snapshots.
- Stencil bytes are inherited independently of depth readback, and full target
  overwrites discard superseded tickets. Unsupported target formats retain the
  synchronous path. Deferred read failures surface as device memory faults rather
  than stale pixels. Submission validation retains the existing software fallback.
- Ordinary scalar RAM writes retain a direct fast path. Deferred range checks
  distinguish actual overlaps from gaps between device buffers, avoiding per-byte
  reads or whole-region slice materialization for unrelated data in those gaps.
- All 26 guest, 16 renderer and 68 runtime unit tests pass; strict Clippy passes.
  Two explicit hardware tests verify frozen guest planes/stencil and byte-identical
  multi-list replay versus synchronous Vulkan, including a partial guest CPU write
  between lists. Probe counters report queued/materialized tickets and wait/encode
  time. The switch remains off by default pending recorded gameplay and timing
  validation. The 200 FPS objective remains unmet; no new speedup is claimed yet.

### R-081 2026-10-28 — Controller input and analog replay

- The window feature now uses gilrs for standard controller mappings and hot
  plugging. The first connected pad supplies Xbox-position face buttons, D-pad,
  bumpers, Start/Select and a proportional left stick with a radial dead zone.
  Input is neutralized on focus loss; a disconnected controller no longer holds
  buttons. Keyboard and controller input combine without canceling each other.
- One atomic live snapshot carries buttons and both signed stick axes coherently.
  HID receives the actual analog position; recordings preserve it as Cx:y tokens.
  Older recordings retain their direction-bit stick behavior unchanged.
- Added tests for radial range/dead-zone behavior, keyboard/controller merging,
  analog recording round trips, release and invalid coordinate rejection. All
  71 enabled runtime tests and the 53-test default runtime pass. Linux builds need
  pkg-config and libudev development files; the dependency audit and CI list them.
  Physical Xbox-controller gameplay still requires an end-to-end user test.

### R-082 2026-10-28 — Internal resolution scaling and scaled LCD presentation

- Added 1×–8× Vulkan internal resolution (`USUM_RESOLUTION_SCALE`), with F3/F4
  changing scale during play. The requested first user test is 6× (2400×1440 top).
  Changing scale discards resident target/display caches while preserving native
  guest memory, input coordinates and saves.
- GPU targets, viewports, snapshot seeds and depth-feedback stencil planes scale
  together. Readbacks retain the full-resolution color image while resolving native
  guest color/depth. Supported linear LCD transfers preserve the high-resolution
  image through cropping, hardware downscaling, flipping and 3DS display rotation;
  CPU writes invalidate presentation reuse through native-byte comparison.
- Native resolution stays the default. Other presentations and software fallbacks
  use native frames. Deferred readback is temporarily restricted to 1×; scaled
  rendering still uses synchronous transfers and needs optimization/measurement.
- All 73 enabled runtime tests pass, as does strict Clippy. LCD transform tests
  agree with the native transfer and rotation reference for flips, crops and both
  hardware downscales. A hardware 6× test produces 192×192 raster data from a
  32×32 guest target and verifies variation inside native pixel blocks, proving
  real subpixel rasterization rather than nearest-neighbor enlargement. Native
  guest buffers retain their sizes. Full scaled gameplay validation is pending.

### R-083 2026-10-28 — Fix scaled LCD subregions in real gameplay

- User screenshots disproved the first 6× presentation: the real game still
  displayed native images despite larger GPU targets. A bounded trace showed
  zero scaled LCD hits. The game's top and bottom transfers begin 112 and 192
  rows inside a 256×512 padded render target; matching only the exact target
  base address rejected both screens.
- Recognize tile-aligned subregions with the same tiled row stride, validate
  only the rectangle actually read by the cropped transfer, and carry its origin
  through full-resolution rotation/downscaling. Bounds, format and native-byte
  coherence checks still reject incompatible or stale views. The common unscaled
  LCD conversion avoids averaging/division for every high-resolution pixel.
- New unit cases cover the actual top/bottom crop origins and rejected stride,
  alignment and bounds cases. A hardware test follows real 6× rasterization
  through a padded-buffer crop, LCD transfer, rotation and presentation selection;
  subpixel edges survive, and a changed guest framebuffer rejects the cached view.
- Actual recorded gameplay now produces both scaled LCD hits and a visibly sharper
  2400×1440 overworld capture. At VBlank 481, 55,687 of 96,000 native pixel blocks
  contain more than eight channel levels of internal detail. The prior presentation
  supplied no high-resolution frame at all. All 74 enabled runtime tests and strict
  Clippy pass. `USUM_SCALE_TRACE=1` enables bounded diagnostics and periodic scaled
  PPM captures alongside native probe captures (private game-derived evidence).

### R-084 2026-10-28 — Remove repeating road lighting bands; profile the intro

- Both renderers reproduced the bright bands. Per-draw captures isolated them to
  the road's lighting, rather than its textures or resolution scaling. PICA lighting
  LUT differences store an 11-bit magnitude and a separate sign bit; interpreting
  them as two's complement turned tiny negative slopes, including negative zero,
  into large sawtooth brightness changes. Correct both CPU and WGSL decoding.
  Hardware field reference: [Azahar PICA core LUT layout](https://github.com/azahar-emu/azahar/blob/master/src/video_core/pica/pica_core.h).
- Restore programmable float24 depth scale/offset and Z/W buffer mapping before
  depth quantization in both renderers. This is a separate correctness fix; depth
  changes alone did not remove the road bands.
- CPU tests cover signed magnitudes, interpolation continuity and programmed depth.
  Vulkan tests cover negative zero/tiny descending slopes, coplanar depth bias and
  W-buffer depth. A recorded overworld replay now has a smooth road; at its VBlank
  900 capture, GPU/reference mean channel difference is 0.301/255. All 76 enabled
  runtime tests, 16 renderer tests and strict Clippy pass.
- Add Y2R conversion-job timing. In a diagnostic 20-game-second intro run, 1,184
  conversion jobs took 0.982 seconds out of 57.160 seconds elapsed. Guest execution
  was the dominant cost, with billions of instructions and frequent movie-module
  calls into the executable. Profiling/compilation overlapped this diagnostic, so
  these wall times are not a controlled performance benchmark. Intro playback
  and scaled gameplay performance remain unresolved; no 200-FPS claim is made.

### R-085 2026-10-28 — Cache host dispatch during movie playback

- MovieLib repeatedly crosses into executable helpers and returns through the host
  dispatcher. Those transitions repeated executable/module entry searches and
  translation-identity searches. Cache resolved code by guest address including
  the instruction-state bit. Each cached module entry still validates its loaded
  index, base, file size, name and SHA-256; unloading, replacing or relocating a
  module cannot reuse stale code. Executable entries retain dispatch precedence,
  and their calls preserve the current module delta as before.
- `USUM_DISPATCH_CACHE=0` retains uncached lookup for reference runs. The cache
  changes no guest instructions, budgets, clock rates or video decoding. Lifecycle
  tests exercise cached hits, ARM/Thumb separation, unload, replacement, relocation,
  changed identity and bounds, and removal from the loaded-module list.
- Controlled intro comparison used an initialized save, ten game seconds, no input
  or guest-call profiling, native Vulkan rendering, and off/on/on/off ordering.
  Uncached runs took 24.911/24.570 seconds; cached runs 18.851/19.077 seconds.
  Average elapsed time fell 23.3%, equivalent to 30.5% higher throughput. Every run
  executed exactly 2,659,171,725 guest instructions, 584 conversion jobs and 586
  top-screen updates. Both captured screens at VBlank 300 are byte-identical across
  all four runs. No encoder was active. All 77 enabled runtime tests and strict
  Clippy pass. Movie playback remains below real time (about 0.53×); this is a
  measured improvement, not a claim of full-speed playback or 200-FPS gameplay.

### R-086 2026-10-28 — Keep scaled framebuffer traffic native until presentation

- At 6× the old path expanded guest color/depth on the CPU, uploaded 36 times as
  many pixels, and read full-resolution color/depth back after command lists.
  Add a render pass that expands native seeds on the GPU and a compute pass that
  freezes native center samples into immutable staging tickets. Full-resolution
  color stays on the GPU until an LCD transfer needs it. Guest memory, depth
  quantization, stencil handling and CPU-write coherence retain their semantics.
- Color/depth GPU tests cover scales 1/2/3/6/8, odd dimensions and padded rows,
  all channel values, exact depth samples and frozen results after target reuse
  and destruction. The 6× presentation test compares GPU-native samples to the
  old full readback and verifies exact LCD rotation/cropping, subpixel edges and
  rejection after CPU writes. All 77 enabled runtime tests, six runtime hardware
  tests, 16 renderer unit tests, both new renderer hardware tests and Clippy pass.
- In the same recorded 16-game-second 6× overworld replay, elapsed time went
  from 123.923 seconds to 87.307 with native readbacks, then to 51.761 with GPU
  seed expansion too: 2.39× baseline throughput. All runs execute 1,170,300,377
  guest instructions, 45,832 draws and 956 VBlanks, with no software fallbacks.
  Both native and scaled LCD captures at VBlank 900 are byte-identical. Throughput
  remains only 9.62 top-screen updates/second, far short of full-speed 6× gameplay
  or the 200-FPS goal. Controlled intro measurements are separate.
- `USUM_VULKAN_NATIVE_READBACK=0` and `USUM_VULKAN_SCALED_UPLOAD=0` retain the old
  paths for reference measurements. Deferred guest readbacks remain opt-in at 1×.

### R-087 2026-10-28 — Perform LCD crop, rotation and RGB packing on the GPU

- Replace the large strided CPU RGBA scan with a compute pass that crops,
  rotates, flips, averages hardware-downscaled samples and packs four RGB pixels
  into three words. Only tightly packed display RGB reaches the CPU; high-resolution
  RGBA and depth stay resident. CPU source-byte coherence checks are retained.
  `USUM_VULKAN_GPU_DISPLAY_COPY=0` selects the prior CPU transform for reference.
- Hardware comparisons match the existing CPU transform exactly at scales
  1/2/3/6/8, across cropping, both flip states, all four 1×/2× sampling-factor
  combinations and RGB tails that do not fill a four-pixel group. The real padded
  6× LCD-view test also passes without materializing a CPU RGBA plane.
- The recorded 16-game-second 6× replay now takes 29.217 seconds, versus
  51.761 after R-086 and 123.923 originally: 17.04 top-screen updates/second and
  4.24× original throughput. Instructions, draws and VBlanks are unchanged; both
  native and high-resolution captures of both screens are byte-identical.
- A controlled 6× intro comparison used CPU/GPU/GPU/CPU ordering over six game
  seconds. CPU transforms took 23.782/24.386 seconds; GPU transforms 13.638/14.154.
  Each executes 1,583,340,348 guest instructions, 344 conversion jobs and 347
  top-screen updates. All native/scaled captures at VBlank 180 are byte-identical.
  Including R-086, intro throughput is 4.78× the original 65.991/66.806-second
  reference. It still runs below real time, and the 200-FPS goal remains unmet.

### R-088 2026-10-28 — Index full guest pages without weakening memory checks

- Add an immutable, cloneable 4-KiB-page-to-region index for address spaces with
  at least eight regions. Partial pages retain the original region search, so
  adjacent subpage mappings, holes, permissions and wraparound accesses keep
  their checked semantics. Mapping, remap rollback and coalescing rebuild the
  index; memory clones retain independent region layouts and a shared immutable
  index. No raw host pointers or unsafe code are introduced.
- `USUM_GUEST_PAGE_INDEX=0` selects the original region lookup. A lifecycle test
  compares indexed/reference byte and word accesses across partial/full pages,
  permissions, cross-page writes, relocation, failed remaps, coalescing, a new
  lowest region, clone independence and the top of the 32-bit address space.
  All 27 guest tests and strict Clippy pass.
- Repeated 6× intro off/on/on/off runs took 15.676/10.818/10.568/15.216 seconds
  for six game seconds: 44.4% greater throughput versus disabled indexing in the
  same binary. The 16-game-second 6× overworld replay took 27.701 seconds off and
  25.367 on (17.98 versus 19.63 top-screen updates/s). Instructions, video jobs,
  draws and VBlanks match, and every native/scaled capture is byte-identical.
  Intro playback and gameplay remain below real time; 200 FPS is not achieved.

### R-089 2026-10-28 — Recover controllers connected after launch

- The test environment is a toolbox container: startup udev enumeration works,
  while host hotplug broadcasts need not reach its namespace. Select Gilrs'
  device-directory notification backend in Linux containers, including its
  permission-change events. A safe early self-exec supplies the discovery flag
  without mutating process-global environment around running threads.
- Select newly connected or recently active pads rather than always using the
  first connected receiver/virtual pad. Disconnect clears the selected device;
  failed backend startup retries with deduplicated errors. Healthy empty backends
  stay running, avoiding repeated discovery-thread creation. Advance Gilrs' event
  counter on each input poll.
- Selection tests cover startup without a pad, a late connection behind an idle
  receiver, disconnect/fallback, reconnection and subsequent activity. Existing
  analog/keyboard merging tests pass. The container bootstrap self-exec and
  usage path were smoke-tested without opening a window. Physical post-launch
  reconnection remains a user-hardware retest, not a claimed automated result.

### R-090 2026-10-28 — Preserve scaled depth across battle color-buffer switches

- The reported striped/translucent battle models reproduce at 6× with either
  CPU or GPU vertex shading; native-resolution captures remain solid. The battle
  changes color attachments while reusing a guest depth allocation. Expanding
  its native center samples back to 6× loses the depth prepass between native
  pixel centers, causing later model fragments to fail depth tests in a grid.
- Before replacing a scaled target's color seed, find the most recent compatible
  GPU depth whose native snapshot still matches the guest's depth. Freeze it in
  a GPU-only depth texture, then restore it after seeding and before drawing.
  Dimensions, depth address/format and exact native-depth comparisons guard reuse;
  changed CPU depth falls back to the normal seed path. No depth tests are disabled.
- A synthetic sloped-depth prepass/color-buffer-switch test failed before the
  fix (7,739 filled pixels versus 13,220 expected), and now matches the complete
  reference image exactly. A separate hardware test verifies frozen depth survives
  source overwrite, reuse and destruction. The recorded battle now shows solid
  trainer and Pokémon models at 6× instead of the reproduced holes.
- All 78 enabled runtime tests, eight runtime hardware tests, three renderer
  hardware tests and strict Clippy pass. Private game captures remain outside Git.

### R-091 2026-10-28 — Reduce intro dispatch and redundant depth-copy overhead

- User-space CPU sampling identifies guest word reads, writes and region lookup
  as about 35% of intro cycles, with dispatch resolution and its keyed hash another
  9%. Replace the dispatch cache's string-oriented hash with a mixed integer hash
  for its verified code-address keys. Module unload/replacement/relocation and
  instruction-state validation remain unchanged.
- Track whether a scaled depth surface contains raster-written detail. Preserve
  native-seeded depth without an extra full-resolution snapshot when no raster
  write has occurred; surfaces with subpixel depth still take the battle fix's
  immutable GPU-copy path. The sloped-depth battle regression test still passes.
- At 6×, old/new/new/old intro runs take 11.018/10.187/10.227/11.295 seconds for
  six game seconds: 9.3% greater throughput. Every run executes 1,583,340,348 guest
  instructions, 344 conversion jobs, 688 draws and 347 top-screen updates. All
  four native/scaled screen captures match byte-for-byte across all four runs.
- One 16-game-second overworld pair takes 29.301 versus 28.394 seconds, a modest
  3.2% throughput increase needing further repetition. Instructions, draws and
  498 top-screen updates match, as do all four captures. Enabled runtime tests,
  strict Clippy and the hardware depth-prepass regression pass. These are headless
  update-throughput measurements; real-time playback and the 200-FPS goal remain
  unmet. Sampling data, binaries and game captures stay outside the repository.

### R-092 2026-10-29 — Inline checked guest region lookup into memory access

- CPU samples showed an out-of-line region lookup on the hot guest load/store
  path. Inline that private helper so callers can eliminate its tagged-pair return
  and combine region access with the page lookup. Page bounds, permissions,
  partial mappings and deferred visibility keep their existing checks.
- Frozen pre/post binaries at 6× run the same six-game-second intro in
  10.644/9.476/9.064/10.232 seconds (old/new/new/old), a 12.6% throughput gain.
  All runs execute 1,583,340,348 instructions, 344 conversion jobs, 688 draws and
  347 top-screen updates. Every native and scaled capture matches exactly.
- A single overworld pair measures 24.917 versus 24.392 seconds for 16 game
  seconds, only 2.2% greater throughput, with matching instructions, draw counts
  and all four images. This small gameplay difference needs repetition; GPU
  timing also varies. All 27 guest tests and strict Clippy pass. Intro and gameplay
  remain below real time at 6×; this does not achieve 200 FPS.

### R-093 2026-10-29 — Preserve command-list order and current depth aliases

- Batch only consecutive draws to the same target. A/B/A attachment switches
  previously merged both A groups ahead of B. Within a list, captures share its
  initial guest memory, so returning to A must also inherit the most recent
  compatible GPU depth from B, including at native resolution. Track target
  recency on every group and select the newest compatible feedback source.
- Read each target after its last group and commit native readbacks in actual
  target order. This prevents a late readback of an earlier alias from overwriting
  newer physical depth. A hardware test covers native/6× A/B/A occlusion, both
  directly and across a texture-feedback barrier, and checks complete color output
  plus final guest depth. It fails before the fix and passes afterward.
- All 78 enabled runtime tests, nine runtime hardware tests and strict Clippy pass.
  The four overworld and 28 battle native/scaled captures stay byte-identical.
  This fixes a separate ordering bug; it does not resolve the reported outlines.
- The user's requirement is clean outlines in the overworld and battle, enabled
  by default, with a future option to disable them. A private overworld trace
  identifies enabled stencil tests and replacement writes of model IDs. Current
  software/Vulkan raster paths omit stencil execution and preserve old bytes;
  proper stencil handling is the next rendering investigation. Temporary trace
  instrumentation and game-derived payloads are excluded from Git.

### R-094 2026-10-29 — Execute stencil operations and restore overworld outlines

- The overworld replaces stencil bytes with model IDs, which later passes sample
  as the red channel of D24S8 reinterpreted as RGBA8. Both raster backends previously
  omitted stencil operations and retained old guest bytes. Implement masked
  comparisons, write permissions, all eight operations and the correct failure /
  depth-failure / pass selection. Alpha rejection suppresses stencil changes.
- Vulkan uses float depth plus stencil, dynamic reference values and pipeline
  keys that exclude those references. Seed stencil from native snapshots, keep
  uniform clears on the GPU, preserve both aspects across aliases and immutable
  snapshots, and resolve stencil alongside native color/depth. Feedback samples
  the actual rendered stencil directly, removing CPU expansion/upload of old
  stencil. Deferred planes now freeze rendered stencil rather than old guest data.
- Restore stencil when replaying isolated diagnostic draws and include stencil
  in comparisons and private diagnostic exports. Native snapshots and residency
  comparisons include stencil. Adapters without the required float-depth/stencil
  format report an error and retain the software rendering fallback.
- CPU and hardware tests cover operations, masks, saturation/wrap, depth and
  alpha failures, exact odd-sized 1/2/3/6/8× seeds/readbacks, immutable snapshots,
  dynamic-reference pipeline reuse, deferred guest writes and framebuffer aliases.
  All 80 enabled runtime tests, 16 enabled renderer tests, nine runtime hardware
  tests, five renderer hardware tests and strict Clippy pass.
- The recorded 6× overworld now visibly outlines the player, with zero software
  fallbacks; battle models remain solid. Scaling of outline edges is still too
  coarse and remains under investigation, including depth/stencil reads across
  command lists. Outlines remain enabled by the game's default rendering state;
  the requested future outline-disable setting is not implemented in this change.
  The 200-FPS target remains unmet. Game captures stay private.

### R-095 2026-10-29 — Preserve scaled framebuffer textures across command lists

- Later lists previously decoded framebuffer color or D24S8 textures from native
  guest bytes, losing subpixel detail from earlier 6× rendering. Keep immutable
  GPU texture views for compatible RGBA8 color and D24S8 reads. Exact native
  data, address, dimensions, format, sampler and border checks guard reuse;
  CPU changes fall back to normal texture decoding.
- A new immutable snapshot identity distinguishes GPU generations even when
  their native center samples are byte-identical. Prune obsolete cached views,
  select the newest compatible source, and distinguish color from depth/stencil.
  Different texture formats or framebuffer subviews keep the existing path.
- A GPU regression moves geometry between native pixel centers without changing
  native bytes. Both color and stencil reads in a later list must still change
  at full resolution and match a direct GPU reference. CPU writes reject reuse.
  An asymmetric odd-sized color test independently checks channels, row direction
  and immutable output after the source is overwritten and destroyed.
- Recorded overworld and battle replays confirm cleaner 6× outlines; intermediate
  color reads caused the remaining coarse battle contour. All 82 enabled runtime
  tests, 16 enabled renderer tests, ten runtime hardware tests, six renderer
  hardware tests and strict Clippy pass. `USUM_VULKAN_RESIDENT_TEXTURES=0` retains
  the previous native-upload texture path for diagnosis. Outlines stay on by default.
- Separately measured R-094 stencil implementation: old/new/new/old intro runs
  take 9.146/9.368/9.707/9.143 seconds for six game seconds, a 4.3% wall-time
  regression under investigation. Overworld takes 24.237/20.329/20.754/23.657
  seconds for 16 game seconds: 16.6% greater update throughput, about 24 versus
  21 top-screen updates/s. Guest instructions, draws and updates match. These
  measurements exclude this texture-residency change; 200 FPS remains unmet.

### R-096 2026-10-29 — Fuse depth/stencil conversion and avoid reading replaced planes

- Decode native D24S8 depth and stencil in one tiled pass. Encode rendered depth
  and stencil together, retaining old stencil only when no replacement is
  supplied. Complete D24S8 overwrites no longer read/copy old guest bytes first.
  Deferred encoding uses the same combined pass.
- Golden format tests check depth bits and stencil across tiles. A deferred
  provider that faults on every read proves a complete rendered overwrite never
  materializes old data. All 83 enabled runtime tests, ten runtime hardware tests
  and strict Clippy pass.
- Frozen old/new/new/old 6× intro runs take 9.659/9.358/9.520/9.496 seconds for
  six game seconds, about 1.5% more throughput. The 16-game-second overworld runs
  take 20.894/18.693/20.008/20.421 seconds, about 6.8% more throughput on average,
  with noticeable variation between the two new runs. Instructions, draws,
  updates and all native/scaled captures match exactly. The result averages about
  26 headless top-screen updates/s; it does not establish normal-speed 200 FPS.
- Separately, R-095's full-resolution texture change measured 9.382/9.463/9.622/
  9.298 seconds for the intro and 20.448/20.772/20.279/20.017 for the overworld.
  Its small overhead preserves the corrected high-resolution effect chain rather
  than substituting native-resolution intermediate textures.

### R-097 2026-10-29 — Validate experimental deferred readbacks at 6×

- Resolve immutable readback tickets to native dimensions at every internal
  scale. Reject tickets with the wrong guest dimensions. Already materialized
  tickets supply a native snapshot identity for full-resolution GPU textures
  and LCD presentation without introducing a new wait for unused tickets.
- Hardware tests compare native and full-resolution results at 1× and 6× across
  multiple lists and partial CPU writes. Resident color and stencil tests also
  cover deferred 6× generations whose native samples are unchanged, and verify
  the CPU texture reader actually materializes each lazy snapshot.
- All 36 recorded intro, overworld and battle native/scaled captures match the
  corrected synchronous renderer byte for byte, with zero software fallbacks.
  The initial mode benchmark was invalid: this option tests environment-variable
  presence, so assigning `0` still enabled deferred mode. Those timings do not
  compare synchronous and deferred performance. Corrected comparisons remove
  the variable and verify the logged mode; see R-098. Deferred mode remains
  opt-in while performance validation continues. The 200-FPS target remains
  unmet. Captures and game inputs stay private.

### R-098 2026-10-29 — Omit unused LCD input padding and correct mode measurements

- Common unfiltered tiled RGBA8-to-RGB8 LCD copies now read only the prefix
  containing sampled texels. The 256×512 top/bottom inputs need 409088/327168
  bytes for 240×400/240×320 crops instead of 524288 each. This also avoids
  crossing a deferred framebuffer boundary merely to read unused padding.
  Other formats, filters, tile modes and unaligned strides retain full reads.
- Validate checked input sizes and empty output. Compare cropped/flipped prefix
  conversion against the full-input generic format oracle, and verify removing
  the final required byte fails. All 84 enabled runtime tests and strict Clippy
  pass. Recorded native and 6× intro/overworld/battle captures remain byte-exact;
  the deferred cases have zero software fallbacks.
- Correct the initial R-097 benchmark: the readback option is presence-based,
  so `USUM_VULKAN_DEFER_READBACK=0` still enables it. New runs unset it for
  synchronous mode and explicitly check the logged mode after each trial.
  Old synchronous, trimmed synchronous and trimmed deferred intro pairs take
  9.342/9.335, 9.248/9.182 and 9.152/9.324 seconds; overworld pairs take
  20.119/19.072, 20.234/20.018 and 19.816/19.112 seconds respectively. Deferred
  mode remains opt-in; these small differences do not establish a robust gain.
- `USUM_DISPLAY_FULL_INPUT=1` retains a same-binary full-read reference. Repeated
  full/trimmed/trimmed/full intro times are 9.328/9.251/9.211/9.184 seconds;
  overworld times are 19.843/19.480/19.383/18.858. All captures, instruction and
  draw counts agree. No consistent FPS gain is claimed from trimming alone.
- Fresh user-space cycle sampling places guest word reads/writes at about 32%
  of intro CPU samples and dispatch lookup at 7.6%. Gameplay samples show guest
  word access around 11%, vertex loading 4.2%, draw encoding 3.9%, and combined
  depth/stencil conversion 5.5%. These identify remaining CPU work, not an FPS
  result. Profiles and game data stay private. The 200-FPS objective is unmet.

### R-099 2026-10-29 — Reject shared module-identity dispatch experiment

- Tested immutable shared CRO metadata and SHA strings, guarded by weak
  allocation identities in the dispatch cache. Copy-on-write edits, replacement,
  unload, relocation and instruction-state changes invalidate the guard; weak
  references release removed module tables. All 84 runtime tests and strict
  Clippy passed, and 28 battle captures matched the corrected renderer.
- Old/new/new/old intro times were 9.613/9.101/9.179/9.160 seconds; overworld
  times were 18.993/20.006/20.216/19.768 seconds. New gameplay averaged about
  3.8% slower. Separate retired-instruction measurements dropped from 123.686
  to 122.402 billion in the intro and 186.400 to 186.183 billion in gameplay,
  only about 1% and 0.1% respectively. Gameplay cycles and wall time increased.
- Discarded the implementation rather than ship a gameplay regression. The
  existing module name/hash/size/base guards remain. This experiment does not
  advance the verified FPS result; it redirects work toward larger framebuffer
  readback costs. Private binaries, profiles and a rejected patch remain outside
  Git. The normal-speed 200-FPS goal is still active and unmet.

### R-100 2026-10-29 — Unblock opening the bag in battle

- The user's recorded session stopped at VBlank 2263 on unimplemented HID
  command `0x00110000`, EnableAccelerometer. Implement the documented enable /
  disable pairs for accelerometer and gyroscope on hid:USER and hid:SPVR.
- Track sensor state and publish neutral six-byte vectors in the documented
  8-entry accelerometer and 32-entry gyroscope rings. Advance timestamps on
  entry-zero updates and signal only enabled sensor events. Disable stops
  updates; re-enable initializes a fresh entry-zero sample. Physical host motion
  input is not connected; this supplies neutral simulated samples.
- Independent service facts: [libctru HID commands](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/hid.c)
  and [3dbrew shared-memory layout](https://3dbrew.org/wiki/HID_Shared_Memory).
  No emulator implementation was used.
- Regression tests cover malformed requests, read-only shared mappings, both
  ring sizes and wrapping timestamps, adjacent memory, disable/re-enable and
  correct per-sensor event signaling. All 86 enabled runtime tests and strict
  Clippy pass.
- Replay of the exact input/save reaches 45 game seconds, past the original
  stop, with the bag visibly rendered at 6×. A second replay adds B to close the
  menu and A to reopen it, reaches 50 seconds and captures the reopened bag.
  Both stop at their configured sleep/time budget, with no unsupported command
  and zero software rendering fallbacks. Game data and captures remain private.
  This fixes a gameplay stop; it does not establish the 200-FPS goal.

### R-101 2026-10-29 — Keep deferred framebuffer textures on the GPU

- Freeze compatible full-plane RGBA8 color and D24S8-as-RGBA textures directly
  from their proven resident GPU generation. Exact immutable deferred-provider
  identity, dimensions and format replace eager CPU pixel decoding. Cache GPU
  views per source generation / plane / sampler / border without ownership cycles.
  CPU writes and incompatible layouts retain ordinary decoding. Within-list
  feedback still takes precedence over the frozen incoming view.
- A fallible on-demand native reference supports isolated rendering, diagnostics
  and export. Software fallback still reads authoritative logical guest memory.
  GPU tests verify capture/sampling never map the source, output survives source
  reuse/destruction, native references match regular decoding, and CPU writes
  invalidate reuse. Extended color/stencil tests move subpixel geometry while
  native samples remain identical; both generations must render correctly.
- All 86 enabled runtime tests, eleven runtime hardware tests and strict Clippy
  pass. All 36 intro/overworld/battle native and scaled captures match. The latest
  complete user recording also matches all 24 captures, 5,117,853,423 guest
  instructions, 165,848 draws and 3708 VBlanks at the 62-second budget. There
  are no software fallbacks. Concurrent session checks establish correctness;
  their wall times are not performance measurements.
- Sequential same-binary 6× overworld pairs: synchronous 19.792/18.622 seconds,
  deferred-only 19.213/18.500, frozen/deferred 18.254/18.605 for 16 game seconds.
  The combined path averages about 4.2% more throughput than synchronous, around
  27 top-screen updates/s, with variation between runs. Materialized tickets fall
  from 2004 to 1666; 2388 texture captures stay on the GPU.
- Battle synchronous/frozen/frozen/synchronous times: 39.628/30.030/29.571/39.969
  seconds for 30 game seconds, about 33.6% more throughput for the combined path.
  It averages 32.6 versus 24.4 top-screen updates/s; all captures agree. There
  are 4620 frozen texture captures and 3050 materialized tickets versus the
  deferred-only correctness replay's 3579. No gain is attributed to this texture
  change alone when comparing against synchronous mode.
- Enable the validated combined path by default in play/probe. Explicit
  `USUM_VULKAN_DEFER_READBACK=0` or `USUM_VULKAN_DEFER_TEXTURES=0` retain reference
  modes; the legacy full-readback switch remains honored. This supersedes the
  earlier presence-based readback option. Rendering correctness and the CPU
  fallback remain required. Game data stays private. Normal-speed 200 FPS has
  not been reached; the goal remains active.

### R-102 2026-10-29 — Handle offline cleanup and DSP shutdown/suspend

- One recent menu session stopped on FRD Logout (`0x00040000`). Reply success
  for the already-offline console on both friend service names. Handle the next
  APT sleep-query and sleep-complete acknowledgements by recording their values;
  the virtual shell stays open and awake. Real IPC tests cover repeated logout
  and acknowledgements on all three APT service names.
- Implement the documented NDSP audio lifecycle: state 1 shutdown / 3 suspend
  stops audio processing and exposes acknowledgement 1 in receive register zero.
  State 0 initialization / 2 resume publishes fresh structure addresses. Treat
  the command as u16, accepting the observed nonzero upper padding bytes. Register
  reads consume the acknowledgement; component unload clears pipe state.
  Disabled processing no longer schedules or emits audio frames. Preserve the
  existing frame deadline when ordinary initialization occurs before it.
- Protocol facts: [libctru friend service](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/frd.c),
  [APT commands](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/apt.c),
  [NDSP lifecycle](https://github.com/devkitPro/libctru/blob/master/libctru/source/ndsp/ndsp.c).
  No emulator implementation was used.
- All 90 enabled runtime tests and strict Clippy pass. The normal intro and
  overworld retain identical guest instruction/draw counts and all eight native /
  scaled captures, with zero renderer fallbacks. These concurrent correctness
  replays are not performance measurements.
- The previously stopped recording now passes these calls and reaches
  `APT:PrepareToDoApplicationJump`, flags 2: restart the current title. Actual
  application restart remains unsupported and is the next correctness task;
  this change does not claim the whole menu-return path is fixed. See the
  [documented jump flags](https://www.3dbrew.org/wiki/APT%3APrepareToDoApplicationJump).
- Separately, the latest manual 6× test closed normally after 11969 VBlanks,
  719680 draws and 134566 Vulkan lists, with zero software fallbacks. Recordings
  and game data stay private. The normal-speed 200-FPS objective remains active.

### R-103 2026-10-29 — Restart the title when leaving Mystery Gift

- Both the earlier short recording and the latest reported freeze stop at
  `APT:PrepareToDoApplicationJump` with flags 2 (current application). The game
  then releases its GPU right and sends `DoApplicationJump` with empty delivery
  buffers. This is process termination and relaunch, not a return into the old
  game's torn-down runtime. Follow the command formats and self-chainload flag
  in [libctru APT](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/apt.c)
  and [GPU-right release](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/gspgpu.c).
- Capture a pristine CPU/address-space boot image before execution. A prepared
  self-jump restores it and creates fresh kernel handles, threads, mappings,
  loader state, audio sources, GPU registers, and framebuffer/texture caches.
  Invalidate translated dispatch ownership too. Keep the host session's save
  directory, RomFS, controller/touch state, quit flag, audio sink, display sink,
  current resolution/Vulkan policy, cumulative counters and monotonic clock /
  input-recording timeline. In particular, never restore an old save snapshot
  or reset recorded-input indices. Delivery data is copied before termination
  and can be received once by the new process. Other-title jumps stay unsupported.
- The original recording now restarts at VBlank 950, reaches the visible title
  screen and runs through the 30-second limit (8767 lists, zero fallbacks).
  Repeating the same menu sequence restarts twice at VBlanks 950 and 1900 and
  runs to the 45-second limit (12235 lists, zero fallbacks).
- The user's latest recording now restarts at VBlank 3728 and keeps running
  through 78 seconds (4664 VBlanks, 25117 lists, zero fallbacks), beyond its
  previous freeze. These are automated functional replays, not FPS benchmarks.
- All 93 enabled Vulkan/window runtime tests and 68 default-feature tests pass,
  together with strict Clippy for both configurations. New IPC tests verify
  delivery bytes, pristine boot memory, terminated handles/mappings, preserved
  session state and rejection of unprepared/invalid jumps. GPU-state reset tests
  preserve 6× acceleration policy and cumulative counts. The standard intro and
  overworld retain exact instruction/draw counts and all eight reference captures.
- The 200 actual rendered FPS objective is still unmet; this fixes the menu
  correctness regression before continuing performance work.

### R-104 2026-10-29 — Keep independently owned color/depth planes resident

- A color target can still own its exact logical color plane after another
  color target updates their shared physical depth/stencil allocation. The old
  all-or-nothing proof mapped native tickets and re-imported color in this case.
  Prove each plane independently through the actual deferred-memory provider
  identity, dimensions and format. Retain valid color and copy the current GPU
  depth/stencil plane. Newer writes in the same command list take precedence.
- Recheck the color identity during replay. Any partial CPU write removes the
  affected provider; incompatible or lost proofs retain normal upload/fallback
  behavior. Uniform copied depth clears stale destination-detail metadata.
  Short-circuit native/non-opaque targets and avoid looking up a superseded
  memory provider when the current list already supplies authoritative depth.
- New hardware coverage switches A/B color targets across lists at 1× and 6×,
  requires the preceding draw's stencil generation, checks no warm-path ticket
  materializations, tests partial CPU color/depth writes and a full uniform-depth
  clear. Native results match synchronous rendering; full-resolution color
  matches an independent two-color/shared-depth GPU reference. Synchronous native
  re-expansion is deliberately not the 6× reference: it can lose subpixel color.
- All 93 enabled runtime tests, 12 hardware GPU tests and strict Clippy pass.
  Intro and overworld captures remain identical. Battle retains every native
  capture; two scaled captures change with retained full-resolution detail.
  The longer 62-second gameplay/bag recording retains all 12 native captures,
  unchanged instruction/draw counts, and zero fallbacks across 50965 lists;
  four scaled top-screen captures change, while all bottom-screen captures match.
  Materialized tickets fall from 6993 to 4644 (33.6%) in that longer replay.
  Concurrent correctness timings are not performance evidence.
- Isolated 6× ABBA comparisons, with encoders absent before/after each run and
  no periodic frame dumping: refined battle baseline 30.680/30.084 seconds versus
  candidate 28.591/29.823 seconds for the same 973 updates and 2004562830 guest
  instructions: about 4.0% more replay throughput. Battle materializations fall
  3050 → 2423 (20.6%). An earlier prototype independently measured about 4.7%.
- Overworld timings are inconclusive: the prototype's small slowdown was not
  reproduced after cheaper guards (refined baseline 18.719/18.103 seconds;
  candidate 18.147/18.212). Do not claim a stable overworld or intro speedup.
  GPU shaders, 6× scaling, deferred/frozen textures and normal guest time settings
  were identical in both variants. These headless throughput figures do not
  prove uncapped, normal-speed rendering or satisfy the 200-FPS objective.

### R-105 2026-10-29 — Fresh CPU profiles; reject broad word-access inlining

- Re-read R-084/R-085/R-098 and profiled the R-104 renderer with isolated 6×
  intro/overworld runs and no encoders active. Intro user-cycle samples: checked
  word reads 22.87%, writes 10.76%, dispatch resolution 8.85%, guest call wrapper
  4.02%. Overworld: reads 7.99%, writes 3.98%, draw encoding 4.14%, vertex loading
  3.75%. Individual MovieLib routines have about 2.50/1.24/1.02% exclusive samples;
  their callees are accounted separately, so these are not inclusive costs.
- A separate six-game-second movie call-count diagnostic finds 6,363,255 calls
  from 0x001076c8 to 0x00108bf8 and 6,309,128 from 0x00544db8 to 0x004bed34,
  plus millions of MovieLib helper calls. Compilation overlaps that diagnostic;
  only deterministic counts, not its wall time, are evidence. These support
  testing the shared checked-word path before porting a whole decoder routine.
- First experiment force-inlined word access and outlined rare slow paths.
  Retained the original checked implementation behind USUM_GUEST_INLINE_WORDS=0,
  with the pre-change executable also frozen to avoid bias from switch overhead.
  All 42 image captures, three audio WAVs, output saves, guest instruction counts,
  VBlanks, draw/triangle counts, video conversions and audio-frame counts match
  across legacy/reference/fast modes. Both modes pass unit/GPU/QEMU checks.
- Controlled legacy/fast/reference/reference/fast/legacy runs at 6×, without
  periodic captures or call profiling and with encoder/compiler overlap rejected:
  intro legacy 9.310/9.069 seconds, fast 11.040/11.074; overworld legacy
  17.588/18.885, fast 19.879/20.429; battle legacy 29.817/29.865, fast
  32.948/32.535. Replay throughput drops about 16.9%, 9.5%, and 8.9% respectively.
  The executable .text grows 261099133 → 1006150125 bytes (3.85×).
- Reject the broad prototype. Code growth and measured regressions motivate a
  smaller indexed-RAM fast path; instruction-cache causality is not established
  by these timings alone. The active revision excludes the entire deferred-range
  hull conservatively, uses the original path for partial pages / boundaries /
  permissions / device reads, and keeps logical-cache invalidation out of ordinary
  stores. R-106 records the completed rebuild and controlled comparisons.
- Extend the differential harness to run each program with both one-region and
  indexed layouts, so the new RAM path is exercised rather than only its fallback.
  The 3DS runtime, translated game logic, instruction budgets and virtual clock
  remain the compatibility/timing foundation. No FPS improvement or 200-FPS
  completion is claimed from the rejected experiment or unmeasured revision.

### R-106 2026-10-29 — Small indexed-RAM word path improves movie replay throughput

- Select one low-level native shortcut: checked 32-bit reads/writes wholly within
  a fully indexed RAM region. Check read/write permission and all four bytes;
  reject the entire deferred-provider hull conservatively. Partial pages,
  boundaries, wraparound and device data use the original implementation.
  Keep cached logical-region destruction in a cold helper. Ordinary compiler
  inlining replaces the force-inline policy rejected in R-105. Translation,
  service behavior, instruction budgets and virtual-clock accounting are unchanged.
- USUM_GUEST_INLINE_WORDS=0 retains the original word implementation in the same
  binary. The frozen pre-change executable is the performance baseline: same-binary
  reference mode also pays selection/layout overhead and is not an unbiased proxy
  for the old binary. Hash source saves, input scripts and binaries per run.
- At 6×, controlled legacy/fast/reference/reference/fast/legacy trials without
  frame dumping or call profiling: intro old 9.289/9.239 seconds, new 8.376/8.353
  (10.75% more throughput); overworld old 18.573/18.366, new 18.088/17.753 (3.06%);
  battle old 29.432/29.379, new 29.115/28.955 (1.28%). Both sides execute identical
  guest instruction, VBlank, draw, triangle, video-conversion and screen-update
  counts. Check for encoders/builds before and during each run, discarding overlap.
- The three-mode correctness matrix matches all 42 native/scaled image captures,
  three exported PCM WAVs, output save hashes, and audio-frame counts. Counts are
  1583340348/1170300377/2004562830 guest instructions for intro/world/battle,
  with 344/28/142 video conversions and 1226/3272/6136 audio frames. All renderer
  lists retain zero software fallbacks. A separate 62-second gameplay/bag replay
  matches 26 further captures, its WAV and saves, with 5117853423 instructions,
  3708 VBlanks and 165848 draws. The 78-second Mystery Gift recording matches
  six further captures, WAV/saves, 15903540758 instructions and 4664 VBlanks,
  including the same self-restart at VBlank 3728. These longer functional replay
  timings are not additional controlled performance claims.
- Both modes pass the 34-program, 292714-instruction QEMU corpus in both simple
  and indexed layouts, as well as 29 guest tests, 93 runtime tests, 12 hardware
  GPU tests and strict Clippy. New memory checks exercise real indexed fast-path
  hits, read-only/write-only mappings, overlap fallback, region boundaries and
  cached logical-view invalidation. Existing randomized deferred-memory checks
  also remain byte-exact with the retained reference configuration.
- A fresh post-change intro profile puts dispatch around 8.9% of user-cycle
  samples and MovieLib routines around 5.15/3.01/2.72%. Those routines now include
  inlined word-access work: changing attribution is not proof that the former
  33.6% memory cost disappeared. Keep the replay timings as the performance result.
- Cost: .text is 524895629 bytes versus 261099133 before (2.01×), despite being
  much smaller than the rejected 1.006-GB prototype. Small gameplay improvements
  rest on two interleaved runs per variant and are less certain than the intro
  gain. These are headless update-throughput results, not 200 normal-speed FPS;
  the full-game objective remains unmet. Game data and evidence stay private.

### R-107 2026-10-29 — Reject dispatch front cache; refresh gameplay profile

- The post-R-106 intro profile attributes about 8.9% of user-cycle samples to
  dispatch. Annotated samples point to hash-table probing as well as the required
  module identity checks. Test a 1024-slot address-indexed front cache backed by
  the existing hash lookup and stable code indices. Every hit still checks the
  complete address and module identity; collisions only miss. Preserve the
  hash-only mode for comparison. Collision/growth and module lifecycle tests,
  94 runtime tests, 12 hardware GPU tests and strict Clippy pass on the prototype.
- Isolated 6× legacy/fast/reference/reference/fast/legacy replays use identical
  saves and input scripts, no capture/profile overhead, and reject encoder/build
  overlap. Intro old 8.351/8.245 seconds, new 8.237/8.302; overworld old
  18.165/18.233, new 18.043/17.446; battle old 28.299/28.414, new 29.023/28.350.
  Throughput changes are +0.34%, +2.56%, and -1.15%, respectively. The modified
  hash-only reference takes 8.580/8.554, 18.138/17.568, and 28.670/28.579 seconds;
  use the frozen shipped executable, not this changed storage layout, as baseline.
  Instruction, VBlank, draw, triangle, video-job, update and output-save counts
  match. No image/audio equivalence claim is made for this rejected experiment.
- Reject the prototype: it does not establish a useful intro improvement and
  gives mixed gameplay results. Preserve its patch and evidence privately;
  restore the original dispatch implementation without changing guest behavior.
- A fresh isolated post-R-106 overworld profile puts draw encoding at 4.35%,
  vertex loading 3.93%, GPU snapshot materialization 2.76%, depth decoding 2.45%,
  depth encoding 2.42%, deferred target encoding 2.24%, and dispatch 1.53% of
  samples. Inlined guest memory work remains attributed to its callers. These
  samples favor investigating framebuffer plane conversion over another dispatch
  micro-optimization for gameplay. No normal-speed 200-FPS result is established.

### R-108 2026-10-29 — Encode frozen framebuffer planes only when read

- The R-107 profile and provider code show that a color-only CPU reader also
  encodes the frozen depth/stencil plane into guest tiled layout. Give color and
  depth/stencil independent immutable encoding caches. A depth-only reader also
  leaves color unencoded. Frozen texture diagnostics use the same per-plane path.
  Retain the original eager encoder with USUM_VULKAN_SPLIT_ENCODE=0, read once
  when enabling deferred readback and frozen into each provider's policy.
- Both planes still use the same complete GPU staging ticket, including its
  original polling/mapping error behavior. This change avoids unused CPU layout
  conversion and allocation; it does not omit GPU snapshot planes or change
  rendering, guest instructions, budgets, virtual time, services, or decoder work.
  Count a materialized target once even if both planes are requested concurrently.
- Controlled 6× trials, with encoder/build overlap rejected, use identical hashed
  starting saves and input scripts. Order is old/new/reference/reference/new/old/
  new/old/old/new per scene, without periodic captures or guest-call profiling.
  Intro old 8.509/8.220/8.380/8.220 seconds, new 8.189/8.108/8.117/8.222;
  overworld old 17.998/17.592/17.820/18.187, new 17.470/17.037/17.802/17.033;
  battle old 29.247/28.336/29.487/28.409, new 28.132/27.633/28.550/28.160.
  Average replay throughput improves 2.12%, 3.25%, and 2.67%, respectively.
  Eager same-binary reference pairs are 8.308/8.181, 17.600/17.911, and
  29.489/28.439 seconds. Frozen pre-change binaries remain the primary baseline.
- Baseline/eager/lazy comparisons match all 42 image captures, three PCM WAVs,
  saves, instruction/VBlank/draw/triangle/video-job/audio-frame/update counts,
  and queued/materialized ticket counts. There are zero renderer fallbacks.
  Baseline/lazy long gameplay/bag and Mystery Gift recordings match 32 additional
  captures, two WAVs, saves and guest counters; both restart at VBlank 3728.
  These functional-run timings are not additional controlled benchmarks.
- All 68 CPU runtime tests, 93 enabled Vulkan/window runtime tests, 12 hardware
  GPU tests and strict Clippy pass. Extend the frozen-plane test to check eager,
  color-first lazy, and depth-first lazy reads, exact stencil bytes after newer
  GPU writes, cache independence, one-target accounting and CPU-write invalidation.
- Fresh post-change overworld samples put depth encoding below the 1% report
  threshold (previously 2.42%); depth decoding remains about 2.60%, vertex loading
  4.38%, draw encoding 3.84%, and ticket materialization 2.37%. Sampling and changed
  attribution are diagnostic, not substitutes for replay timing. The replay
  still shades all 45832 overworld and 62746 battle draws on the GPU; CPU vertex
  and software-renderer fallback paths remain available.
- No average regression in these scenes, but four runs per variant and these
  recordings cannot establish whole-game behavior or performance. The gains are
  modest headless throughput improvements, not 200 actual normal-speed FPS.
  Full GPU snapshots, remaining framebuffer conversion and guest execution are
  still costs. Evidence and game data remain private.

### R-109 2026-10-29 — Reject direct frozen-plane imports after replay coverage check

- Prototype importing linear color and quantized D24S8 directly from immutable
  readback tickets instead of encoding to guest tiled layout and decoding again.
  Exact plane identity, dimensions, format and read permission gate the path;
  CPU writes and remaps remove it. Preserve D24 rounding and original fault paths,
  and retain the tiled import behind a reference switch during the experiment.
- All 42 replay captures, three WAVs, saves and guest counters match across
  baseline/reference/prototype. The prototype passes 68 CPU runtime tests,
  93 enabled Vulkan/window tests, 13 hardware GPU tests and strict Clippy.
  A dedicated GPU check covers quantization, immutable snapshots, permissions,
  wrong layouts/planes, partial writes, remaps and one-target accounting.
- Coverage counters find 0/338/901 direct color imports in intro/world/battle,
  and zero direct depth imports in all three. The profiled depth decode cost
  therefore remains outside this shortcut. Stop before controlled benchmarks;
  capture-run elapsed times are not performance evidence. Reject the prototype
  as insufficiently targeted, retain its patch/evidence privately, and restore
  the shipped path. Investigate the raw depth buffers that actually reach the
  decoder instead. This experiment establishes no FPS improvement.

### R-110 2026-10-29 — Specialize exactly uniform D24S8 imports

- Post-R-108 gameplay sampling places tiled depth decoding at about 2.6%.
  The raw buffers reaching it include whole-plane clears, unlike the opaque
  providers tested in R-109. Recognize complete 8×8-tiled dimensions containing
  one repeated D24S8 word: compare every depth and stencil byte in 16-byte blocks,
  decode its value once, and fill the linear buffers. Nonuniform or incomplete
  layouts keep the original decoder. The original checked memory slice still
  controls permissions, bounds and deferred-device failures.
- USUM_VULKAN_UNIFORM_DEPTH=0 retains per-pixel tiled decoding. Capture this
  policy when constructing PICA and retain it across process restarts. Diagnostic
  counters verify 684/991/2348 uniform imports in intro/world/battle, and zero
  when disabled. No shader, draw, guest instruction, budget, clock or service
  behavior changes; all existing renderer and CPU vertex fallbacks remain.
- Controlled 6× replay ordering is old/new/reference/reference/new/old/new/old/
  old/new, with identical hashed saves and inputs, no capture/call-profile overhead,
  and encoder/build overlap rejected. Intro old 8.135/8.193/8.036/8.138 seconds,
  new 7.920/8.029/7.932/7.868; overworld old 17.800/17.488/17.492/17.662,
  new 17.905/17.753/17.641/16.972; battle old 27.963/28.562/28.053/28.561,
  new 27.671/26.577/27.168/27.863. Average throughput improves 2.37% intro and
  3.53% battle. Overworld's +0.24% is inconclusive: three optimized samples are
  slower than the paired baseline. Do not claim an overworld improvement or
  exclude a small regression. Same-binary reference pairs are 8.032/8.088,
  17.673/17.201, and 28.180/27.778; the shipped fce3036 binary is the baseline.
- Baseline/reference/optimized runs match all 42 native/scaled captures, three
  WAVs, saves and guest instruction/VBlank/draw/triangle/video/audio/update counts.
  The longer bag/gameplay and Mystery Gift recordings match 32 additional images,
  two WAVs, saves and guest counters, including restart at VBlank 3728. Their
  baseline captures are reused only after verifying the identical binary SHA,
  starting-save hashes, input hash and audio hash. Functional elapsed times are
  not controlled performance evidence. All scenes retain zero software fallbacks.
- All 68 CPU runtime tests, 94 enabled Vulkan/window runtime tests, 12 hardware
  GPU tests and strict Clippy pass. Compare uniform decoding bit-for-bit with the
  retained tiled decoder across sizes, depth values and stencil bytes. Changes
  at the first/middle/last byte, truncated inputs and partial tiles must fall back.
- An isolated component benchmark with extracted production functions checks the
  nonuniform scan cost: four interleaved trials of 512 decodes of a 256×512 plane.
  Uniform average elapsed time is 0.11123 → 0.01116 seconds; an early mismatch
  remains about 0.1106 seconds; a last-byte mismatch takes 0.11051 → 0.11750
  seconds (about 6.3% more decoder time). These are component measurements, not
  a tenfold game speedup. Late nonuniform mismatches are a remaining cost.
- A fresh gameplay profile puts tiled depth decoding below the 1% reporting
  threshold, vertex loading at 5.04%, draw encoding 3.89%, GPU ticket materialization
  1.89%, and deferred encoding 1.83%. Changed attribution and sampling variance
  cannot establish FPS. Four runs and these recordings do not cover the whole
  game. These modest headless gains do not change the game's render cadence or
  establish 200 actual rendered FPS at normal speed; that objective remains open.

### R-111 2026-10-29 — Prove safe vertex instruction budgets once per program

- The post-R-110 overworld profile attributes 0.82% of samples to Cost::bound.
  GPU draw preparation repeatedly walks an immutable shader cost tree. Prove
  the budget at compilation when maximum loop counts fit the existing inclusive
  65536-instruction limit. All cost operations are nonnegative saturating sums
  and products; each loop reads only an integer uniform's low byte, whose maximum
  is 255. A bound at [255; 4] therefore covers every possible draw uniform.
- Eligible programs keep a successful proof in their cached VertexProgram and
  avoid per-draw tree traversal. Programs whose maximum is larger still evaluate
  the original tree with the actual uniforms and retain dynamic rejection.
  USUM_VULKAN_STATIC_BUDGET=0 retains per-draw traversal for every program. Capture
  this process-wide policy on the first successful translation to keep the
  program cache consistent. Shader source/emission, limits, rendering, guest
  instructions, budgets, virtual time and CPU fallback behavior are unchanged.
- Differential checks compare original and proved validation across 256 generated
  nested cost trees and 32 uniform arrays per tree, including saturation, empty
  sums, all four slots, ignored high bits and maximum counts. Explicit cases
  retain acceptance at exactly 65536 and dynamic rejection immediately above it.
  Both policies pass 18 render unit tests, six renderer hardware tests and twelve
  runtime hardware tests. All 94 enabled runtime tests, 68 CPU runtime tests and
  strict render/runtime Clippy also pass.
- Controlled 6× old/new/reference/reference/new/old/new/old/old/new trials use
  identical hashed saves and replay inputs, no periodic captures/call profiles,
  and reject encoder/build overlap. Intro old 8.130/8.054/8.114/7.917 seconds,
  new 7.852/7.929/8.060/7.958; overworld old 17.247/17.770/17.053/17.563,
  new 17.223/16.494/16.419/17.225; battle old 28.245/27.846/28.208/27.524,
  new 27.411/27.403/28.100/28.144. Average throughput differences are +1.31%,
  +3.37%, and +0.69%. The overworld result is the strongest observed improvement;
  intro/battle differences are small compared with the variation, including a
  slower fourth battle trial. Do not claim stable gains for those scenes.
- Same-binary reference pairs are 7.895/7.938, 17.796/17.808, and 27.691/28.368
  seconds. The frozen b10c254 executable remains the primary baseline: the new
  reference still has the added proof field/branch and a changed binary layout.
  Do not attribute every elapsed-time difference solely to tree traversal.
- Baseline/reference/optimized output matches all 42 native/scaled captures,
  three PCM WAVs, saves and guest instruction/VBlank/draw/triangle/video/audio/
  update counters. Long bag/gameplay and Mystery Gift recordings match 32 more
  captures, two WAVs, saves and guest counters, with restart at VBlank 3728.
  Long baseline captures are reused only after checking the identical binary
  SHA, starting-save/input hashes and WAV hash. Those functional-run timings
  are not additional controlled benchmark evidence. All scenes have zero software
  fallbacks and unchanged GPU/CPU vertex draw counts.
- A fresh overworld profile puts Cost::bound below the 0.1% reporting threshold,
  vertex loading at 5.07%, draw encoding 3.81%, GPU ticket materialization 2.12%,
  and deferred encoding 1.56%. Sampling does not prove zero traversal in all
  programs; the dynamic fallback is intentional. Vertex input loading remains
  a larger target. Four timing trials and these recordings do not establish
  whole-game performance/correctness or 200 actual rendered FPS at normal speed.

### R-112 2026-10-29 — Plan GPU vertex input assembly once per draw

- The post-R-111 overworld profile places load_vertex at 5.07% of samples. Each
  vertex builds a twelve-attribute optional array, then applies fixed values and
  register permutation. For hardware draws with at least sixteen distinct inputs,
  precompute the final destination of each field and a fixed-register template
  once per draw. Load directly into that template, preserving later-loader and
  later-attribute alias precedence. CPU shading and smaller draws retain the
  original loader; USUM_VULKAN_INPUT_PLAN=0 also selects it for hardware draws.
- Retain every loader slice and every field read in original order, including
  overwritten/unused attributes: these reads can fail. Keep checked-memory slice
  and per-element fallback, wrapping address arithmetic, partial components and
  ordered first-error collection. No shader arithmetic, GPU topology, guest
  instruction/budget/clock behavior, or CPU fallback is changed. The plan is
  local to a draw; it caches no guest memory or cross-process state. Its policy
  and cumulative diagnostic counters survive a title restart.
- Counters verify 37544 planned overworld draws / 12407034 inputs, out of
  45832 / 12454554 (99.62% of inputs). Battle uses 42851 / 14263275, out of
  62746 / 14387636 (99.14%). The intro's 688 tiny draws / 2760 inputs retain
  the original loader. Reference mode reports zero planned draws/inputs.
- Both modes pass 99 enabled runtime tests and 12 hardware GPU tests; all 68
  CPU runtime tests and strict Clippy pass. Differential checks cover 256 varied
  layouts and six vertex indices each, all element formats, register aliases,
  fixed/default attributes, NaN/signed-zero bits, duplicate loaders, read-only and
  unreadable mappings, split regions and address wrap. Explicit checks retain
  faults for discarded values and identical deferred-provider call/error order.
  Captures for 3/33/257 vertices compare the small, serial and parallel paths.
- At 6×, controlled old/new/reference/reference/new/old/new/old/old/new trials
  use identical hashed saves/inputs, no periodic dumps/call profiling, and reject
  encoder/build overlap. Intro old 7.906/7.900/7.893/8.162 seconds, new
  8.098/7.985/7.867/8.056; overworld old 16.976/16.558/17.763/17.800,
  new 16.636/17.648/16.492/16.920; battle old 27.529/28.118/27.587/28.141,
  new 28.044/26.993/28.172/27.787. Average throughput changes are -0.45%,
  +2.07%, and +0.34%. Intro/battle are inconclusive, and overworld samples also
  vary substantially. Same-binary reference pairs are 8.059/7.926,
  17.446/17.326, and 27.526/27.907; frozen b4d9439 is the primary baseline.
- An isolated same-binary reference/new/new/reference overworld counter check
  corroborates reduced host work. User instructions are 135087949656/135087035841
  reference versus 133352431089/133382644906 planned (1.27% fewer); user cycles
  are 63467652927/63201293324 versus 61960592509/61952546170 (2.18% fewer).
  Both events run 100% scheduled, and all runs retain 1170300377 guest
  instructions, 956 VBlanks, 45832 draws and 498 updates. These counters include
  process setup and driver threads, use the same hashed starting save/input,
  reject encoder/build overlap, and do not measure normal-speed FPS.
- All 42 baseline/reference/planned native/scaled captures, three WAVs, saves
  and guest instruction/VBlank/draw/triangle/video/audio/update counts match.
  Long bag/gameplay and Mystery Gift recordings match 32 additional captures,
  two WAVs, saves and counters, including restart at VBlank 3728. Reused long
  baseline captures verify the identical binary SHA, starting-save/input hashes
  and WAV hash. Their elapsed times are not additional performance trials.
  All scenes keep zero software fallbacks and unchanged GPU/CPU vertex counts.
- Fresh gameplay samples attribute 1.76% to InputPlan::load and 1.41% to
  element_in, with draw encoding at 4.81% and GPU ticket materialization 2.07%.
  Inlining/attribution differs from the old loader, so do not subtract those
  percentages as a measured speedup. Scalar element decoding, draw submission
  and guest execution remain targets. Sixteen vertices is an amortization policy,
  not a proven optimum for every workload. Four timing trials and these recordings
  leave whole-game performance/correctness uncertain. The 200 actual rendered
  FPS objective at normal speed remains unmet.

### R-113 2026-10-29 — Pack GPU vertex uploads in complete native blocks

- Revisit the R-084/R-085/R-098 CPU investigation after the later optimizations.
  The R-112 overworld profile puts draw encoding at 4.81%; annotated assembly
  locates the dominant samples within that function in scalar float appends,
  including repeated Vec capacity/length work. Each vertex contains 64 floats.
  Replace those 64 four-byte appends with one 256-byte append, using safe Rust
  array mapping and native-endian float bytes. Keep the original scalar loop
  behind USUM_VULKAN_BLOCK_PACK=0, sampled once at first hardware vertex upload.
- Preserve every float bit, register order, validation, storage-limit check,
  buffer/bind-group lifetime, shader operation and draw order. CPU geometry and
  guest execution remain unchanged. The 3DS runtime, instruction budgets and
  virtual clock remain the compatibility/timing foundation. This is a small
  host-side helper optimization, not a native movie-decoder replacement.
- Renderer tests pass in both modes (20 enabled), including varied raw float
  bits, signed zeros, NaN payloads, register ordering and varied vertex counts.
  All 99 enabled runtime and 68 CPU tests pass, as do strict renderer/runtime
  Clippy checks. Six renderer and twelve runtime hardware tests pass with both
  settings; the game replays exercise the actual hardware upload path.
- An isolated component check packs 262144 vertices per trial, four trials per
  mode with interleaved order and batch sizes 4/16/256/4096. The block packer is
  approximately 7.2–9.0 times faster at packing alone, with identical bytes.
  This microbenchmark is not a whole-game or normal-speed FPS result.
- Controlled 6× replay trials use frozen def1ef3 as the primary baseline and
  legacy/fast/reference/reference/fast/legacy/fast/legacy/legacy/fast ordering.
  Saves, inputs and executable hashes are recorded. Periodic dumps and call
  profiling are disabled; encoder/build overlap is rejected without interrupting
  other processes. Intro old 7.901/8.055/7.928/7.994 seconds, new
  7.981/8.005/7.943/7.907; overworld old 16.728/17.712/17.331/17.353,
  new 16.388/16.303/17.015/16.539; battle old 27.438/27.733/27.372/27.363,
  new 27.553/26.991/27.244/26.929. Average replay throughput changes are
  +0.13%, +4.35%, +1.09%. Intro is unchanged within noise; battle is inconclusive,
  including a slower first pair. Overworld is the strongest measured gain.
- Same-binary scalar pairs are 8.017/7.987, 17.812/17.339 and 26.972/27.691
  seconds. They retain a policy branch and changed function/binary layout;
  the frozen executable is the primary baseline rather than assuming reference
  mode reproduces its performance exactly.
- Separate isolated overworld user-space counters, ordered legacy/fast/scalar/
  scalar/fast/legacy, corroborate reduced host work. Legacy instructions are
  133351424865/133415483555 versus 125485000598/125552020631 block-packed
  (5.90% fewer). Legacy cycles are 61740133188/62534107637 versus
  60455099569/60622795914 (2.57% fewer). Same-binary scalar instructions are
  133485613102/133404497040 and cycles 62516475400/62237551863, giving 5.94%
  and 2.95% reductions respectively. Both events are 100% scheduled in every
  run. Counts include setup and driver threads, not just packing, and do not
  measure normal-speed FPS. All runs retain identical guest work and timing.
- Baseline/scalar/block modes match 42 native/scaled captures, three PCM WAVs,
  output saves and guest instruction/VBlank/draw/triangle/video/audio/update
  counts. Long bag/gameplay and Mystery Gift recordings match another 32 images,
  two WAVs, saves and counters, including restart at VBlank 3728. Reused long
  baseline captures verify identical executable, starting-save/input and audio
  hashes. Functional capture timings are not performance trials. All five scenes
  retain zero software fallbacks and identical GPU/CPU vertex counts.
- Fresh candidate overworld samples put packing at 1.04%, InputPlan::load at
  2.14%, element_in at 2.00%, and GPU ticket materialization at 2.29%. Changed
  attribution/inlining and the small sample count preclude subtracting these
  percentages to infer speedup. A fresh isolated six-second intro profile still
  puts dispatch resolution at 8.94%, the guest call wrapper at 4.93%, MovieLib
  functions 0x990021c4/0x99002ea8/0x99002548 at 4.89/4.32/3.34%, and executable
  helper 0x004bed34 at 3.83% exclusive samples. Several separate write_u32
  instantiations each contribute 1.88–3.93%; some word access is inlined into
  translated code, so those symbols do not measure total memory-access cost.
  Movie conversion itself is 1.42%. Profiles stay private with the replay data.
- No behavior regression was observed in these recordings; broader correctness
  and performance remain uncertain. Movie helpers/dispatch and checked memory
  still warrant targeted investigation. This change does not improve the intro
  measurably or establish 200 actual rendered FPS at normal game speed.

### R-114 2026-10-29 — Reject generic vertex-field block decoding

- Follow the R-113 overworld profile: element_in has 2.00% exclusive samples,
  alongside 2.14% in InputPlan::load. Annotated element_in assembly shows repeated
  kind dispatch and bounds checks, and four byte loads/shifts/ORs for each float.
  Test decoding an entire attribute field from an already checked contiguous
  loader slice, selecting the format once for all components. Keep the original
  scalar decoder as a same-binary comparison and leave CPU shading unchanged.
  Failed slices retain the original ordered per-element reads, including discarded
  fields, deferred-provider calls, partial faults and wrapping addresses.
- Both modes pass 94 enabled Vulkan runtime tests and twelve hardware tests;
  the window/Vulkan configuration passes 100 enabled tests. All 68 CPU tests and
  strict all-target runtime Clippy pass. Differential checks compare all four
  formats, component counts 1–4, unaligned byte offsets, varied raw bits, signed
  zero and NaN payloads. Existing randomized layouts and fault/provider-order
  checks explicitly exercise both field modes against the scalar CPU oracle.
- Frozen 9206db7, same-binary scalar and candidate replays match all 42 native/
  scaled captures, three PCM WAVs, output saves and guest instruction/VBlank/
  draw/triangle/video/audio/update counts. There are zero software fallbacks and
  unchanged GPU/CPU vertex counts. Reused baseline captures verify executable,
  starting-save, input and WAV hashes. No new long/restart candidate runs are
  claimed: the short performance results do not justify shipping this prototype.
- Isolated 6× legacy/fast/scalar/scalar/fast/legacy/fast/legacy/legacy/fast trials
  use the same recorded inputs and starting saves, with dumps/profiling disabled
  and encoder/build overlap rejected without interrupting other processes.
  Intro baseline 7.999/7.951/8.024/7.964 seconds, candidate
  7.959/7.933/7.907/8.028; overworld baseline 16.443/16.658/16.429/16.881,
  candidate 16.937/17.076/16.432/17.189; battle baseline
  26.993/26.449/26.951/27.210, candidate 26.497/26.730/26.373/27.141.
  Average throughput changes are +0.35%, -1.81%, +0.81%. Intro/battle are
  inconclusive, while the overworld average regresses. Same-binary scalar pairs
  are 7.924/8.021, 17.149/15.582 and 26.794/27.388; the frozen executable
  remains the primary baseline rather than assuming identical binary layout.
- Separate legacy/fast/scalar/scalar/fast/legacy native counters show a genuine
  reduction in executed host work. Baseline user instructions are
  125519266274/125484596855 versus 121623171866/121563783082 (3.11% fewer).
  Baseline user cycles are 60607132173/60316186574 versus
  59837500439/59330727942 (1.45% fewer). Same-binary scalar instructions are
  125469667699/125501608549 and cycles 60653547153/60531755249, giving 3.10%
  and 1.66% reductions. Events run 100% scheduled throughout. Counts include
  process setup and driver threads; they do not prove an FPS improvement.
- That mismatch justifies four additional balanced overworld pairs, in
  fast/legacy/legacy/fast/legacy/fast/fast/legacy order. Additional baseline
  times are 16.999/16.792/16.406/16.532 seconds, candidate
  16.404/16.816/16.677/16.909. Across all eight trials per build, candidate
  throughput is still 0.97% lower. Variation prevents a strong causal claim
  about the slowdown, but neither timing set demonstrates the intended gain.
- Fresh candidate sampling puts InputPlan::load at 1.74%, GPU ticket
  materialization at 2.59%, dispatch at 2.00% and deferred encoding at 1.83%.
  Changed inlining and limited samples prevent attributing the elapsed-time
  result to these percentages. Reduced native instruction counts alone are
  insufficient to ship a performance change with no demonstrated throughput
  benefit. Discard the implementation and comparison flag; preserve the patch,
  executables, replay metadata, counters and profiles privately. The validated
  R-113 packer remains active. Movie helper/dispatch, checked memory and GPU
  readback/submission costs remain targets; 200 actual normal-speed FPS is unmet.
- Rebuild the restored source and compare both probe and play executables
  byte-for-byte with the validated R-113 binaries: both match. No experimental
  decoder or runtime policy remains in the working source or runnable build.

### R-115 2026-10-29 — Keep scaled LCD images resident through window presentation

- After rejecting R-114, inspect the display/readback path. Every supported LCD
  transfer synchronously maps a full scaled RGB image on the game thread; the
  software window then copies/scales those CPU pixels for presentation. At 6×,
  the top image alone contains 10368000 RGB bytes. Keep the existing GPU crop,
  rotation, downscale averaging and RGB packing, but freeze their output in an
  immutable GPU buffer. CPU captures/fallbacks materialize it once on demand.
  The Vulkan window samples that buffer directly, preserving integer nearest
  sampling, screen layout, clipping, channel values and black margins.
- Keep the instance/adapter for window surfaces, require byte-preserving
  RGBA8/BGRA8 UNORM swapchains, and fall back to the software window if unsupported.
  Serialize surface configuration against every queue submission, including
  game rendering and lazy display readbacks. Frozen buffers own their generation
  independently of later target writes/destruction. Renderer ownership uses an
  explicit shared identity rather than trusting GPU handle equality across
  independently created contexts. Native/software LCDs can upload their RGB to
  the same compositor. Guest shaders, memory semantics, instruction budgets,
  virtual clock and normal pacing remain unchanged.
- USUM_VULKAN_RESIDENT_DISPLAY=0 restores eager RGB readback;
  USUM_VULKAN_GPU_PRESENT=0 restores the software window compositor. Both zero
  select the previous display flow. The CPU rendering reference/fallback and
  CPU display transform remain available. USUM_PLAY_REPLAY=1 is a diagnostic:
  it loads the recorded input, disables live input only for that run and exits
  at the configured game-time limit, retaining normal real-time pacing.
- All 99 enabled window/Vulkan runtime, 68 CPU and 20 renderer tests pass, with
  strict all-target runtime/renderer Clippy and formatting checks. Seven renderer
  hardware tests and twelve runtime hardware tests pass. Exact compositor checks
  cover RGBA/BGRA, all channel values, odd source sizes/RGB tail padding,
  overlapping screens, clipping, nearest sampling and absent screens. They verify
  composition never materializes source pixels and rejects foreign renderer
  frames/overflowing rectangles. Existing display-oracle checks now also verify
  frozen results at scales 1/2/3/6/8, all downscale axes, flips and cropped origins,
  after target reuse/destruction, with concurrent readers sharing one result.
- A synthetic real-window check presents 48 times at five actual sizes
  (800×960, 400×480, 1024×768, 129×77, 825×998), while another thread completes
  nine raster submissions. It closes automatically, uses no game data, and
  verifies no source materialization. A 6× gameplay window capture is also
  visually checked; its pixels come from the same GPU compositor.
- Paced, 800×960 window comparisons use the same executable, isolated identical
  starting saves/inputs and CPU/GPU/GPU/CPU order. Sound output is disconnected
  consistently; guest audio generation continues. Final-build intro takes
  8.569/8.560 seconds with eager CPU presentation versus 6.878/6.889 resident
  (24.42% higher replay throughput). Overworld takes 18.278/18.241 versus
  16.452/16.385 (11.21%). Battle takes 30.049/30.065 versus 30.019/30.019:
  both are limited by normal
  pacing, so claim no meaningful battle speed increase. These are game-thread
  elapsed times, including initialization work within that interval; renderer
  initialization can occur earlier on the GPU window's main thread. They do not
  isolate steady-state pixel-copy cost or represent end-to-end process timings.
- Average window redraw time falls from 1.85/1.87 to 0.25/0.25 ms in the intro,
  2.06/2.09 to 0.25/0.25 ms in the overworld, and 1.98/1.96 to 0.24/0.25 ms
  in battle. GPU runs report 341/492/967 distinct resident buffers per screen
  respectively, 26 initial/native CPU uploads and zero materialized display-source
  uses. Window redraws/buffer counts are not unique-game-frame FPS evidence.
  All runs retain identical guest instruction/VBlank/draw/triangle counts and
  output saves; those saves also match the frozen probe baseline.
- Separately, initial isolated headless 6× trials compare frozen 9206db7 with resident
  display output, in legacy/fast/reference/reference/fast/legacy/fast/legacy/
  legacy/fast order, without dumps/call profiling. Reject encoder/build/replay
  overlap without interrupting other processes. Intro baseline
  8.004/7.966/7.964/7.975 seconds, new 6.597/6.630/6.673/6.697; overworld
  16.819/16.931/17.238/15.820, new 13.484/13.490/13.412/13.586; battle
  26.763/27.764/26.970/26.841, new 21.269/21.248/21.254/21.283.
  Average throughput gains are 19.97%, 23.78% and 27.38%. Same-binary eager
  reference pairs are 8.064/7.963, 16.917/16.526 and 25.799/27.106; the frozen
  executable remains the primary baseline. Headless consumers no longer request
  unused high-resolution pixels; these throughput gains are not window FPS.
- Final review makes failed diagnostic screenshot-file writes nonfatal, matching
  the CPU window policy. The changed binary layout warrants fresh validation:
  all five final-build recordings and short eager comparisons match again.
  Additional final-build legacy/fast/fast/legacy headless trials take
  7.998/7.926 seconds baseline versus 6.658/6.648 intro; 16.234/16.189 versus
  13.433/13.467 overworld; 27.791/27.638 versus 21.225/21.252 battle. Gains
  are 19.68%, 20.53%, 30.49%; variation across the two matrices is not an
  independent improvement. A bounded final gameplay replay with a directory as
  screenshot destination verifies write failures leave GPU presentation active,
  with identical guest work and zero materialized display-source uses.
- Final-build isolated overworld native counters, in legacy/fast/reference/reference/fast/
  legacy order, corroborate lower CPU cost. Baseline user instructions are
  125469333004/125466284941 versus 123495436146/123490140067 (1.57% fewer).
  Baseline user cycles are 60122812080/60501522485 versus
  53464079650/53235227078 (11.54% fewer). Same-binary eager instructions are
  125070706126/125048763707 and cycles 59874361445/59944542464, giving 1.25%
  and 10.95% reductions respectively. Both events run 100% scheduled throughout.
  Counts include process setup/driver threads and do not by themselves explain
  the larger elapsed-time improvement or establish normal-speed FPS.
- Baseline/eager/resident replays match 42 native/scaled captures, three PCM WAVs,
  saves and guest instruction/VBlank/draw/triangle/video/audio/update counts.
  Long bag/gameplay and Mystery Gift recordings match 32 more captures, two WAVs,
  saves and counters, including restart at VBlank 3728. Reused baseline captures
  verify identical executable, starting-save/input and audio hashes. Functional
  capture timings are not extra performance trials. All five scenes retain zero
  software fallbacks and unchanged GPU/CPU vertex counts.
- Fresh final-build overworld samples put native ticket materialization (now
  attributed through OnceLock initialization) at 2.68%, InputPlan::load at 2.90%,
  dispatch at 1.77%, element_in at 2.16% and deferred encoding at 1.52%. Fresh
  intro samples put dispatch at 9.92%, the guest call wrapper at 4.72%, MovieLib
  0x990021c4 at 6.73% and executable helper 0x004bed34 at 3.21%. Other MovieLib
  helpers and separately instantiated/inlined checked memory operations remain
  substantial. Changed attribution and small samples prevent interpreting these
  percentages as independent per-function speedups.
- No regression is observed in these recordings. Fullscreen/multi-monitor
  presentation, other adapters and broader game scenes remain unverified;
  ordinary system background load is not fully eliminated. All captures, binaries
  and profiles remain private. This removes a substantial display bottleneck and
  improves the intro, but the intro still falls short of real-time playback in
  these trials. Uncapped 200 actual rendered FPS at normal game speed is not
  achieved; the goal remains active.

### R-116 2026-10-29 — Discover byte-packed area/script callbacks

- The user's new-area stop is a fail-loud missing translation at ARM
  0x00386fd0, reached by the indirect branch at 0x0053bbb8. The preserved input
  and starting save reproduce it at VBlank 25224 after 22604896289 guest
  instructions. The game thread stops; this is not a Vulkan queue deadlock.
- The executable stores that callback pointer at 0x005e6f8f, in a packed table
  alternating string and function pointers. Existing discovery scans aligned
  words plus halfword-aligned data pointers, missing odd-byte locations.
  Extend the data-pointer scan to offsets 1/2/3 within each word. Preserve
  code-range, instruction-state/alignment and initial-decode checks. Unrelocated
  CRO data still does not seed absolute pointers; relative/ADR discovery stays
  unchanged. There is no address-specific seed or gameplay workaround.
- All eleven recompilation-library tests and strict Clippy pass. The new
  synthetic regression covers all four pointer-storage alignments, including a
  final complete word at a partial image tail, and checks that disabling data
  seeds still excludes these entries.
- Regeneration increases executable functions from 36441 to 38565, retaining
  every old function; module discovery stays at 60178 functions. The private
  workspace changes 167 generated sources and rebuilds successfully. A check
  against the compiled executable entry table finds the formerly missing
  callback and verifies that entering it with zero budget returns Budget with
  the original PC/budget, without touching unmapped memory.
- The user requested no further gameplay replay past this point. No successful
  area traversal is claimed; the demonstrated missing-entry cause is corrected
  and the rebuilt play executable includes it. Input/save artifacts remain
  private. Optimization resumes, with no FPS improvement attributed to this
  discovery fix and the normal-speed 200-FPS goal still active.

### R-117 2026-10-29 — Batch checked RAM accesses for register-list transfers

- Revisited R-084/R-085/R-098 after the resident-display improvements. The fresh
  intro profile still shows MovieLib 0x990021c4/0x99002548, guest calls and checked
  word stores among the hottest CPU work. These movie functions save/restore
  nine/five registers at entry/exit. Target that repeated memory overhead rather
  than replacing movie decoding or changing guest timing.
- Translate ARM/Thumb general-register LDM/STM lists of at least four words
  through checked block helpers. One full-page index lookup, region permission
  and whole-span bounds check can establish ordinary RAM; the span must also be
  nonwrapping and outside the deferred-device hull. Other cases retain ordered
  scalar accesses, including provider calls, first faults and earlier completed
  STM stores. LDM still reads everything before committing registers/writeback.
  Stores invalidate the region's logical view as before. No unsafe memory access,
  guest function override, instruction-budget change or clock change is added.
- `USUM_GUEST_BLOCK_WORDS=0` retains the scalar transfer path in the candidate;
  disabling inline words or the page index also prevents the block fast path.
  Lists below four words keep the original generated call sites: a component
  benchmark found two-word batching slower. Four/nine/sixteen-word component
  transfers improve about 1.47/4.78/6.40 times; these are memory-helper results,
  not gameplay FPS. The pre-change executable is separately frozen for the
  actual before/after comparison because the candidate's scalar helper loops
  have their own overhead.
- All 32 guest-memory tests, eleven recompilation tests, 99 enabled runtime
  tests, formatting and strict Clippy for the changed crates pass. Memory tests
  compare values, permissions, alignment, page/region boundaries, address wrap,
  partial fault effects, deferred-provider order/failures and logical views.
  The independent QEMU corpus passes with batching enabled and disabled:
  35 programs in two memory layouts, 319988 translated instructions per corpus,
  zero failures. The added assembly covers wide transfers in all four addressing
  modes, writeback, base-in-list stores and loading all registers including PC.
- At 6× Vulkan resolution, the same five starting-save/input fixtures produce
  74 byte-identical images and five identical WAVs against commit 51b4118.
  Final saves, guest instructions, VBlanks, draws, triangles, screen updates,
  movie jobs and audio counts agree. Short fixtures also match the candidate's
  scalar switch. Every replay reports zero software fallbacks. Longer fixtures
  include battle/menu use and the recorded restart; game data stays private.
- Controlled headless trials use identical settings and frozen binaries, with
  old/new/reference/reference/new/old/new/old/old/new ordering. Four trials per
  old/new path give these mean elapsed times; the reference column is the mean
  of two same-binary scalar trials. Builds and encoders finish before timing;
  no profiling or periodic captures run during these measurements.

  | Fixture | Previous executable | Batched | Candidate scalar | Throughput gain over previous |
  |---|---:|---:|---:|---:|
  | Intro, 6 game seconds | 6.6355 s | 6.1775 s | 6.8690 s | 7.41% |
  | Overworld, 16 game seconds | 13.8848 s | 13.5660 s | 14.0785 s | 2.35% |
  | Battle, 30 game seconds | 21.6078 s | 21.0650 s | 21.8745 s | 2.58% |

- Normal-speed 800×960 window trials retain 6× internal resolution and resident
  Vulkan presentation, in old/new/new/old order. Mean game-thread elapsed time
  changes from 6.8880 to 6.3870 seconds for the intro (+7.84% throughput),
  16.7465 to 16.5640 for overworld (+1.10%), and 30.0200 to 30.0185 for battle
  (pacing-limited, no meaningful FPS gain). Guest counters and saves agree;
  presentation has no materialized source uses or software fallbacks. Host
  audio is disconnected consistently for these window benchmarks; generated
  guest audio was compared separately. These elapsed times exclude some window
  initialization and do not isolate steady-state rendering.
- Alternating old/new/new/old overworld hardware-counter runs retire
  123536535123/123562778617 native user instructions before versus
  120056246966/120066475213 after: 2.82% less work. Mean user cycles fall 1.24%,
  a smaller/noisier effect; counters are scheduled at 100%. Separate fresh intro
  profiles still show dispatch resolution, guest calls, MovieLib and scalar word
  accesses as substantial costs; the new block lookup itself remains visible.
  Sampling percentages alone are not used to claim a speedup.
- No image/audio/save or guest-timing regression was found in these fixtures.
  Gains outside them remain uncertain; the intro is still slightly below real
  time in the window, and normal-speed 200 rendered FPS remains unachieved.
  No further replay beyond the reported new-area stop was requested. Pause
  optimization after this validated change, as the user requested.

### R-118 2026-10-29 — Core presentation policy, timing measurements and testing HUD

- The user requested built-in frame-rate groundwork and an opt-in top-left FPS
  counter, then resumed the native dynamic-FPS/200-FPS objective. Add a common
  runtime presentation policy and timing module; no mod files or guest patches
  are needed for these controls. This milestone does not uncap game rendering.
- F6 cycles auto/30/60/120/144/240 maximum window presents per second;
  `USUM_PRESENT_FPS=auto` or 1..1000 selects the initial policy. Auto follows
  notifications rather than spinning duplicate redraws. Pending frames coalesce,
  expose/resize events obey the cap, and input polling keeps its separate cadence.
  Guest instructions, simulation-clock charging, hardware timers and audio rates
  are unchanged. Drawing time is included in the cap period, with no queued
  catch-up burst after a slow redraw.
- Publish frame versions and clock/submission counters under the same mailbox
  mutex. A failed capture retains the older image's version. Track produced top
  updates, shown versions, repeated presents and skipped versions separately;
  failed presents do not count. A GPU present followed by a diagnostic-capture
  failure switches backend on the next redraw rather than presenting twice.
  Restart keeps the existing mailbox and its cumulative counters.
- Use a monotonic frontend origin established before the game thread starts.
  One-second samples report observed rates, guest-clock/wall-time speed, host
  redraw duration and mean/p95 completed present intervals over the latest 240
  samples. These are observations, not projected performance from execution cost.
  F5 toggles optional title statistics; `USUM_FRAME_STATS=1` enables them at
  launch, and `USUM_FRAME_LOG=1` enables periodic logs. Title statistics are
  hidden by default; a session summary is always logged.
- F7 toggles the testing-only top-left counter; `USUM_TEST_FPS=1` enables it at
  launch. It is off by default. `FPS` currently measures actual top-screen GSP
  framebuffer submissions per wall second; `WIN` measures successful host presents.
  GSP submissions are not proof of unique rendered images or a native engine
  frame-completion boundary. Do not use this proxy or window redraws to claim
  the native 200-FPS objective. Native frame-completion instrumentation remains
  necessary before validating engine uncapping.
- The small bitmap panel uses the existing GPU compositor or the CPU window
  buffer, independently of internal resolution. Cache its bitmap/upload until
  sample text changes. It never writes guest images or saves, and the disabled
  path does no HUD drawing/upload work. While enabled it also refreshes without
  new guest frames, so a stopped producer reports zero rather than retaining an
  old FPS value; HUD-only redraws count as repeated presents, never game updates.
- All 104 enabled window/Vulkan runtime tests and 72 non-window tests pass,
  as do strict Clippy, formatting and a CPU-window-only feature build. New tests
  cover coalescing, cap changes, long stalls, frontend epoch independence, draw
  time accounting, mailbox overwrites/failed captures, measured rates/percentiles,
  bounded sample history and clipped HUD composition outside the panel.
- Against the frozen 3dd3da6 executable, final fixed-input/save captures match
  all 74 images, five WAVs, final saves and guest instruction/VBlank/draw/triangle/
  update/video/audio counts. The 6× window matrix covers auto/15/30/120 caps in
  intro, overworld and battle, CPU presentation at auto/15, and the 78-second
  restart at cap 15. Saves and guest counters match; candidate guest-clock
  stamps and submission counts agree across caps. Cap bounds hold. GPU runs
  have zero software fallbacks and zero materialized game-display source uses.
- Example restart validation produces the original 4055 top-screen submissions
  and 4664 VBlanks while submitting only 1161 window presents at cap 15. This is
  presentation coalescing, not reduced game execution. Default auto comparisons
  use old/new/new/old ordering, fixed inputs/saves and no encoder/build overlap;
  this is a control/measurement feature, with no optimization speedup claimed.
- After adding the HUD refresh behavior, repeat the five capture fixtures and
  test Vulkan/CPU HUD-on, Vulkan cap-15 HUD-on and HUD-off window runs. Pixel
  checks verify the panel's location/colors and its absence when disabled;
  visual inspection confirms readable text. Game counters and saves still match.
  Invalid cap settings fail before game data loads. The final source hashes are
  recorded with the private validation artifacts.
- Separate compatibility finding from the user's preceding live run: the stop
  at CreateThread(priority 0x30, processor -2) reaches a lifetime thread-table
  limit of 32, despite exited entries; the argument values themselves are in
  the supported range. Preserve that recording for follow-up resource-lifetime
  work. No successful replay beyond that stop or fix is claimed here.

### R-119 — Recycle exited-thread TLS without reusing thread identity

- Reproduce the user's recorded CreateThread stop at VBlank 39517 with the
  original allocation policy: 32 historical threads comprise only ten live
  threads and 22 exited threads. Priority 0x30 and processor -2 are supported;
  the failure comes from treating historical entries as simultaneous TLS owners.
- Preserve initial TLS placement and retain exited thread records, IDs and
  handles. Once pristine slots are exhausted, select storage not owned by a
  live thread and clear its 512 bytes before publishing the new thread. Reserve
  the main thread's slot even before its first saved CPU context. Allocation
  faults and genuine simultaneous exhaustion leave thread identity unchanged.
  `USUM_THREAD_LIFETIME_REFERENCE=1` retains the old policy for comparison.
- This follows the TLS lifetime model documented by
  [3dbrew](https://3dbrew.org/wiki/Thread_Local_Storage) and corroborated by
  [Azahar's thread lifecycle](https://github.com/azahar-emu/azahar/blob/master/src/core/hle/kernel/thread.cpp).
  No emulator implementation code is copied. Clearing recycled storage restores
  this runtime's fresh-block behavior; it is not a claim about hardware clearing.
- Retry only the failed SVC in the exact stopped state with recycling enabled:
  it succeeds with TLS 0x1ff83200 and a new handle. Guest clock and VBlank count
  do not advance, and no guest gameplay is executed beyond the failed call.
  This validates the resource failure, not every later area transition.
- All 107 enabled window/Vulkan tests and 75 non-window tests pass, with strict
  Clippy and formatting checks. New tests cover 128 sequential thread lifetimes,
  retained joins, simultaneous exhaustion, exact-slot reuse, read-only storage
  faults and reference-policy reproduction. Against ed5d9c1, the same five
  replay/save fixtures match all 74 images, five WAVs, saves and guest counters.
  This is a compatibility fix; no FPS improvement is claimed.

### R-120 2026-10-29 — Reject refreshed field decoding and a cadence-only FPS unlock

- Fresh post-R-118 overworld sampling still places InputPlan::load and
  element_in at 2.75% and 2.55% exclusive samples. Re-evaluate field-level format
  selection against the current pipeline, retaining scalar conversion and
  ordered fault/provider behavior. This repeats the approach rejected in R-114;
  the newer profile motivates a measurement, not an assumption of improvement.
- Frozen f1a17a2, candidate and same-binary scalar comparisons use identical
  saves, input and 6× settings. Across intro, overworld, battle, longer-session
  and restart fixtures, all 74 images, five WAVs, saves and guest counters agree
  per path, with zero software fallbacks. The prototype passes 108 enabled
  window/Vulkan tests and strict Clippy, including format/bit-pattern and memory
  fault/provider-order comparisons.
- Four balanced before/after trials per scene, with profiling/captures disabled
  and observed encoder/Rust-build overlap rejected, yield baseline/candidate
  means of 6.21225/6.2255 seconds (intro), 13.266/13.2865 (overworld) and
  20.8385/20.84275 (battle). Throughput changes are -0.21%, -0.15% and -0.02%:
  no useful gain. Same-binary scalar pairs are 6.146/6.180, 13.332/13.304 and
  20.923/20.894 seconds. Remove the implementation and comparison flag; retain
  the patch, executables, replay metadata and measurements privately.
- Separately locate an application frame-wait controller in the user's private
  executable. A deterministic known-save diagnostic changes its established
  cadence selector after VBlank 600, leaving all other state identical. By
  VBlank 900 the control remains in the overworld while the changed cadence has
  entered an encounter; audio also differs. This is not timing-preserving native
  FPS, so no cadence patch is shipped. These virtual-clock diagnostics are not
  normal-speed performance benchmarks.
- A nearby float initialized to 1.0 is not established as a timestep. A hardware
  read/write watchpoint on our own diagnostic child observes only a boot-image
  clone and two diagnostic range copies during the remaining gameplay window;
  symbol resolution confirms those callers. It provides no evidence of a game
  simulation consumer in that interval. Leave the field unchanged. No global
  debugger/profiling permissions are modified, and no game-derived code or
  captures are published. True simulation timing and engine completion still
  need tracing; 200 native normal-speed FPS remains unmet.

### R-121 2026-10-29 — Reject temporary GPU seed pooling without gameplay gains

- Fresh overworld sampling places scaled snapshot initialization at 1.77% of
  exclusive CPU samples and GPU allocation at 1.61%. Test a renderer-owned pool
  of native seed textures, views, scale parameters and bind groups. Hold a lease
  until its batch is submitted or abandoned, separating overlapping encoders;
  cap retained entries at eight and nominal pixel payload at 32 MiB.
- Real Vulkan tests cover reverse submission order, reuse before immutable
  readback materialization, abandoned batches, scale/dimension separation,
  actual payload/entry eviction and oversized-resource rejection. Nine hardware
  renderer tests, 20 ordinary renderer tests, 107 enabled window/Vulkan runtime
  tests and strict Clippy pass for the prototype.
- Intro, overworld, battle, longer-session and restart replays match the frozen
  f1a17a2 executable and same-binary unpooled reference: 74 images and five WAVs
  per path, saves and guest counters agree, with zero software fallbacks. The
  overworld allocates three temporary seed sets and reuses them 1326 times,
  versus 1329 allocations with pooling disabled.
- Four balanced baseline/candidate trials per scene yield throughput changes
  of +1.33% intro, -0.57% overworld and +0.05% battle. The intro baseline includes
  a slow first trial (6.381 seconds versus 6.093/6.195/6.186), while candidate
  times are 6.152/6.066/6.140/6.170. Overworld means are 13.20675/13.28275
  seconds; battle means are 20.84725/20.8375. Dumps/profiling are disabled;
  observed encoder/Rust-build overlap is rejected without interrupting encoders.
- Allocation reduction does not demonstrate a gameplay improvement. Remove
  the prototype, comparison switch and counters; retain the patch, frozen
  executables, fixtures and measurements privately. Omit the queued window
  matrix once the primary gameplay result rejects the change. No window
  performance gain or shipped allocation pool is claimed.

### R-122 2026-10-29 — Default Vulkan play builds and clean Linux build instructions

- Newly generated `play` manifests request both window and Vulkan support.
  Plain `cargo +stable build --release -j 2 -p play` includes the renderer;
  normal launches default to guarded acceleration, hardware vertex processing,
  depth feedback, deferred readback and GPU presentation. No guest instructions,
  clock charging, simulation timing or frame-rate unlock are changed.
- `USUM_VULKAN_ACCELERATE=0` selects the software reference at native scale.
  Use the same selection policy for launch validation, rendering, presentation
  and resolution controls. Explicit live-comparison modes keep their existing
  software-active behavior unless acceleration is explicitly requested.
  `USUM_VULKAN_DEPTH_FEEDBACK=0` disables that default; the older alias remains
  accepted, with the primary setting taking precedence. The deterministic probe
  retains explicit renderer selection and its existing diagnostic semantics.
- Rewrite BUILDING.md around Debian/Fedora dependencies, stable Rust, tool
  compilation, private translation, the generated game build, ordinary launch,
  controls, saves, updates and troubleshooting. Move differential testing,
  profiling/replays and experimental renderer switches into DEVELOPMENT.md.
  Check distribution package names against primary inventories, including
  Fedora's split libwayland-client/libwayland-cursor packages. Document the
  generated workspace's separate lockfile and the need to regenerate after
  translator changes. Do not claim clean-install validation on both distros:
  the available local build host is Ubuntu, with no container runner installed.
- All 218 enabled default-workspace tests with window/Vulkan features pass,
  as do strict all-target Clippy, formatting, CPU-window-only checking and the
  publication-guard tests. Documentation links/anchors and shell block syntax
  pass. Regenerating the private Ultra Sun workspace leaves all 269 Rust source
  files byte-identical and updates only the intended play dependency features;
  its ordinary build succeeds without explicit feature arguments.
- Fixed-save/input intro, overworld and battle window runs compare frozen
  f1a17a2 with explicit acceleration against the candidate with all renderer
  switches unset and with acceleration explicitly enabled. Game instruction,
  VBlank, draw/triangle/submission counts and saves agree; candidate guest-clock
  stamps also agree. All hardware runs use resident GPU presentation, with zero
  software fallbacks and zero materialized display-source uses. Explicit software
  and an unavailable-ICD fallback both complete the intro with matching guest
  counters and saves. This establishes default selection and fallback, not a
  new rendering speedup or native dynamic FPS implementation.

### R-123 2026-10-29 — Reject uniform seed clears without overworld gains

- Test replacing uniform scaled color/depth seed uploads and expansion with
  attachment clears. Validate every byte/depth bit before specializing; retain
  the original shader for nonuniform planes, subnormal or negative-zero depth,
  and the same-binary reference switch. Preserve stencil with load/store.
- All 256 channel values at scales 2/3/6/8 match the original GPU path exactly,
  including depth bits and previously seeded nonuniform stencil. Eight renderer
  hardware tests, 21 ordinary renderer tests, 108 enabled window/Vulkan runtime
  tests and strict Clippy pass. CPU checks reject last-pixel/last-depth changes,
  partial planes, nonfinite/out-of-range values and tiny/signed-zero depth.
- Freeze an exact 3e42c3b baseline: its rebuilt play executable matches the
  previously validated binary byte-for-byte. Fresh baseline, candidate and
  candidate reference replays agree on all 42 intro/overworld/battle images,
  three WAVs, saves and guest counters, with zero software fallbacks. The
  candidate actually uses 684/991/1683 clears in those respective fixtures.
  Capture-run elapsed times are not used as benchmarks; diagnostic work can
  overlap those correctness-only runs.
- Isolated balanced trials disable captures/profiling and reject observed
  encoder/Rust-build overlap without interrupting other jobs. Intro baseline
  6.204/6.221 versus candidate 6.119/6.118 seconds gives +1.54% throughput;
  overworld baseline 13.196/13.186/13.247/13.242 versus candidate
  13.322/13.218/13.319/13.230 gives -0.41%; battle baseline 20.897/20.807 versus
  candidate 20.820/20.898 gives -0.03%. Same-binary reference pairs are
  6.192/6.192, 13.357/13.381 and 20.866/20.782 seconds. Reduced upload work does
  not establish the desired gameplay gain against the frozen baseline.
- Remove the experiment and counters. No long/restart or window candidate
  matrix is claimed after the primary gameplay result rejects the change.
  Preserve private artifacts, then rebuild restored probe/play and compare both
  byte-for-byte to the frozen baseline. Vulkan remains the default; no discarded
  seed optimization remains in the runnable build.

### R-124 2026-10-29 — Cache repeated dispatch addresses before hashing

- Fresh user-only CPU stack profiles of 3e42c3b put dispatch resolution at
  12.30% exclusive/15.65% inclusive intro samples and 1.91%/2.17% in overworld.
  The frequent matrix helper is 1.57% inclusive in overworld; prioritize the
  larger movie dispatch cost before extending floating-point transfer batching.
  Sampling is diagnostic, not an elapsed-time benchmark. User-only callchains
  avoid changing host profiling permissions.
- Add 256 tagged address slots ahead of the existing hash cache. Preserve full
  ARM/Thumb keys and validate module index, base, extent, name and SHA-256 on
  every module hit. Collisions and invalid owners use the existing resolution
  path. Share immutable owner metadata between cache levels, avoiding string
  copies on refill. Instruction budgets, guest state and clocks are unchanged.
  `USUM_DISPATCH_FRONT_CACHE=0` retains hash-only lookup; the existing master
  switch disables both. `USUM_DISPATCH_STATS=1` logs observed cache counts.
- All 109 enabled window/Vulkan runtime tests and 76 non-window tests pass,
  alongside strict runtime Clippy and formatting. Extend module-lifecycle tests
  through uncached/hash/front modes, covering unload, replacement, relocation,
  changed names/hashes/extents, instruction state and removed modules. A forced
  collision test proves distinct code keys retain the checked backing lookup.
- Five fixed-input/save fixtures match frozen 3e42c3b and hash-only reference:
  all 74 images, five WAVs, saves, guest instructions, VBlanks, draw/triangle/
  video/audio/submission counts agree. Restart also matches. Hardware runs have
  zero software fallbacks. Observed front hit rates are 95.6% intro and 93.7%
  overworld. Synthetic lookup timing shows reduced helper work; conditional
  statistics updates are slower in that component test, so that variant is not
  adopted. Component throughput is not projected gameplay FPS.
- Balanced isolated 6× probe trials use four baseline/candidate runs in intro
  and battle and eight in overworld, with captures/profiling disabled and
  observed encoder/build overlap rejected without stopping other jobs:

  | Fixture | Baseline mean | Candidate mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 6.12575 s | 6.04300 s | +1.37% |
  | Overworld, 16 guest seconds | 13.17550 s | 13.12000 s | +0.42% |
  | Battle, 30 guest seconds | 20.78875 s | 20.67075 s | +0.57% |

- Normal-speed Vulkan window trials retain 6× internal resolution, fixed input/
  saves and old/new/new/old ordering. Intro game-thread elapsed time averages
  6.3250 → 6.2090 seconds (+1.87% throughput). Overworld is 16.0905 → 16.1200
  seconds (-0.18%); battle is 30.0180 → 30.0190. Those gameplay runs remain
  pacing-limited and do not demonstrate a meaningful FPS gain. Guest counters
  and saves agree; resident presentation has no materialized source uses.
- Separate balanced native-counter runs retire 0.26% fewer intro user
  instructions and 0.17% fewer overworld instructions. Intro user cycles fall
  1.03%, while overworld cycles rise 3.41% in those four trials. Counters run
  100% scheduled but include driver/worker activity; this disagreement limits
  confidence in the small overworld benefit. Retain the measured intro improvement,
  report gameplay gains as small/uncertain, and keep the comparison switch.
  No native FPS unlock or timing change is claimed.

### R-125 2026-10-29 — Full native cadence requests still need simulation correction

- Follow the application's process manager into the active FieldRo update and
  resolve the actor-position getter in the user's private module. These are
  read-only diagnostics; no module dump, game code or capture is published.
- Revisit the earlier cadence experiment using the engine's pending-rate
  request, allowing its normal input-manager transition side effects to run.
  Start both variants from the same save/input state at VBlank 600. Record
  actual actor positions at common VBlanks rather than inferring timing from
  encounter screenshots alone.
- Align 192 common VBlank samples across the full trace. Restricting path-length
  comparison to VBlanks 620–800 gives about 932.95 versus 1853.50 units for normal/higher cadence:
  approximately 1.99× movement for the same guest-time/input interval. Audio
  differs too. The higher-cadence run submits 675 top-screen buffers versus 498
  and executes 1533116635 versus 1170318582 guest instructions over 956 VBlanks.
- More rendering is real, but the simulation behavior is wrong. Do not ship
  this request as native dynamic FPS. Movement, animation and event advancement
  need a timing-preserving design before the rate can increase. These runs use
  the virtual clock and are not normal-speed FPS benchmarks; no timing patch or
  frame-rate option from the experiment is enabled in normal gameplay.

### R-126 2026-10-29 — Let Mystery Gift release its network-exclusive state

- Replayed the reported back-out stop with its original starting save and input.
  The old runtime stops at VBlank 4443 on `ndm:u` request `0x00020002`, after
  entering exclusive local communication state at VBlank 4385. This is a missing
  service reply, not a GPU crash.
- Implement the explicit `LeaveExclusiveState` command, accepting only its
  process-ID descriptor and returning a result-only reply. It matches the existing
  offline `EnterExclusiveState` handling: no network daemons run in this runtime.
  Protocol reference: [libctru NDM requests](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/ndm.c).
  Unknown requests and invalid descriptors still stop with diagnostics.
- The same replay now reaches 90 game seconds / 5384 VBlanks and the Mystery Gift
  menu. All 56 periodic images before the old stop and all 2,430,560 preceding
  audio sample frames are byte-identical; saves remain unchanged. Appending Back
  leaves Mystery Gift through the normal application restart and resumes the
  intro. That longer movie replay needs a larger diagnostic instruction budget;
  exhausting the probe's default budget is not a guest service failure.
- All five established regression fixtures preserve 74 images, five WAVs, saves,
  instruction/VBlank/draw/triangle/video/audio/submission counts exactly. Runtime
  tests: 110 passed with window/Vulkan, 12 hardware tests ignored; strict Clippy
  and formatting pass. New IPC coverage checks repeated entry/exit, replies,
  unchanged clocks and rejection of malformed/unknown messages.
- Wider menu testing is ongoing: the Internet option reaches a separate missing
  `ac:u` client-version request. This fix does not implement online gift delivery
  or establish complete Mystery Gift compatibility. No performance gain claimed.
  Private recordings, saves, captures and executables are retained outside Git.

### R-127 2026-10-29 — Isolate the field simulation cadence before enabling native dynamic FPS

- Extend R-125 with a private diagnostic around the active field process's main
  update and pre-update callbacks. Request the higher cadence through the normal
  engine transition at VBlank 600, then omit alternating steady-field updates.
  This tests which callback advances simulation; it is not a production policy.
  Fixture object addresses and modified callback tables stay outside Git.
- Repeat the same starting save and recorded input in four variants: normal
  cadence, higher cadence, higher cadence with alternating main updates, and
  higher cadence with alternating main/pre updates. All reach 956 VBlanks.
  Normal main/pre callback counts are 345/334. Higher cadence runs 522/511;
  gating restores 345 main calls (177 skipped), or 345/334 with both gated.
- Across 98 common position samples in VBlanks 620–800, path lengths are 932.95,
  1853.48, 932.50 and 932.07 units respectively. Both gated variants end at the
  normal run's position, while intermediate positions differ by up to about
  19.2 units. Top-screen submissions remain 675 versus 498 at normal cadence.
- Audio and rendered output still differ. Extra submissions with held simulation
  updates do not prove additional smooth animation poses or native FPS. This
  establishes the field update as a timing domain worth separating, not a usable
  unlock. Investigate the remaining input/update phase and audio differences,
  then movement, animation and event timestep handling. Preserve battle/menu
  transitions and the original compatibility path throughout.
- These virtual-clock runs are behavioral diagnostics, not normal-speed FPS
  measurements. No hardcoded field gate, clock acceleration, generated frames
  or unlocked-rate setting is enabled in the shipped runtime.

### R-128 2026-10-29 — Reduce worker scheduling for GPU vertex-input preparation

- Fresh post-dispatch user-cycle profiles put overworld `InputPlan::load` at
  3.78% self samples and worker waiting/yielding high in the call stacks. Intro
  remains dominated by dispatch, movie helpers and checked word writes; changing
  vertex scheduling does not target that movie work. Profiles are diagnostic,
  not FPS measurements.
- Raise the GPU vertex-input parallel threshold from 256 to 4096 distinct inputs
  per draw. The existing decoder, input ordering, floating-point bits, checked
  memory and GPU shader path remain unchanged. CPU shading retains its original
  scheduling. `USUM_VULKAN_INPUT_PARALLEL_MIN=256` restores the old threshold in
  the same binary; the process reads this setting on first use.
- Frozen baseline, candidate and same-binary reference runs of intro, overworld,
  battle, longer gameplay and restart agree on all 74 images, five WAVs, saves
  and guest execution/render/audio counters. All Vulkan runs have zero software
  fallbacks. Capture/profiling are disabled for isolated timing trials, encoders
  are allowed to finish, and overlapping measurements are discarded.

  | Fixture | Baseline mean | Candidate mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 6.22425 s | 6.18000 s | +0.72% |
  | Overworld, 16 guest seconds | 13.45075 s | 13.06175 s | +2.98% |
  | Battle, 30 guest seconds | 20.67475 s | 20.16500 s | +2.53% |

- Four trials per baseline/candidate are interleaved, with two old-threshold
  reference trials per scene. Reference means are 6.1695/13.5565/20.736 seconds.
  The tiny headless intro change is not a reliable improvement; its draws have
  about four distinct inputs and never used the parallel branch.
- Normal-speed window runs use old/new/new/old ordering and identical replay
  inputs/saves at 6×. Intro averages 6.230 → 6.317 seconds (1.38% lower throughput
  in this short window set); overworld 16.1835 → 16.0210 (+1.01%); battle stays
  30.0190 seconds. Report that observed intro regression rather than inferring a
  universal win. Gameplay remains cadence-limited; these timings demonstrate
  modest headroom, not a native FPS unlock. Guest counters and saves match, and
  resident presentation uses no materialized display sources.
- Separate balanced user-only hardware-counter runs show 9.77% fewer retired
  host instructions and 22.04% fewer cycles in overworld, and 7.06%/17.21% fewer
  in battle. All counters were fully scheduled. These include worker/driver
  activity and support reduced scheduling overhead, not proportional FPS gains.
  One overlapping battle trial was discarded and repeated.
- The frozen optimization candidate passed all 110 enabled window/Vulkan runtime
  tests (12 hardware tests ignored), strict Clippy and formatting. No guest clock,
  frame pacing, native timestep or game-service behavior changes in this patch.

### R-129 2026-10-29 — Extend offline Mystery Gift initialization and self-content reads

- The shortened fixed-input menu replay reaches AC client-version registration,
  default configuration creation, disconnect-event registration, the EULA-version
  config block and SelfNCCH reads. Previously each missing request stopped the
  guest. Add only the observed protocol operations; do not report an Internet
  connection, accept an agreement or emulate successful gift delivery.
- AC reports disconnected/no configured access-point slot. Its empty default
  configuration writes only the validated 512-byte static buffer. Registration
  validates an event handle and does not signal a disconnection that never
  happened. Last-error queries return no AC error when no connection was attempted;
  the observed setup failure originates in the guest before ConnectAsync.
  See [libctru AC requests](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/ac.c).
- CFG exposes zero agreed/latest EULA versions. SelfNCCH archive handles share
  the archive allocator but never map into host save paths. Read-only raw RomFS
  and icon/banner/logo ExeFS sessions read bounded ranges from the user's dump.
  Missing menu files return not-found; unsupported paths and writes remain
  rejected. Open files survive archive closure; process restart retains the
  source images but discards old archive handles. See
  [SelfNCCH access paths](https://www.3dbrew.org/wiki/Filesystem_services).
- Synthetic tests cover high-word archive handles, independent file lifetime,
  bounded reads, missing menu files, executable-path/write rejection, restart
  retention, offline status, event validation and static-buffer permissions.
  All 115 enabled window/Vulkan runtime tests pass (12 hardware tests ignored),
  with strict Clippy and formatting.
- Menu coverage confirms local-wireless searching can be cancelled to the root
  menu, then exited through normal application restart. Card-album and Read Info
  navigation also return. Code/password warning pages can be cancelled, but that
  does not validate the keyboard or submission path. Internet now reaches the
  missing Error Display library applet (0x406); confirmed infrared reaches missing
  InitializeIrnopShared. These remain explicit compatibility gaps, not completed
  menu support. No networking or performance improvement is claimed here.

- Five fixed-input/save regression fixtures retain all 74 images, five WAVs,
  saves and execution/render/audio counters exactly against the frozen R-128
  candidate. Restart coverage also matches; no Vulkan software fallbacks occur.

### R-130 2026-10-29 — Narrow native movement timing and distinguish audio events

- Extend R-127 with DSP source-event logs. The two streaming music channels have
  identical event records at normal/higher/gated cadences. Non-music events differ:
  the first observed 3041-sample effect starts at DSP frame 2075 normally, 2069
  with unrestricted higher cadence and 2076 with field gating. Later effect
  choices also differ. This implicates event timing/selection; it does not prove
  all audio playback or animation timing correct.
- A private write watchpoint on the active actor's X coordinate identifies two
  position-writing callers through a common setter. A narrower diagnostic scales
  only the primary player's movement increment by half at the higher cadence,
  leaving field updates active. The setter keeps its original guest instruction
  charge; no generic memcpy/vector helper or shipped runtime is replaced.
- Over 86 common samples in VBlanks 620–800, normal/higher/scaled path lengths
  are 932.95/1853.50/960.07 units. The scaled run still differs by up to 54.7 units,
  and its VBlank-900 capture has already entered an encounter transition. It
  submits 590 top-screen buffers, versus 498 normal and 675 unrestricted.
  Movement scaling alone therefore fails to preserve behavior: collision,
  animation and event/encounter advancement need coordinated timing work.
- Keep these fixture-address diagnostics outside the product. Their wall times
  include tracing and sometimes concurrent correctness work, so they are not
  performance comparisons. Neither experiment is enabled as native dynamic FPS.

### R-131 2026-10-30 — Complete recorded offline Mystery Gift error and cancellation paths

- Extend R-126/R-129 from initial menu back-out to confirmed communication
  choices. Implement IRUSER shared-memory initialization, empty packet rings,
  disconnected status, search state, cancellation events and finalization.
  No peer, received gift or successful connection is fabricated. The recorded
  infrared and local search paths now cancel to the menu and restart normally.
  IR AutoConnection remains an offline pending search until cancellation;
  automatic timeout and real communication are not implemented.
- Support the Error Display library applet's capture handshake and host dialog.
  GlanceParameter and ReceiveParameter deliver separate client handles, so
  closing a glanced handle no longer destroys the queued capture object.
  UnmapMemoryBlock releases a complete shared mapping while retaining its bytes
  for remapping; mismatched mappings are rejected before mutation. Memory lookup
  indices and QueryMemory blocks are updated together. The host overlay leaves
  guest VRAM in place during the scoped capture/save/restore handoff.
- Validate the observed 0xF80-byte SDK parameter layout and its return field.
  A fresh A/B press acknowledges a network error through the normal APT resume
  parameter. Unknown dialog types/layouts remain unsupported. An unavailable
  agreement returns NOT_SUPPORTED without changing the agreed version; no
  agreement is silently accepted. Protocol references include
  [libctru APT](https://github.com/devkitPro/libctru/blob/master/libctru/source/services/apt.c),
  [error applet declarations](https://github.com/devkitPro/libctru/blob/master/libctru/include/3ds/applets/error.h)
  and [SVC mapping definitions](https://www.3dbrew.org/wiki/SVC).
- Recorded 6× Vulkan checks cover Internet error 30399, acknowledgment, the
  game's own error message, menu return and restart; a second attempt in the
  same session also completes that flow. The code/password route passes its
  Internet confirmation and returns through the same offline error/restart
  path. This does not validate software-keyboard entry or online submission.
  A 35-second windowed replay visually confirms the host error text with the
  testing FPS overlay disabled and reports zero software fallbacks.
- The five frozen intro/overworld/battle/long/restart fixtures match the R-129
  executable: 74 images and five WAVs are byte-identical, with equal saves,
  guest instructions, VBlanks, draws, triangles, framebuffer submissions, movie
  jobs and audio counts. The 60 numbered Internet captures preceding the old
  shared-memory stop also match. Captures, binaries, inputs and save hashes
  remain private; these correctness runs are not performance benchmarks.
- All 33 guest-memory tests and 121 enabled window/Vulkan runtime tests pass
  (12 runtime hardware tests remain ignored in that unit invocation). The
  minimal runtime configuration passes 88 tests. Strict Clippy passes for the
  changed crates/configurations, as do formatting and whitespace checks.
  Focused tests cover invalid dialog parameters, repeat preparation, one-time
  acknowledgment, unchanged clocks, handle lifetime, remapping contents and
  stale memory-index prevention.
- No native FPS unlock or new performance gain is claimed here. Native dynamic
  FPS remains core work: additional engine-rendered frames must advance scene
  state correctly while preserving movement, animation, encounter decisions,
  event ordering and audio timing. R-130's movement-only correction is still
  excluded from the product.

### R-132 2026-10-30 — Bulk stencil extraction from mapped Vulkan readbacks

- Fresh post-R-131 profiles put the pending readback materialization closure at
  3.35% self time in the overworld and 2.86% in the intro. Instruction annotation
  concentrates its samples in the per-pixel stencil push loop, including repeated
  vector-length loads, bounds checks and row-offset arithmetic. These profiles
  supersede older bottleneck rankings for this experiment.
- Extract contiguous stencil rows with a bulk slice copy, or a fixed-stride
  iterator for four-byte stencil elements. Preserve the original scalar loop
  with `USUM_VULKAN_BULK_STENCIL=0` for comparison. Mapping, synchronization,
  color/depth conversion, guest timing and rendered dimensions are unchanged.
- Four alternating frozen-baseline/candidate runs at 6× give mean isolated
  replay times of 6.024/5.928 seconds for the intro, 12.844/12.725 seconds for
  the overworld and 20.204/20.035 seconds for battle: approximately 1.62%,
  0.93% and 0.84% throughput gains. Two same-binary scalar-reference means
  are 5.961, 12.870 and 20.257 seconds respectively. Gains are small and noisy;
  the last battle candidate trial is slower than its adjacent baseline.
- Two counter trials per executable reduce host user-space instructions by
  about 2.62% in the overworld and 2.71% in battle, with fully scheduled counters.
  Counter totals are CPU-work evidence, not FPS projections. Wait for external
  encoders and discard an observed overlapping trial; never stop those jobs.
- Normal-speed windowed means are 6.215/6.175 seconds for the intro,
  16.076/16.021 for the overworld and 30.019/30.018 for battle. These remain
  largely pacing-limited and do not establish a meaningful gameplay FPS gain.
  All trials report zero software fallbacks and identical guest work counts.
- Frozen intro, overworld, battle, long and restart fixtures produce 74
  byte-identical images, five identical WAVs, unchanged saves and matching
  guest/render/audio counters. Short fixtures also match with the scalar
  reference selected. Inputs, initial saves, binary hashes and captures are
  retained privately under `stencil-readback-validation`.
- All 21 enabled renderer and 121 enabled runtime unit tests pass. All seven
  renderer and 12 runtime hardware tests pass with each implementation,
  including padded stencil rows, target reuse and scaled rendering. Strict
  Clippy and formatting checks pass. Other hardware and full-game behavior
  remain unverified.
- Native dynamic FPS remains separate core work. This change reduces CPU cost
  while preserving the current engine cadence; it does not uncap simulation
  or manufacture extra game frames.

### R-133 2026-10-30 — Separate movement accumulation from actor displacement

- Read-only field-domain tracing reproduces the prior normal, higher-cadence
  and half-position diagnostic outputs exactly: trajectories, VBlank-900
  captures, audio WAVs and audio event logs. Five observed field entry points
  run approximately twice as often over the same VBlank interval at the higher
  cadence. This is partial coverage: direct translated calls bypass some entry
  wrappers, so absent observations do not imply an inactive subsystem.
- Follow the primary actor position setter into its caller. After writing the
  position, the guest separately adds the movement vector's magnitude to an
  accumulator, compares it with a threshold and retains a remainder. Halving
  only the setter's position argument leaves that accounting unscaled. The
  caller also uses the vector for later direction/state bookkeeping; it cannot
  safely be treated as an isolated position-only delta.
- A private experiment additionally halves that caller's movement vector after
  the setter, preserving its original instruction charge and leaving all field
  updates active. The observed accumulator wraps 19 times in the normal run,
  38 with half-position alone and 20 with the additional vector scaling. These
  are observed counter decreases, not a proven count of encounter rolls.
- At VBlank 900 the new experiment remains in the overworld, like the control;
  the half-position experiment is already in an encounter transition. It
  submits 675 top buffers versus 498 normal and 590 half-position, but this
  does not establish native dynamic FPS correctness. Over the same 86 shared
  trajectory samples, travel is 932.95/960.07/968.27 units for normal,
  half-position and additional vector scaling. Maximum positional divergence
  remains 54.67 units. Turning, collision and animation still need investigation.
- The new run's source-zero audio event log matches the control, while other
  source events number 403 versus 349. The waveform and rendered pose differ.
  More frames with these behavioral differences are not a shipping unlock.
- Keep these save-specific addresses, generated diagnostics and captures private
  under `native-fps-research`. This is causal timing research, not a throughput
  benchmark or a production change. Native dynamic FPS remains a core feature
  under development, with normal game/audio/event speed required.

- A hardware watchpoint on the diagnostic child's player quaternion identifies
  a separate turning interpolation. At the observed roughly 0.6 blend per
  update, test `1 - sqrt(1 - blend)` at double cadence to preserve the response
  to a fixed target over equal time. Both instrumented controls still match
  their prior outputs exactly. The experimental correction does not solve the
  moving-input replay: across 100 common samples, mean position error rises
  from 22.79 to 26.32 units, while maximum error changes from 54.67 to 54.25.
  Non-source-zero audio events rise from 403 to 411. Reject this as a standalone
  fix; retain only the private diagnostic and evidence. Input sampling/pipeline
  latency, changing targets and the remaining animation timing are unresolved.

### R-134 2026-10-30 — Copy LCD rows in Morton-tile groups

- Fresh post-R-132 CPU profiles still place native DisplayTransfer conversion
  at 1.48% self time in the overworld. The intro instead remains dominated by
  dispatch (7.17%), movie-module functions, the tree accessor and checked memory.
  Preserve those separate CPU targets for subsequent work.
- For the existing unfiltered tiled RGBA8-to-linear RGB8 case, compute the row
  origin once and copy eight pixels using their fixed Morton offsets. Require
  only the sampled prefix of each tile row; retain per-pixel handling for partial
  tails. Cropping, flipping, errors, guest instruction charges, clocks, GPU
  synchronization and other formats/modes are unchanged.
  `USUM_DISPLAY_TILE_ROWS=0` selects the original per-pixel reference.
- Four alternating frozen-baseline/candidate isolated runs at 6× average
  5.896/5.882 seconds for the intro, 12.690/12.567 for the overworld and
  19.937/19.674 for battle: about 0.24%, 0.98% and 1.34% higher throughput.
  Same-binary reference pairs average 5.904/12.551/19.929 seconds; the overworld
  reference is slightly faster than the candidate, so wall-time attribution
  remains noisy and code layout contributes to the frozen-build comparison.
- Two fully scheduled user-space counter trials per executable show 3.94% fewer
  host instructions and 2.18% fewer cycles in the overworld; battle drops
  3.99% instructions and 2.16% cycles. These are CPU-work measurements, not
  extrapolated FPS. Trials wait for other encoders/builds/replays to finish.
- Normal-speed windowed means are 6.186/6.066 seconds for the intro,
  16.021/16.021 for the overworld and 30.018/30.018 for battle. Intro's nominal
  gain is about 1.99% in this pair; gameplay remains cadence-limited. All
  windowed runs use resident Vulkan presentation with zero software fallbacks
  and no materialized display-source uses.
- Intro, overworld, battle, long and restart fixtures preserve 74 byte-identical
  screen captures, five identical WAVs, saves, guest instructions, VBlanks,
  geometry, submissions, movie jobs and audio counts. Short fixtures also match
  with the per-pixel reference selected. All 122 enabled runtime unit tests pass
  with each path, including a generic-format oracle over tile boundaries,
  unaligned advertised widths, empty/cropped/flipped output and short input.
  The 12 hardware-specific runtime tests remain ignored in that unit invocation;
  recorded Vulkan runs supply the integration evidence. Strict Clippy,
  formatting and whitespace checks pass.
- Retain private frozen binaries, source/input/save hashes, captures and results
  under `display-tile-validation`. This is a small timing-neutral CPU reduction;
  native dynamic FPS and broader full-game performance remain active work.

### R-135 2026-10-30 — Validate cached code against tracked module revisions

- The post-R-132 intro profile still spends 7.17% self time resolving host code.
  Replace repeated cached-owner string/size/base checks with a revision token
  for the loaded-module list. Every mutable borrow replaces the token before
  access escapes, including indexing, iteration and access through the Vec.
  Cached tokens keep their allocations alive, preventing address reuse from
  validating stale ownership. Clones can share a token while contents agree;
  mutation of either clone creates a fresh token.
- Lookup after invalidation still checks the translated module's name and hash
  and recomputes relocation ownership. Executable precedence and ARM/Thumb
  keys are unchanged. `USUM_DISPATCH_MODULE_REVISION=0` retains the original
  per-hit metadata checks; the existing cache/front-cache reference switches
  remain available. This differs from R-099's rejected weak-identity experiment:
  no weak-reference upgrade or reference-count operation is needed on a hit.
- Four alternating frozen-baseline/candidate 6× replay runs average
  5.849/5.716 seconds in the intro, 12.498/12.474 in the overworld and
  19.661/19.563 in battle: about 2.31%, 0.19% and 0.50% higher throughput.
  Same-binary metadata-reference pairs average 5.949/12.610/19.547 seconds.
  Battle's reference is slightly faster than the candidate, so these small
  gameplay differences do not establish a robust FPS gain.
- Two fully scheduled user-space counter trials per executable reduce intro
  instructions by 3.44% and cycles by 2.15%; overworld instructions/cycles drop
  0.33%/1.58%, and battle drops 0.71%/0.39%. Wait for other builds/replays and
  external encoders, discard observed overlaps and retain the accepted repeats.
  No external process is stopped. Counter reductions are not FPS projections.
- Normal-speed windowed means are 6.041/6.005 seconds for the intro,
  16.020/16.021 for the overworld and 30.018/30.017 for battle. The intro
  nominally improves about 0.60%; gameplay remains pacing-limited. All window
  runs retain resident GPU presentation, zero software fallbacks and no
  materialized display-source uses.
- Five identical-input/save fixtures retain 74 byte-identical images, five
  identical WAVs, unchanged saves and matching guest/render/audio counters.
  Short fixtures also match with metadata checks selected. All 123 enabled
  window/Vulkan runtime tests pass with both policies, and the minimal runtime
  passes 90 tests. Lifecycle tests cover unloads, replacement, relocation,
  changed name/hash/size, table replacement, clones, borrowed-Vec mutation and
  instruction state. The 12 hardware-specific unit tests remain ignored in
  that invocation; recorded Vulkan replays provide integration coverage. Strict
  Clippy, formatting and whitespace checks pass.
- Private binaries, hashes, captures and benchmark results are retained under
  `module-revision-validation`. Guest code, instruction charges, clocks and
  native frame cadence are unchanged. Full-game behavior remains unverified.

### R-136 2026-10-30 — Coordinate animation stepping and movement caps

- A hardware watchpoint in the private diagnostic child identifies animation
  state with a stored rate, current phase and previous phase. Root-motion
  extraction also consumes the stored rate. Halving only phase advancement
  leaves movement extraction inconsistent; it still produces roughly double
  travel in the recorded higher-cadence run. Instrumented controls preserve
  their prior trajectories, final images, WAVs and audio-event logs exactly.
- A second diagnostic normalizes the observed player's stored unit rates to
  half steps. It changes the steady movement vector from 19 to about 11.78
  units, rather than the required 9.5. Trace the subsequent per-update cap:
  the original movement exceeds the 19-unit cap, while the half-step vector
  falls below it. The cap therefore needs the same timestep treatment.
- Cover both overlapping generated copies of the cap block. An initial probe
  changed only the resume translation and did not exercise the intended path;
  its unchanged result cannot test the hypothesis. Corrected instrumentation
  records 62 primary cap executions normally and 126 at the higher cadence.
  It preserves the original block instruction charges and scales the complete
  cap expression, producing steady 9.5-unit steps.
- Across 100 shared samples in the turning replay, normal/rate-only/coordinated
  travel is 932.95/1197.96/980.23 units. The coordinated run still differs by up
  to 54.76 units and produces 323 non-music source events versus 349 normally.
  It submits 675 top buffers versus 498 normal; additional rendering alone
  does not validate movement, collision, event or audio timing. Normalizing
  observed animation channels globally produces similar player results, with
  980.29 units of travel.
- Build a separate replay-input harness and retain the original replay as a
  control. Adding pose readbacks preserves that control's trajectory, final
  image, waveform and audio-event log. Four cardinal-direction scripts use
  the same starting save and pre-transition inputs, then hold a fixed circle
  direction before releasing it. For engine-frame interval [640,680), the
  downward and rightward runs each deliver 380 intended movement units at
  both cadences: 20 original steps versus 40 half steps. Upward intended
  movement is 347.20 units in each run; leftward differs about 0.23%.
- Between VBlanks 650 and 670, each straight replay captures 11 distinct
  guest-rendered top images normally and 21 at the higher cadence. These are
  game framebuffer outputs from additional engine work, with fractional
  animation stepping; no window repeats, generated frames or extrapolation
  are counted. These traced virtual-clock runs are correctness diagnostics,
  not normal-speed FPS benchmarks.
- World positions still diverge, with 26.7–38.4-unit maximum offsets in the
  sampled steady intervals. Audio source-zero events match in the turning
  and downward cases checked, while effects differ. Probe snapshots can lag
  the engine's current frame, so separate input/pipeline sampling effects from
  actual collision and timing defects before accepting broader equivalence.
- This remains private research under `native-fps-research/motion-*` and
  `straight-controls`, including input/save/source/binary hashes and frozen
  diagnostic libraries. The constant-rate rewrite handles only the observed
  rates and must not become production policy. Next, preserve nominal rates
  and apply an effective per-frame step consistently to advancement and root
  extraction; investigate blend clocks, camera updates and integer timers.
  Replace save-object addresses with validated engine bindings before exposing
  native dynamic FPS as a core feature. The normal build contains no timing
  diagnostic hooks, and its existing cadence is unchanged.

### R-137 2026-10-30 — Native single-precision arithmetic on a proven domain

- Fresh post-R-135 overworld samples put `round32` at 2.81% self time and
  `nans32` at 1.43%. The intro still centers on movie functions, calls, the
  tree accessor and dispatch. Target the common guest floating-point helper
  without changing translated instructions or the compatibility runtime's clock.
- Normal finite and signed-zero binary operands can use host f32 arithmetic.
  NaN/subnormal operands, NaN/subnormal results and the smallest-normal result
  retain the original f64/reference handling. The last exclusion matters:
  an exact tiny result may round up to the smallest normal float, while ARM FZ
  must still flush it. Directed conversions and square root are unchanged.
  No global fast-math setting is enabled. `USUM_GUEST_NATIVE_F32=0` retains
  the original arithmetic path.
- Test boundary bit patterns and 100,000 random operand pairs across add,
  subtract, multiply and divide with each FZ/DN combination, comparing exact
  result bits. Include signed zero, infinities, signaling/quiet NaN payloads
  and the tiny-result boundary. A native-CPU release test also verifies that
  multiply/accumulate rounds the product first, preserving VFP's non-fused
  semantics. All 36 guest tests and 123 enabled window/Vulkan runtime tests
  pass with both policies; strict Clippy and formatting checks pass. The 12
  hardware-specific runtime unit tests remain ignored in that invocation.
- Arithmetic-only microbenchmarks favored the prototype and justified the full
  generated rebuild; their speedups are not gameplay FPS estimates. Freeze a
  fresh production baseline and verify all five prior fixture outputs before
  comparing the new build. Intro, overworld, battle, long and restart replays
  preserve 74 byte-identical images, five identical WAVs, saves and guest/render/
  audio counters. Short fixtures also match with the reference path selected.
- Four alternating isolated baseline/candidate 6× replay runs average
  5.725/5.672 seconds for the intro, 12.544/12.067 for the overworld and
  19.561/19.130 for battle: approximately 0.93%, 3.95% and 2.25% higher
  throughput. Same-binary reference pairs average 5.696/12.521/19.606 seconds.
- Two fully scheduled counter trials per executable reduce host instructions
  3.69% in the overworld and 2.31% in battle. Intro instructions increase
  0.10%. Battle cycles fall 2.89%, intro cycles fall 0.38%, while overworld
  cycles average 0.09% higher with substantial trial variation. The cycle
  result therefore does not independently establish an overworld gain. Wait
  for other jobs and encoders and discard observed overlaps; none are stopped.
- Normal-speed windowed means are 6.005/6.005 seconds for the intro,
  16.021/16.020 for the overworld and 30.018/30.017 for battle. Existing cadence
  still limits presentation; no native frame-rate unlock is claimed. All runs
  use resident Vulkan presentation with zero software fallbacks and no
  materialized display-source uses. These fixture gains and exact outputs
  do not establish full-game performance or compatibility.
- Private sources, frozen binaries, input/save hashes, captures and accepted
  trial results remain under `native-f32-validation`. Native dynamic FPS
  continues separately using the effective-step research from R-136.

### R-138 2026-10-30 — Preserve nominal animation rates and trace effective-step consumers

- Continue R-136 in private generated-code copies and a manually linked diagnostic
  harness. Production translations and timing policy remain unchanged. Replace
  stored-rate rewrites with local half-steps at phase advance 0x0036a880 and both
  root-extraction copies containing 0x004b805c; scale the complete movement cap in
  both overlapping translations containing 0x003a9988. Nominal rates remain 0/1.
  Run the same starting save and replay through VBlank 956. The normal and raw
  higher-cadence controls match the previous trajectory, image, WAV and source
  event logs exactly. These correctness diagnostics are not speed benchmarks.
- Effective phase/root/cap steps alone travel 980.641 units across the 100 common
  trajectory checkpoints at VBlanks 620–800, versus 932.947 normally. Maximum
  position error is 54.766 units, mean 24.811. Keeping stored rates intact exposes
  another interval consumer: a hardware read/write watchpoint on our own child
  identifies the getter at 0x004b7f94, besides phase/root reads and observer reads.
  A separate getter wrapper traces 8,189 calls through 0x005ada88; outer callers
  resolve to movement update 0x003a975c (8,188 calls) and one field-module call.
  The movement update stores the result at controller offset 0x130.
- Give this getter an effective half-step in a separate private operation, without
  changing stored rates or guest instruction charges. Travel becomes 980.287,
  maximum error 54.763, mean 24.766. Non-streaming source events improve from
  131 to 225 against 251 normally; the two streaming source logs remain exact.
  For comparison with R-136's older metric, excluding source 0 alone gives
  229 → 323 against 349 normally. That metric also includes streaming source 1;
  it should not be interpreted as a count solely of effects or footsteps.
- Combine the effective getter with a half-speed blend clock: travel 980.455,
  maximum error 54.763, mean 25.088, 225 non-streaming events. Also adjust the
  identified turning response with f / (1 + sqrt(1 - f)): travel 978.372,
  maximum error 53.336, mean 25.354, 298 non-streaming events. Neither combination
  preserves the reference trajectory/event behavior. Reject them as a complete
  unlock, despite additional genuine engine-rendered frames in R-136.
- Record integer timer bits rather than formatting them as floats. All 24 observed
  controllers have 0xffffffff at offset 0xd4 throughout this experiment: the
  signed -1 sentinel, not a positive countdown. There is no evidence here for
  scaling that field. Preserve it and trace the effective-rate field's consumers
  and remaining movement/camera/event timing instead. Keep fixture addresses and
  generated diagnostic code private; validated engine bindings are still required
  before any core native-FPS implementation can use these findings.

### R-139 2026-10-30 — Prove scaled LCD source ownership without a pixel scan

- Fresh profiles after R-137 attribute 2.35%/1.93%/2.21% exclusive intro/world/
  battle user-cycle samples to scaled transfer capture. That path compares every
  sampled native pixel against a cached target even when guest memory still
  holds its exact immutable GPU color-plane owner. Other visible costs include
  dispatch/movie helpers, vertex input preparation, uploads and seed/readback.
- After the unchanged guest input read and LCD conversion, use the existing
  exact-range deferred color owner to prove the selected GPU generation. Retain
  crop/stride/format/scale bounds. A CPU write, unmap or different aliased owner
  loses the proof and uses the original byte comparison; ordinary RAM sources
  also retain that path. Neither readback nor guest framebuffer output becomes
  lazy. Depth ownership is unnecessary for a color-only LCD transfer.
  `USUM_VULKAN_DISPLAY_IDENTITY=0` retains the per-pixel source check.
- All 123 enabled window/Vulkan runtime tests, strict Clippy and formatting pass.
  Extend the Vulkan 6× cropped-target test through ordinary bytes, an immutable
  deferred owner, rotation/full-resolution presentation and a one-byte CPU write
  invalidating the source. It passes with both policies. The remaining hardware
  tests are not implied by this targeted invocation.
- Preserve R-137's frozen binaries and verified baseline fixtures. Intro/world/
  battle/long/restart replays keep all 74 images, five WAVs, saves and guest,
  rendering, conversion and audio counters identical. Short fixtures also match
  with the same-binary reference. There are zero software fallbacks.
- Four balanced isolated trials per executable, at 6× without captures/profiling:

  | Fixture | Baseline mean | Candidate mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 5.73350 s | 5.74350 s | -0.17% |
  | Overworld, 16 guest seconds | 12.44750 s | 12.32500 s | +0.99% |
  | Battle, 30 guest seconds | 19.69625 s | 19.44575 s | +1.29% |

- Intro candidate times are 6.154/5.607/5.588/5.625 seconds. Keep the slower first
  accepted trial in the result; do not infer a reliable intro wall-time gain.
  Same-binary reference means are 5.782/12.4075/19.7305 seconds. Discard observed
  compiler/diagnostic/encoder overlap and wait for other work; no external job
  is paused or stopped.
- Separate fully scheduled user-counter pairs reduce instructions 3.28% intro,
  3.53% overworld and 4.12% battle, with cycles down 1.91%/1.96%/1.35%. Normal-speed
  windowed means are 6.0055/6.0050, 16.0200/16.0195 and 30.0185/30.0190 seconds
  before/after: no meaningful windowed gain at the existing cadence. Saves and
  guest counters match; resident presentation has no materialized source uses.
- Keep the change for lower host work and the measured gameplay throughput gains.
  Native simulation timing is unchanged, and this does not unlock dynamic FPS.
  Broader compatibility and performance remain unverified outside these fixtures.
  Private binaries, input/save hashes and results are in display-identity-validation.

### R-140 2026-10-30 — Keep frequent dispatch hits outside the backing resolver

- The post-R-137 intro profile places CodeCache::resolve at 5.85% exclusive
  user-cycle samples. Annotated code reserves 232 stack bytes and saves six
  registers even on the frequent tagged-front hit. Most allocation, insertion,
  translation lookup and owner construction work is needed only after a miss.
- Inline a small front-hit path and keep backing lookup in a separate function.
  Owner revision checks, complete ARM/Thumb keys, collision fallback, full miss
  validation and counters retain their behavior. `USUM_DISPATCH_OUTLINE_MISSES=0`
  retains the original complete resolver. Existing cache/front/revision switches
  remain available. This changes no translated instruction, budget or game clock.
- A synthetic 32-key component test, with fixed mock module ownership and exact
  result checksums, favors the split (means 0.033569/0.031399 seconds for ten
  million lookups). It motivates a game trial; it is not gameplay evidence.
  All 123 enabled window/Vulkan runtime tests pass under both policies. Expand
  the loaded-module mutation/replacement/relocation/instruction-state lifecycle
  matrix through both resolver paths. Strict Clippy and formatting pass.
- Frozen R-139 baseline, split and same-binary original-resolver short replays
  match byte for byte. All five baseline/candidate fixtures preserve 74 images,
  five WAVs, saves and guest/render/conversion/audio counters, with zero software
  fallbacks. Private fixtures and executable hashes are in dispatch-outline-validation.
- Four balanced, isolated 6× replay trials per executable, without captures or
  profiling, average 5.62975/5.60525 seconds for intro (+0.44% throughput),
  12.27300/12.25350 for overworld (+0.16%) and 19.47425/19.48250 for battle
  (-0.04%). Same-binary reference means are 5.727/12.3735/19.533 seconds.
  Gameplay differences are within trial variation; do not claim a robust gameplay
  wall-time improvement. Wait for encoders and other jobs; never interrupt them.
- Two fully scheduled native-counter trials per executable reduce instructions
  2.87% intro, 0.65% overworld and 0.85% battle. Cycles change -0.69%/+0.16%/-0.43%
  respectively. Keep the lower host work and small intro improvement, with the
  gameplay uncertainty explicit. The component result is not projected to FPS.
- Normal-speed window means before/after are 6.005/6.005, 16.020/16.020 and
  30.019/30.0185 seconds. Existing pacing limits those runs. Saves, guest counters
  and resident presentation agree, with no materialized source uses. Neither a
  native-FPS unlock nor broader game compatibility is established by this change.

### R-141 2026-10-30 — Correct the first cadence step and locate an actual positive countdown

- Trace the event interval passed from 0x003aeb6c to 0x004ba088. The former reads
  controller offsets 0x12c/0x130 and scans from the previous animation phase to
  the current one. Normal cadence supplies 179 intervals in the post-transition
  replay; the higher-cadence effective-step experiment supplies 356. Source logs
  and interval counts are diagnostics, not sound accuracy or FPS measurements.
- The first higher-cadence update is at engine frame 602, following frame 600.
  A constant half-step incorrectly advances phase 4 → 4.5; the elapsed interval
  still requires 4 → 5. Give this first update its full phase/root/cap/getter
  step in a separate private build, then use half-steps at frames 603 onward.
  Initial phases align at common engine frames. A previously divergent
  3041-sample effect now starts at DSP frame 2076 versus 2075 normally; other
  effects and source allocations still differ. This does not fix all audio timing.
- The same 100 common trajectory checkpoints still travel 980.294 versus 932.947
  units normally, with maximum position error 54.692 and mean 24.584. Capture
  the primary actor's surrounding state at field updates. A different field
  contains a positive countdown 3/2/1/0 over successive updates, and another
  counts 0/1/2/3/4. These are separate from R-138's unchanged -1 sentinel.
- A hardware read/write watchpoint on our own child locates the countdown update
  in the field module's overlapping translations containing 0x6f051168. It is
  component offset 0x34, observed at controller-relative offset 0x1a8 in this
  fixture. The count-up is component offset 0x3c, in blocks containing
  0x6f050f40. Patch both translated copies in a separate private module library,
  retaining the original guest instruction charges and all normal translations.
- Test alternating integer updates on odd/even engine frames, first just the
  countdown, then both counters. Preserve an exact unmodified-transition control:
  its trajectory, top-screen image, WAV and source-event log match the earlier
  build. Results at the same 100 common trajectory checkpoints:

  | Private operation | Travel | Maximum position error | Mean position error | Non-streaming source events |
  |---|---:|---:|---:|---:|
  | Normal cadence | 932.947 | 0 | 0 | 251 |
  | Effective steps with corrected transition | 980.294 | 54.692 | 24.584 | 270 |
  | Countdown on odd frames | 925.425 | 40.543 | 23.352 | 270 |
  | Countdown on even frames | 916.253 | 45.538 | 31.417 | 270 |
  | Both counters on odd frames | 925.552 | 40.669 | 23.382 | 256 |
  | Both counters on even frames | 915.983 | 45.668 | 31.149 | 256 |

- State snapshots prove the countdown holds on intervening updates. They also
  expose an initialization-phase problem: the even-frame policy initially holds
  4 rather than the reference's 3. Integer duration needs a clock anchored to
  the component's initialization/reset, not an arbitrary global parity. Matching
  the number of source events alone does not establish matching choices/times.
- Each tested higher-cadence variant still produces 21 distinct guest-rendered
  images during VBlanks 650–670, versus 11 normally. No generated or extrapolated
  frames are used. Their movement and events remain different, so none is enabled
  as a core native-FPS unlock. These are timing-correctness experiments, with no
  controlled wall-time/FPS gain claimed. Fixture addresses, private generated
  patches, dependency hashes and results remain in native-fps-research.

### R-142 2026-10-30 — Reuse draw uploads with submission-scoped leases

- Obtain a normal-cadence gameplay-only profile after R-140, enabling user-space
  sampling at VBlank 600 and disabling it at 900 through acknowledged perf
  controls. Startup and output captures are outside this interval. Both native
  screen images at 900, the complete WAV/save and guest instructions match the
  normal replay. Draw-buffer upload accounts for 11.76% inclusive CPU samples;
  GPU allocation itself is 3.15% exclusive. Sampling is diagnostic, not FPS.
- Pool vertex-input, vertex-uniform, index and CPU-shaded-vertex allocations.
  Hold each allocation until its command batch is submitted or discarded;
  following queue writes occur after the preceding submitted commands. Exact
  binding lengths and vertex/index slices expose only current data, even when
  capacity is larger. Keep usage classes separate. Evicted handles are destroyed
  outside the pool lock. Fixed-function fragment-uniform uploads remain separate.
  `USUM_VULKAN_DRAW_UPLOAD_POOL=0` keeps the original mapped-allocation writer.
- A 128-entry linear prototype passes output checks but still allocates 72,366
  of 137,496 overworld uploads; four-pair replay gains are only about 1.44% world
  and 1.11% battle. Retain that prototype privately. The final pool bins capacity
  by power of two and buffer usage, holds at most 512 idle allocations and
  32 MiB, and does not retain allocations above 8 MiB. This avoids a long linear
  lookup while accommodating a larger command batch within the same byte cap.
- Ten Vulkan renderer tests pass, including new reverse-order submission,
  reuse before immutable readback materializes, discarded batches, exact shorter
  binding lengths, usage separation, and actual entry/byte/oversize limits.
  Respect wgpu's existing LIFO validation-scope ordering in the batch test.
  All 21 ordinary renderer tests, 123 enabled runtime tests with both writers,
  12 Vulkan runtime tests, strict Clippy and formatting pass.
- Five frozen baseline/candidate replays preserve all 74 images, five WAVs,
  saves and guest/render/conversion/audio counters, with zero software fallbacks.
  Short fixtures also match the same-binary original writer. The final overworld
  capture has 523 allocations and 136,973 reuses; battle has 524/187,714 and the
  long fixture 524/497,020. Retained storage is about 23.6 MiB in those scenes,
  below the 32 MiB bound. These counts concern the pooled upload categories,
  not every allocation in the renderer or driver.
- Four balanced isolated 6× trials per executable, without captures/profiling:

  | Fixture | Baseline mean | Final candidate mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 5.60125 s | 5.56175 s | +0.71% |
  | Overworld, 16 guest seconds | 12.37925 s | 12.04525 s | +2.77% |
  | Battle, 30 guest seconds | 19.46575 s | 19.04450 s | +2.21% |

- Same-binary original-writer means are 5.5965/12.398/19.412 seconds. Separate
  fully scheduled native-counter pairs reduce user cycles 2.03%/4.30%/4.17% in
  intro/world/battle. Retired instructions change -0.02%/+0.35%/+0.02%: this
  trades some instruction work for lower allocation/upload latency. Report that
  increase rather than treating allocation counts as a speed measurement.
- Normal-speed window means are 6.005/6.005, 16.020/16.0195 and 30.0185/30.017
  seconds before/after. Existing cadence limits those runs; resident presentation
  preserves counters/saves and has no materialized source uses. No native-FPS
  unlock or full-game compatibility is implied. Wait for encoders and other
  workloads, reject observed benchmark overlap, and stop no external process.
- A second acknowledged VBlank-600–900 profile preserves the same native images,
  WAV, save and guest count. Upload self-plus-callees drops from 11.76% to 4.92%
  of samples; allocator self samples fall 3.15% → 1.67%. Vertex-input assembly,
  framebuffer import and deferred readback remain significant. These proportions
  select future investigations; the controlled replay/counter trials establish
  this change's gain. Private evidence is in draw-upload-binned-validation and
  native-fps-research/steady-gameplay-profile-v2/-v3.

### R-143 2026-10-30 — Counter clocks and turning at higher cadence

- Extend the private R-141 timing diagnostic with per-counter fractional clocks
  anchored to observed resets/gaps, rather than global odd/even frame parity.
  Preserve the first elapsed interval at the cadence transition. Apply effective
  half-interval turning and animation-blend steps locally, retaining stored
  nominal rates. The clock currently recognizes one fixture component address;
  general identity, explicit initialization and reset/lifecycle hooks are absent.
- In the original turning replay, reset-aware countdown/countup clocks reduce
  common-checkpoint mean position error to about 23.38 game units. Adjusting the
  turning response reduces it to 8.68; additionally adjusting blend gives 8.54.
  Maximum error remains 26.37 with the blend adjustment. Travel is 924.56 versus
  932.95 normally. These residual errors prohibit enabling the experiment.
- There are 21 distinct engine-rendered poses during VBlanks 650–670 versus
  11 normally. Extra frames come from engine rendering and advanced scene state,
  with no generated/interpolated frames. Nonstream audio-source event counts
  remain different (261 versus 251); these counts are diagnostics, not direct
  counts of footsteps or encounters. No wall-time/FPS gain is claimed.
- Repeat four cardinal-direction recordings from the identical starting save.
  Every normal-cadence control reproduces its archived trajectory, final native
  image, full WAV and source-event trace exactly. During engine frames 640–679,
  normal/higher cadence takes 20/40 movement steps. Total step travel is:

  | Direction | Original cadence | Higher cadence |
  |---|---:|---:|
  | Down | 380.00000 | 380.00000 |
  | Left | 362.71440 | 362.92487 |
  | Right | 379.99999 | 380.00002 |
  | Up | 347.19991 | 347.19995 |

- Matching steady travel does not establish timing parity: maximum common-time
  position differences remain 19.44/18.29/25.40/18.81 units for down/left/right/up;
  means are 4.47/3.78/8.42/5.63. Investigate initialization, input transitions,
  animation boundaries and event decisions before broader integration. Normal
  generated game sources contain none of these private patches. Frozen binaries,
  dependency manifests and diagnostics remain in native-fps-research under
  clock-step-tests, blend-clock-tests and clock-straight-controls.

- Follow up the turning replay audio trace by collapsing dirty-flag-only changes.
  Nonstream playing/buffer/sample state changes are 94 normally versus 98 with
  the effective-clock/blend experiment, including 47 versus 49 source starts.
  Streaming sources 0/1 match exactly. Additional starts and changed sample
  choices confirm that the remaining audio difference is more than logging
  noise; no higher-cadence behavior is enabled.

### R-144 2026-10-30 — Reuse exactly validated decoded vertex inputs

- Follow the acknowledged gameplay-only R-142 profile: `InputPlan::load` takes
  6.66% inclusive samples (2.17% self), `element_in` 2.70% self, and input packing
  1.77% self. Repeated decoding is now a larger target than upload allocation.
  Keep the existing 3DS memory, shader and rendering paths.
- Add a bounded cache only for planned hardware input batches. Hash lookup is an
  index, never proof: compare the complete integer layout, ordered distinct
  indices, initial float bits and register destinations, then compare every
  current readable source span byte-for-byte. Include overwritten/discarded
  attributes. Source changes, missing permissions and remapping cannot reuse
  stale inputs. Wrapped, gapped or oversized span proofs bypass retention.
- Misses keep the original serial/parallel decoder and its first error. Insert
  only after successful decoding. Share immutable decoded vectors with command
  snapshots; never cache shader results or uniforms. PICA clones start cold.
  Retain at most 512 entries and 32 MiB of derived payload, including capacities,
  source copies and variable keys. Individual source/output limits are 1/8 MiB;
  fixed metadata is separately bounded by the entry limit. Keep
  `USUM_VULKAN_VERTEX_INPUT_CACHE=0` as the same-binary comparison path.
- All 128 enabled runtime tests pass with both cache policies, all 12 Vulkan
  runtime integration tests pass, and strict Clippy/formatting pass. Synthetic
  checks cover NaN payloads/signed zero, fixed aliases, index ordering, changed
  bytes, discarded inputs, unreadable/unmapped/remapped memory, wrapping and
  holes, deferred replacement/failure, eviction bounds and cold clones. Repeated
  complete draw snapshots agree with the original loader before and after a
  source mutation, including serial and parallel input counts.
- Five frozen baseline/candidate recordings preserve 74 native/scaled images,
  five complete WAVs, saves and guest instruction/VBlank/render/audio/conversion
  counters, with zero software fallbacks. The three short recordings also match
  the same-binary uncached decoder. World/battle hits are 37,318/42,502 versus
  226/349 misses; retained payloads are 33,358,996/33,081,199 bytes. The long run
  retains 33,419,521 bytes in 204 entries; restart retains 2,266,942 in 25. Intro
  draws are ineligible, with zero cache lookups/entries.
- Four balanced isolated 6× trials per executable, without captures/profiling:

  | Fixture | R-142 baseline mean | Cached mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 5.50350 s | 5.57425 s | -1.27% |
  | Overworld, 16 guest seconds | 12.01525 s | 11.18825 s | +7.39% |
  | Battle, 30 guest seconds | 19.07650 s | 18.36050 s | +3.90% |

- Same-binary uncached means are 5.5255/11.925/18.919 seconds. Separate native
  counter pairs are 100% scheduled. Retired host instructions change
  +0.0026%/-12.9574%/-9.3278% for intro/world/battle; cycles change
  +0.2488%/-10.3905%/-7.5928%. Guest work is identical. The small intro slowdown
  is recorded explicitly and receives additional balanced confirmation trials;
  this input-cache change does not accelerate movie playback. Four additional
  balanced intro trials average 5.57175 s baseline, 5.60375 s cached and
  5.59500 s same-binary uncached: a 0.57% baseline-relative slowdown, versus
  0.16% between cache policies. Retain both sets; the cache has no eligible
  intro draws, so the elapsed-time difference is not evidence of lookup cost.
  Code layout and ordinary measurement variation remain possible causes.
- Normal-speed window means are 6.005/6.005, 16.019/16.0165 and 30.018/30.0165
  seconds before/after. Resident presentation has no materialized source uses;
  saves and guest counters agree. Existing cadence still limits those runs.
  Wait for encoders and reject benchmark overlap; no external process is stopped.
  Private executables, source snapshots and results are in
  vertex-input-cache-validation. This does not enable native dynamic FPS or
  establish broader game compatibility.
- A fresh acknowledged VBlank-600–900 gameplay profile preserves native images,
  full WAV/save and guest instructions. InputPlan/element decoding falls below
  the 0.1% report threshold; cache lookup is 1.17% inclusive. Target import is
  12.03% inclusive (3.53% self), deferred plane encoding 8.35% inclusive,
  D24S8 decode/encode 2.13%/1.41% self, and vertex-input packing 2.93% self.
  These overlapping percentages select subsequent targets, not extra gains.
  Profiles remain private under steady-gameplay-profile-v4b. The first harness
  link lacked configured native-library paths; preserve that failed build and
  rerun with the normal build environment. A fresh intro profile reproduces four
  native images, WAV/save and guest count; scaled tracing is disabled in that
  profile and remains verified by the separate full replay comparisons.
- Final release `probe` and `play` text-section hashes equal the frozen measured
  candidates after the additional synthetic tests. Fresh intro exclusive samples
  still highlight the guest call wrapper (7.06%), MovieLib helpers
  0x990021c4/0x99002548/0x99002ea8 (6.72%/4.60%/4.19%), executable iterator helper
  0x004bed34 (5.14%) and several separately instantiated word writers. These
  remain candidates for narrowly scoped native/helper work; whole-function
  replacements must preserve budget exits, memory faults, registers and timing.

### R-145 2026-10-30 — Convert framebuffer targets by Morton tile row

- Follow the fresh R-144 gameplay-only profile: target import is 12.03%
  inclusive (3.53% self), deferred plane encoding 8.35% inclusive, and D24S8
  decode/encode 2.13%/1.41% self. These overlap. Extend the R-134 display-row
  approach to the separate RGBA8 and D24S8 target converters: compute one
  Morton base per eight-pixel row, then use fixed within-tile offsets.
- Preserve every color byte, depth clamp/division/quantization and stencil byte.
  Complete aligned tile shapes use the row converters. Unsupported dimensions,
  incomplete tiles and short/mismatched buffers retain the original scalar
  path and its bounds behavior. `USUM_VULKAN_TARGET_TILE_ROWS=0` restores the
  per-pixel converters; the comparison policy is captured at first use.
- Two synthetic tests compare against scalar Morton oracles across tile widths
  and native framebuffer sizes, randomized raw bytes, NaNs/subnormals/infinities,
  signed zero, rounding boundaries, supplied/preserved stencil and untouched
  trailing bytes. Ineligible shapes leave outputs untouched for scalar fallback.
  All 130 enabled runtime tests pass under both policies, all 12 Vulkan runtime
  integration tests pass, and strict Clippy/formatting pass.
- Five frozen R-144/candidate replays preserve all 74 native/scaled images,
  five WAVs, saves and guest instruction/VBlank/draw/triangle/update/audio/movie
  counters, with zero software fallbacks. Short fixtures also match the
  same-binary scalar converters. Source snapshots match the measured release
  candidate. No game-derived artifact is published.
- Four balanced isolated 6× trials per executable, without captures/profiling:

  | Fixture | R-144 baseline mean | Tile-row mean | Throughput change |
  |---|---:|---:|---:|
  | Intro, 6 guest seconds | 5.53300 s | 5.43000 s | +1.90% |
  | Overworld, 16 guest seconds | 11.20100 s | 10.81375 s | +3.58% |
  | Battle, 30 guest seconds | 18.35300 s | 17.79300 s | +3.15% |

- Same-binary scalar means are 5.5495/11.2110/18.2905 seconds. Separate fully
  scheduled native-counter pairs reduce host instructions 6.1004%/13.9382%/
  11.9336% and cycles 3.5475%/5.7125%/5.1970% for intro/world/battle. Guest work
  remains identical. Sample/instruction reductions are not rendered-FPS counts.
- Normal-speed window means are 6.005/6.0045, 16.0165/16.0155 and 30.016/30.017
  seconds before/after. Saves/counters agree, with no materialized display-source
  uses. Existing cadence still limits these runs. No regression appears in the
  tested recordings; broader scenes/adapters remain uncertain.
- Wait for encoders and reject observed benchmark overlap. A brief private
  synthetic helper experiment overlaps one trial; retain that discarded log
  and use its isolated rerun. Stop no external process. Private evidence and
  frozen executables remain in target-tile-rows-validation. Native dynamic FPS
  remains separate correctness work and is not enabled by this optimization.

### R-146 2026-10-30 — Anchor private FPS timer clocks to actual reset stores

- Locate the field component's countdown initialization, countup reset,
  conditional countdown clear and constructor-zero stores. Instrument those
  stores in the private cadence libraries, including overlapping generated
  function copies. Keep the normal generated workspace untouched by this
  experiment. The fixture-specific component address remains a research guard.
- The original turning replay executes two countdown initializations to four,
  348 countup resets to zero and 278 conditional countdown clears. Constructor
  sites are mapped but do not execute after the diagnostic starts; this fixture
  does not validate component destruction/reuse or other actors' lifecycles.
- With resetting disabled, the new instrumented control exactly reproduces
  R-143's trajectory, final native image, complete WAV and audio-source trace.
  Clearing fractional clock state at actual stores while retaining half-first
  countup behavior reproduces that control byte-for-byte too. It replaces a
  value-change inference with actual reset evidence, without claiming improved
  trajectory or audio parity in this fixture.
- Starting countup immediately instead makes mean common-checkpoint position
  error worse: 9.34 versus 8.54 game units, maximum 26.52 versus 26.37. Reject
  that policy. The matching half-first variant still has 49 nonstream source
  starts versus 47 normally, along with remaining travel/event differences.
- These are correctness experiments, not throughput/FPS measurements. No native
  FPS unlock is shipped. Source/dependency hashes, reset-site traces, controls
  and variants remain in effective-reset-clock-build/explicit-reset-clock-tests.
  Preserve the initial private harness link failure and recover it with the
  configured local native-library path; the normal game builds are unaffected.

### R-147 2026-10-30 — Reject a guarded native iterator after full-game trials

- Revisit R-084/R-085/R-098 with fresh profiles after the intervening renderer
  and memory optimizations. The accepted R-145 intro profile places executable
  helper 0x004bed34 at 6.48% self samples. Movie-module helpers, guest dispatch
  and memory access remain substantial; historical percentages are not current
  measurements. Sampling identifies work, not native frame rate.
- Implement an exact-code-fingerprint-selected tree iterator with unchanged
  guest budgets, registers, flags, stack writes and fault ordering. Prove RAM
  reads without invoking deferred providers; decline unproven, overlapping,
  wrapping, interior-entry or insufficient-budget cases to the original code.
  Retain a same-binary original-path switch. No game timing changes are involved.
- Both policies pass guest/recompiler/runtime tests and the QEMU corpus: 35
  programs in two layouts, 319,988 translated instructions, zero failures.
  A private original/native comparison also passes 14,016 full CPU/memory cases.
  Five frozen game replays preserve 74 images, five complete WAVs, starting and
  resulting saves, and guest/render/audio/movie counters. Software fallback
  counts remain zero. The three short fixtures also match the same-binary
  original path. These checks establish tested correctness, not profitability.
- Four balanced isolated 6× trials per executable regress replay throughput:

  | Fixture | R-145 baseline mean | Candidate mean | Throughput change |
  |---|---:|---:|---:|
  | Intro | 5.37550 s | 5.48525 s | -2.00% |
  | Overworld | 10.44950 s | 10.51850 s | -0.66% |
  | Battle | 17.15850 s | 17.39025 s | -1.33% |

- Same-binary original-path means are 5.4115/10.5155/17.3315 seconds. Fully
  scheduled hardware-counter pairs increase host instructions by
  0.6936%/0.0442%/0.1561% and cycles by 2.6728%/1.5037%/1.0862% for
  intro/world/battle. The smaller gameplay differences alone warrant caution,
  but no fixture establishes a benefit. Paced window means remain approximately
  6.005/16.015/30.018 seconds, with identical guest work and normal timing.
- Fresh candidate intro sampling attributes 3.52% self samples to proof reads,
  1.99% to the native traversal and 1.08% to its node-reading closure, with
  0.92% still in the generated helper. This supports investigating proof/call
  overhead; it does not isolate every source of the measured regression.
- Reject and remove the production candidate, restoring the accepted sources.
  Keep its full source snapshots, executable hashes, controls, captures and
  measurements privately in native-tree-validation, with separate baseline and
  candidate intro profiles. Component-level speedups did not survive full-game
  execution. No native dynamic-FPS feature or performance gain is claimed.
- Compile and test sequentially at one job. The temporary memory monitor never
  triggered and was removed before benchmarking. Wait for external work; stop
  no encoder. No game-derived source or captured media is published.

### R-148 2026-10-30 — Separate temporary FPS rate reads from nominal save/restore

- Trace a private higher-cadence animation state whose stored nominal rate changes
  from 1.0 to 0.5. A getter consumer saves the rate, invokes a temporary virtual
  setter, then restores the saved value. Broadly halving getter results leaks
  the effective per-update step into permanent animation state. This differs
  from deliberately scaling the time advanced by an update.
- In the frozen diagnostic harness, scope effective-rate reads to the observed
  movement consumer using its wrapper return address and saved caller. Other
  consumers receive the nominal value. Keep the previous broad policy and normal
  cadence as controls. This is a private, fixture-specific ABI proof, not a
  production engine binding or general caller/lifecycle guarantee.
- Six turning/down replays complete. Both normal-cadence and broad-policy controls
  reproduce their archived trajectories, final native image, full WAV and audio
  events exactly. In the new higher-cadence variant, all 356 noninitial reads of
  the affected state retain nominal 1.0. Previously the turning trace had 283
  reads at 0.5 after 73 at 1.0; down had 278 after 78. Normal cadence has 179
  noninitial reads at 1.0. The initial zero sample is unchanged in every case.
- Correcting that state does not resolve the remaining cadence differences.
  Turning still has 49 nonstream source starts versus 47 normally; down has 43
  versus 39. The new variant and broad control have identical final images,
  complete WAVs, audio-event traces and per-row positions. Their trajectory CSVs
  differ only in the guest-time column; do not describe those files as byte-exact.
  Stream traces remain exact against normal cadence in both scenes.
- Preserve source/dependency hashes, controls and diagnostics privately in
  effective-rate-caller-build/effective-rate-caller-tests. Treat the scoped getter
  as the corrected hypothesis for subsequent composition tests; actor timer
  scope is being tested separately against the old broad policy to isolate its
  effect. No throughput gain or released native-FPS unlock is claimed.

### R-149 2026-10-30 — Trace an unscaled idle-animation counter

- Test broader field-timer ownership separately from R-148's getter correction.
  A private controller-vtable guard still matches only the existing player
  component. The other 23 observed candidates appear in constructor stores with
  zero at the tested identity location. Turning and idle variants with this
  broader guard exactly reproduce the player-only outputs. This does not prove
  that other actors have no timers; it rejects this guard as an explanation for
  the current discrepancy.
- The idle fixture releases controls at VBlank 600. Normal and higher cadence
  have identical sampled player positions and 23 nonstream source starts / 46
  state changes. Two starts shift by one DSP frame (2075 to 2076 and 2116 to
  2117); stream traces agree. The final native images differ at 6,951 pixels.
  Matching source counts is not evidence of exact audio or animation timing.
- Actor snapshots isolate a different integer field at component offset 0x48:

  | Engine frame | Normal counter | Higher-cadence counter |
  |---|---:|---:|
  | 700 | 37 | 76 |
  | 800 | 87 | 176 |
  | 900 | 137 | 276 |

- The higher-cadence counter reaches 324 against a threshold of 323 at frame
  948, when animation 0 changes to 0x30. Normally the counter is only 161 then
  and animation 0 continues. The increment is present in two overlapping
  generated copies of block 0x6f051110. Initialization clears this field and
  chooses a threshold from a random result plus 300.
- Threshold values also differ: 307 normally versus 323 at higher cadence in
  this recording. Correcting the counter rate alone therefore cannot establish
  identical random choices or event timing. Preserve that uncertainty rather
  than attributing every observed difference to one clock.
- Prepare an opt-in private half-step counter with reset tracking, original-path
  controls and a composition with R-148's nominal-rate correction. Queue both
  short comparisons and longer idle replays that can observe expiration after
  the corrected wait. Those experiments are not yet validation results here.
  The completed attribution remains in field-timer-scope-tests and its offline
  reports; no production FPS feature or throughput gain is claimed.

### R-150 2026-10-30 — Borrow immutable vertex input bytes for Vulkan uploads

- Remove the temporary vertex-input allocation and CPU packing copy. A safe
  bytemuck view preserves the native-endian bits of the padding-free input arrays;
  the existing uploader copies them before the borrow ends. GPU upload ownership
  and submission lifetimes are unchanged. `USUM_VULKAN_BORROW_INPUTS=0` keeps the
  previous block writer, and `USUM_VULKAN_BLOCK_PACK=0` still forces the scalar
  writer. No guest timing, instructions or shader behavior changes.
- The preceding steady-gameplay profile attributed 2.94% self samples to input
  packing. That profile used the subsequently rejected R-147 native-helper build;
  packing itself was unchanged. Regenerate and rebuild from the accepted emitter
  for this experiment, explicitly checking that the rejected helper is absent.
  Freeze sources, executables, dependency hashes, input recordings and initial saves.
- Five 6× Vulkan replay fixtures cover intro, overworld, battle, a longer gameplay
  recording and the restart/Mystery Gift recording. All 74 captured images, five
  complete WAVs, resulting save files and guest/render/audio/movie counters match
  the previous binary exactly, with zero software fallbacks. The shorter fixtures
  also match the same-binary reference path. This validates the recordings, not
  every possible menu transition or save.
- Four balanced unpaced trials per binary measure the following means. Retain all
  trials, including the slow intro sample; these are replay-throughput results,
  not native dynamic FPS or projections from guest-clock speed.

  | Fixture | Previous binary | Borrowed inputs | Throughput change |
  |---|---:|---:|---:|
  | Intro | 5.34725 s | 5.44400 s | -1.78% |
  | Overworld | 10.51075 s | 10.32000 s | +1.85% |
  | Battle | 17.33000 s | 17.17750 s | +0.89% |

- Same-binary reference means for overworld/battle are 10.439/17.245 seconds,
  corresponding to +1.15%/+0.39% versus borrowing. Two fully scheduled hardware
  counter pairs reduce host instructions by 1.3954%/0.9302% and cycles by
  2.2083%/0.9524%, supporting the small gameplay gains independently of wall time.
- Intro is a limitation, not a claimed improvement. The initial candidate set
  includes a 5.777-second run. Twelve additional balanced trials, four per policy,
  give previous/borrowed/same-binary-reference means of 5.34975/5.40175/5.44625
  seconds; the reference set includes a 5.763-second run. Thus the separate
  confirmation is 0.96% slower than the previous binary, while same-binary means
  swing in both directions across sets. Keep both full sets and their spreads.
  Intro host instructions change by +0.00004% and cycles by -1.3909%; these do not
  explain the wall-time regression. Before/after memory-pressure snapshots also
  do not isolate its cause. Accept the narrow gameplay improvement with this
  unresolved intro cost documented and the comparison path retained.
- Paced window pairs preserve guest work and remain at normal game timing:
  previous/borrowed means are 6.0040/6.0045, 16.015/16.014 and 30.0185/30.0155
  seconds. These paced measurements cannot establish an uncapped FPS gain.
- Validation passes: 21 renderer tests in both policies, 122 enabled runtime
  tests, 10 renderer and 12 runtime Vulkan hardware tests in both policies,
  formatting and strict window/Vulkan Clippy. Direct hardware vertex checks pass
  in borrowed/block/scalar modes, including float bit patterns, indexed raster
  identity and sparse output/UV mappings. Unit checks verify borrowed pointer
  identity, fallback ownership, random float bits and offset-containing arrays.
- Builds and heavyweight checks run sequentially with one compiler job; no encoder
  is stopped. Preserve detailed private evidence in borrow-inputs-validation.
  Native engine cadence remains separate correctness research; this change does
  not ship or claim the dynamic-FPS feature.

### R-151 2026-10-30 — Validate idle clocks and isolate higher-cadence random checks

- Complete R-149's private idle-counter experiment: eight short replays and four
  longer idle replays. Normal-cadence and disabled controls reproduce the archived
  outputs; each longer run also matches its short WAV/trajectory prefix and
  VBlank-900 image. Track both increment copies and all observed reset stores.
  No fixture-specific hook is promoted into the production runtime.
- The corrected higher-cadence idle counter is 38/88/138 at engine frames
  700/800/900, versus the old 76/176/276 and normal 37/87/137. It now advances at
  the right rate, with a remaining initialization offset. In the longer replay,
  the first idle variation moves from frame 948 to 1272; normally it starts at
  1242. The selected threshold is still 323 versus 307, and the selected animation
  is still different. Correct rate alone does not establish correct event timing.
- Read-only animation entry/exit traces preserve all prior outputs. The player
  reaches phase 15 at frame 622 in both modes. Higher cadence sets the completion
  flag at 623 and begins the next clip at 624; normal cadence sets it at 624 and
  begins the next clip at 626. The nonlooping update clamps and finishes when
  the newly advanced phase exceeds the endpoint. Both endpoint handling and
  when the state machine consumes completion matter; merely halving the step
  leaves this boundary discrepancy.
- Full random-generator snapshots agree before the cadence request. At the first
  differing shared observation, VBlank 604, generator words still agree but the
  indices are 54 normally and 76 at higher cadence. Probe callbacks occur at
  instruction-budget checkpoints, not every VBlank, so these observations are
  neither complete per-frame traces nor counts of random calls.
- Instrument actual entries to the resolved bounded-random helper, including
  direct generated calls. Both modes retain exact trajectories, audio, images,
  phase traces and complete generator snapshots against their uninstrumented
  controls. Across engine frames 600–650 there are 157 normal entries and 344
  higher-cadence entries. Caller 0x00420824 with bound 128 accounts for 153 versus
  340. The two bound-3 calls and the idle threshold/choice calls account for the
  remaining four in each mode. These are observed helper entries, not inferred
  from generator indices or presented as a universal RNG inventory.
- That dominant caller belongs to a small actor state machine with a countdown,
  discrete states and a random callback. Its parent applies changed state to the
  actor separately. Prepare a private fixed-time test of this state machine,
  retaining higher-cadence pose evaluation and original/normal controls. The new
  clock is queued after controlled GPU benchmarks; it has no result yet here.
- Also validate read-only traces after completed movement-position setters.
  All four turning/down controls preserve the prior outputs and step traces;
  every recorded setter stores its target and charges its original 11 guest
  instructions. In frames 640–679, summed committed movement distances are
  379.9961/380.0149 for normal/higher down movement and 380.0006/379.9994 for
  turning. Thus this steady segment does not move twice as far. Unshifted
  matched-frame position differences still reach 9.5006 and 10.3654 units;
  shifting comparison frames is diagnostic only and does not fix them.
- Preserve sources, hashes and controls in the private idle-timer-clock,
  idle-timer-long, phase-boundary-trace, rng-stream-trace, rng-call-trace and
  position-commit-trace experiment directories. Position writes and animation
  updates are not completed rendered frames; no FPS or throughput claim follows
  from their counts. Native dynamic FPS remains correctness research.

### R-152 2026-10-30 — Align a higher-cadence idle transition with logical time

- Continue only native dynamic-FPS correctness work. The private actor-state
  clock from R-151 restores 157 bounded-random helper entries in engine frames
  600–650, versus 344 without it. Counts alone hide a remaining ordering error:
  its first 71 entry tuples match normal cadence, then the player selects an
  idle threshold at frame 624 instead of 626. The threshold becomes 384 rather
  than 307. Compare actual helper entries, not instruction-budget checkpoint
  snapshots, which can observe different points within the same update.
- Test completion publication separately, retaining higher-cadence animation
  evaluation. A private player-only positive, nonlooping-animation gate holds
  the finished flag until the next logical transition can consume it at the
  normal tick. This diagnostic is tied to the existing fixture and uses a
  per-state pending deadline. It is not a production binding or a general
  solution for recycled objects, resets, reverse playback, or arbitrary rates.
- Three 16-second replays complete. Normal cadence and the disabled completion
  gate reproduce the previous trajectories, full WAVs, final images, phase
  traces, random-call traces and generator snapshots exactly. The enabled
  variant retains the same stationary player positions.
- With both private clocks enabled, all 157 recorded helper-entry tuples match
  normal cadence: engine frame, caller, bound, generator identity/index and
  observed owner. The idle draws now occur at frame 626, indices 184 and 186.
  The threshold is 307; counter samples at frames 700/800/900 are 37/87/137,
  matching normal cadence instead of the earlier 38/88/138.
- The initial idle animation now starts at frame 626 in both modes. This is
  evidence for separating logical transition timing from pose evaluation, not
  proof of equivalent animation state: normal completion publishes at 624,
  whereas the diagnostic publishes at 625. The new idle pose starts at 0.5
  instead of 1.0, and full audio and final images still differ from normal.
  Further lifecycle, first-update and event-timing work is required.
- Extend all three variants to 30 guest seconds. Each preserves its shorter
  replay's image, bounded phase/RNG traces, trajectory/event prefixes and full
  WAV prefix exactly. The corrected variant selects the same idle variation
  (0x2f) as normal and matches every observed animation transition at frames
  600, 626, 1242 and 1444. The disabled completion control instead transitions
  at 600, 624, 1394 and 1595. All three have 23 nonstream source starts; equal
  counts do not resolve the existing audio timing differences.
- Preserve sources, hashes, controls and actual-entry analysis privately in
  completion-clock-build/tests, completion-long-build/tests and
  chance-clock-build/tests. A 3 GiB compiler
  cap stopped the first dispatch-table rebuild; retry reused the completed
  animation unit, compiled the dispatch table without optimization, and ran
  under a 6 GiB cap with no swap and one CPU quota. Normal/disabled output
  checks passed. Another project was compiling concurrently: these are
  correctness experiments, with no wall-time performance or FPS claim.

### R-153 2026-10-30 — Follow clip resets and preserve the initial event interval

- Investigate R-152's remaining half-step difference at the first idle pose.
  Read-only entry/exit instrumentation of the suspected binding routine observes
  no calls in either short replay; both controls retain all prior outputs.
  Allocation/binding alone therefore cannot identify these clip transitions.
- Trace both generated copies of the reset routine and explicit rate, phase,
  bounds and loop setters. Both normal and higher-cadence outputs remain exact.
  At engine frame 626, the observed reset reuses an existing animation state:
  phase/previous change from 3/2 normally (3.5/3 at higher cadence) to 0/0, and
  the endpoint changes from 20 to 85. The nominal rate remains 1. This is an
  actual clip-reset observation, not an inference from reaching a loop boundary.
- Test a private reset-driven first-step credit for the player's looping clips.
  Consume it once, only in the reset's frame with matching state fields; clear
  it on observed resets and explicit phase/bounds/loop changes. Normal and
  disabled controls reproduce the archived outputs exactly. One credit is
  consumed at frame 626: the new clip advances by 1 instead of 0.5, then by
  half-steps. Phases at shared ticks 626 and 628 are now 1 and 2, matching normal;
  the extra update at 627 evaluates phase 1.5. All 157 bounded RNG-entry tuples
  still match normal cadence.
- Reject the pose-only change as sufficient: at the initial event check,
  normal cadence passes interval [0, 1], but the candidate passes [1, 1]. The
  event routine compares its stored previous phase plus expected step with the
  current phase. It still expects 0.5 after the pose advances by 1, classifies
  the mismatch as a discontinuity, and replaces the interval start with its end.
  Correct pose samples alone can therefore hide lost animation events.
- The paired experiment communicates the actually consumed first step to that
  discontinuity check, retaining the normal threshold and stored nominal rate.
  Three short replays pass; normal and disabled controls remain exact. The
  corrected interval is [0, 1], shared-tick phases remain correct, and all 157
  recorded RNG-entry tuples still match normal. Audio and final images are
  still not equivalent to normal cadence.
- Broaden to six turning/straight-down replays using the same starting save.
  Both normal runs exactly match the archived normal trajectories, WAVs, audio
  events and images. The event bridge activates twice during turning and once
  during straight movement. Higher cadence still produces 49 versus 47
  nonstream sound starts when turning, and 43 versus 39 going straight. Action
  transitions differ by up to two engine frames in these recordings. These
  results prevent promoting the stationary-scene correction as generally valid.
- The new lifecycle traces expose a missing reset path: straight movement at
  frame 620 resets the existing walking clip but does not take the traced tail
  exit. Consequently the private first-step credit is absent, and the walking
  pose stays half a step behind on shared ticks. Inspect the routine's other
  return before extending the policy. Turning also has odd-tick action changes
  and explicit phase transfers that require separate timing treatment.
- Preserve sources, dependency hashes and outputs privately under animation-
  lifecycle, animation-reset-trace, animation-first-step, animation-first-event
  and first-event-movement experiment directories. These are fixture-specific
  correctness experiments, not performance trials or a playable native-FPS
  feature; lifetime safety and arbitrary-rate behavior remain unproven.

### R-154 2026-10-30 — Cover the completed reset return used by walking

- Inspect the reset routine's alternate return in both generated copies. It
  writes current and previous phase before branching, so this path completes a
  reset even when it does not rebind a different clip. Add a private opt-in
  first-step credit at that return, retaining the existing identity, frame,
  phase, loop and rate guards and the paired event-step calculation.
- Three short idle replays and six turning/down replays complete. Normal paths
  reproduce archived outputs; disabled-feature paths reproduce the previous
  candidate, including trajectories, full WAVs, audio events, images, phase/RNG
  traces and first-step/event logs. The additional read-only return trace is
  compared after removing its newly added rows. The enabled idle outputs are
  unchanged from the preceding candidate.
- Straight walking now receives the missing first-step credit at engine frame
  620. The pose advances to 1, the event interval is [0, 1], and subsequent
  shared-tick phases are 2, 3, 4, and so on, matching normal cadence until later
  action changes. All 157 bounded RNG-entry tuples still match normal. Turning
  does not activate an additional credit and retains the previous outputs.
- Some straight-movement audio timings improve: observed source starts at DSP
  frames 2137 and 2171 now match normal instead of 2141 and 2175. Later starts
  and sample choices still differ. Nonstream source-start counts remain 43
  versus 39 normally for down movement, and 49 versus 47 for turning. Action
  transition frames are unchanged, including the still-early special action.
  These remaining differences prevent treating this as a complete motion fix.
- The frame-620 movement setter records zero translation in all variants;
  matching its animation phase does not prove that movement needs the same
  exceptional first-step adjustment. Investigate consumers from their actual
  call timing and arguments instead of broadly changing nominal-rate getters.
- Extend all three idle variants to 30 guest seconds. Each preserves its short
  image, bounded phase/RNG traces, trajectory/event prefixes and full WAV prefix.
  Normal, disabled and enabled variants select the same observed idle variation
  and transition at frames 600, 626, 1242 and 1444. Each has 23 nonstream source
  starts; matching counts and transition frames do not erase the remaining
  audio or pose differences.
- Preserve the private animation-reset-return, reset-return-movement and
  reset-return-long sources, hashes and captures. Queue read-only field
  action-entry traces to locate the remaining early changes before altering
  their scheduling. These results do not enable interactive native FPS.

### R-155 2026-10-30 — Separate action requests from selector decisions

- Complete six read-only action-entry replays: idle, turning and straight-down
  movement at normal cadence and with the private higher-cadence corrections.
  Every run reproduces its preceding trajectory, full WAV, audio events, image,
  phase/RNG traces and lifecycle logs exactly. The instrumentation is placed
  after successful budget charging so a resumed block cannot log a false call.
- Turning makes the same seven action applications, in the same order through
  the same caller, at both cadences. Several higher-cadence applications occur
  one or two engine ticks earlier. Straight movement makes the same four
  applications; its special-action change occurs at 684 instead of 686. Merely
  suppressing changes on odd ticks would not account for that difference.
- Trace the virtual selection call and its return before action application.
  All six additional replays preserve the previous outputs and action-entry
  traces exactly. Each normal trace has 76 paired selections in the observed
  window; each higher-cadence trace has 150. Every observed call uses the same
  selector. The idle transition remains aligned at 626.
- The straight-movement caller continues requesting the same movement category
  when the selector changes its result early. Turning likewise retains that
  category across its intermediate action changes. This narrows the next
  investigation to selector inputs and branches, rather than assuming the
  application routine itself introduces the timing difference.
- The traced action-component bytes do not change across selection, but this
  does not establish a pure query: source inspection finds a component lookup,
  memory writes and calls to other routines. The subsequent branch trace
  resolves that lookup to the same action component, correcting the initial
  assumption that it referred to a separate object. Compare actual branches
  before skipping calls or deferring their results. Some one-tick changes
  may reflect earlier input sampling; do not classify every cadence-dependent
  transition as a bug without identifying its cause.
- Preserve the private action-transition-trace and action-decision-trace source
  snapshots, dependency hashes and captures. These are correctness diagnostics,
  not performance measurements or a production native-FPS implementation.

### R-156 2026-10-30 — Clock the two-count action flag and test interruption

- Complete six selector-branch traces with exact preceding outputs. Resolve
  the queried component to the already observed action component. The early
  special-action decision reads its byte flag, which is produced by a separate
  two-count conditional timer. That timer still increments once per update,
  reaches two, and saturates; its predicate's broader meaning is not established.
- In straight movement, the normal timer is zero at 682, one at 684 and two at
  686. Higher cadence reaches one at 683 and two at 684. Turning similarly
  reaches two at 736 instead of 738. The selector consumes that flag to choose
  the observed special action. This is direct evidence of a clock-rate error,
  distinct from simply sampling a new input on an earlier native update.
- Add a private opt-in fractional clock for this timer, retaining the original
  path. Begin with a half-step at higher cadence, and clear fractional state at
  the actual constructor, conditional reset, explicit reset and saturation
  stores, including both overlapping generated copies. Saturated updates retain
  the original increment/clamp sequence. Keep the fixture and cadence guards;
  this is not yet a general actor-lifecycle or arbitrary-rate implementation.
- Nine idle/turning/down replays complete using identical starting saves and
  inputs. All normal and disabled controls reproduce the preceding captures
  and traces exactly. Enabled idle output is also unchanged. The corrected
  special-action changes now occur at 686 going straight and 738 while turning,
  matching normal cadence. Other turning action changes remain earlier.
- Both movement cases retain their preceding bounded RNG, phase-boundary and
  first-step/event traces. Straight movement still matches all 157 normal RNG
  tuples; turning retains its existing differences among 161 calls. Final
  captured images are unchanged from the disabled higher-cadence control.
  Turning's full WAV and audio-source trace are unchanged too. Straight
  movement's source start at DSP frame 2346 now occurs at 2343 with the normal
  sample choice; normal is 2342. Remaining sample/timing differences persist,
  and source-start counts remain 49 versus 47 normally for turning and 43 versus
  39 going straight. A changed budget-sampled trajectory CSV alone does not
  establish a change in actual travel distance.
- Add an interrupted-and-resumed movement recording. Five replays include old
  normal/higher binaries, new normal/disabled controls and the enabled clock.
  Normal and disabled captures match their old binaries exactly; starting-save
  hashes remain unchanged. The enabled trace clears a pending half-step at 736
  while the guest counter is still zero, then begins the next eligible update
  at 746 with a fresh half-step. Thus this exercised zero-to-zero reset does
  discard fractional progress rather than carrying it into the next action.
- Preserve private action-selector-path-trace, action-flag-clock-trace and
  action-flag-reset evidence. These are timing-correctness results without a
  throughput claim. Interactive native FPS, other actors and lifecycles,
  arbitrary cadence, battles, movies and transitions still require validation.

### R-157 2026-10-30 — Trace direction-driven turning before imposing delays

- Complete six read-only direction/selector traces with exact preceding outputs
  at normal and corrected higher cadence. Capture the actual direction
  comparisons before the turning flag and counter, plus complete bounded RNG
  state snapshots before and after selection. All 141 paired selections leave
  that RNG state unchanged in the observed window; this does not prove that
  selection has no other side effects in every scene.
- The normal turning direction comparison remains negative at 628
  (-0.0040339548) and becomes positive at 630 (0.0081022121). Higher cadence
  records corresponding values at 627 (-0.0040339604) and 629 (0.0081022084),
  with an intermediate positive value at 628 (0.0034007765). The turning flag
  consequently clears at 628 instead of 630. The earlier movement request was
  sampled at 619 instead of 620. These observations support earlier input and
  intermediate direction updates, rather than an unscaled one-update timer,
  as the cause of this remaining transition difference.
- Do not add a minimum action lifetime or force action publication onto even
  ticks solely to reproduce normal capture timing. The preceding two-count
  flag correction had direct timer-rate evidence; this transition needs a
  different interpretation. Overall motion correctness remains unproven.
- Turning still makes 161 observed bounded RNG calls. Earlier action
  applications shift calls relative to NPC calls, so equal counts do not mean
  identical draw assignment, even after ignoring timestamps. Broader audio
  differences remain to be separated from changed random choices and actual
  timing errors. Preserve the private action-geometry-trace evidence.

### R-158 2026-10-30 — Move fractional timer arithmetic into the guest library

- Add `usum_guest::timing::TickAccumulator` as core, game-data-free arithmetic.
  It converts explicit elapsed guest-time units to whole logical ticks while
  retaining the fraction. Its fixed scale is nonzero; quotient/remainder
  splitting avoids overflow even for the largest elapsed value. Explicit clear
  handles a reset whose guest counter remains zero. The caller remains
  responsible for elapsed-time sourcing, timer eligibility and object lifetime.
- All 40 guest-library unit tests pass, including new checks for zero-to-zero
  reset, changing intervals, zero elapsed time, large-value overflow and cadence
  partitions from 24 to 240 updates per second. These establish arithmetic
  properties; they do not establish game correctness at those frame rates.
- Replace the private field timer helper's manual fractional arithmetic with
  this public implementation in all four existing timer domains. Preserve its
  prior first-step, reset, expected-value, frame-gap and fixture policies. For
  bounded-memory validation, compile the identical public source as a small
  standalone dependency of the instrumented module, retaining frozen guest and
  runtime libraries rather than recompiling every translated unit.
- All nine integration controls reproduce their previous outputs exactly:
  idle/turning/down at normal and corrected higher cadence, plus interrupted
  movement at normal, disabled and enabled settings. Comparisons include full
  WAVs, captured images, trajectory/audio traces, RNG snapshots and the existing
  timer, selector, geometry and reset traces. Source and dependency hashes are
  retained in native-clock-core-trace evidence.
- This is a tested core building block used by the private native-FPS path, not
  an interactive FPS unlock. Object lifecycle policies, non-fixture identities,
  arbitrary-rate game scheduling, native rendered-frame measurement and broader
  battle/movie/transition validation remain unfinished. No performance gain is
  inferred from these correctness checks.

### R-159 2026-10-30 — Resolve the animation event consumer's live owner

- Complete six read-only ownership traces: idle, turning and straight movement
  at normal and corrected higher cadence. All preceding captures and traces
  remain exact, including full audio, images and RNG snapshots. The bounded
  window observes one callback owner, so it does not establish NPC coverage.
- Distinguish the animation state's bound resource from its actual owner. The
  event controller references a model, which references the animation owner;
  the owner's bank, selection flag and parameter determine the queried state.
  The phase query returns the first valid channel's result, not the maximum of
  all channels. A positive observed parameter must not be assumed to be a
  normalized blend weight.
- All five observed first-step event credits resolve to the selected primary
  state: idle at engine frame 626, turning at 718 and 790, and straight movement
  at 620 and 732. This supports checking the live ownership chain and the phase
  actually returned by the query instead of relying on fixed controller/model
  identities. It does not justify extending first-step eligibility to every
  animation or actor.
- Implement the opt-in private owner-bound consumer after the actual phase
  query, retaining the original pre-query fixed-identity path for comparison.
  Require the selected primary state to own the pending credit and its current
  phase to equal the query result, alongside the existing frame, rate, bounds,
  previous-phase and floating-point-mode checks. First-step eligibility remains
  restricted to the existing player fixture.
- All twelve replay controls pass: idle/turning/down at normal cadence with the
  new flag off/on, and corrected higher cadence with it off/on. Every preceding
  output matches exactly, including full WAVs, images, RNG snapshots and owner
  traces. The enabled higher-cadence path consumes all five expected credits;
  the other modes consume none through that new path.
- Compile the actual handwritten consumer helpers into a small synthetic test.
  It accepts valid non-fixture controller/model addresses, consumes a credit
  only once, follows alternate-bank selection, and rejects another state at
  the same phase, a mismatched queried phase, a stale frame, an invalid bank,
  an empty channel and a missing model. This validates guard behavior without
  claiming that the game produces credits correctly for other actors.
- Preserve private animation-owner-trace and owned-event-trace evidence plus
  guard checks. Singleton credit storage, reset/rebinding invalidation, other
  actors, arbitrary game cadence and interactive integration remain open.
  No performance improvement is inferred from these correctness checks.

### R-160 2026-10-30 — Keep event credits per animation state

- Add an opt-in private event-credit registry keyed by the live selected
  animation state. Publishing in a new guest frame removes older credits;
  consuming one state's credit leaves other states intact. Retain the original
  singleton path for comparison and keep first-step eligibility restricted to
  the existing player fixture.
- Exercise reset/rebinding invalidation separately from event consumption. The
  first candidate incorrectly cleared a pending first-pose step on a routine
  same-rate assignment after reset. Idle replay lost its frame-626 credit and
  diverged in poses, RNG snapshots and the final image. Reject that candidate.
  The corrected version preserves the pending pose step through that assignment
  while retaining the existing value checks and explicit reset/binding clears.
- All twelve corrected controls pass: idle, turning and straight movement at
  normal cadence with the registry off/on and corrected higher cadence with it
  off/on. An independent audit compares every captured CSV, WAV, PPM and RNG
  binary, including owner traces initially excluded from the worker's strict
  comparison. All are exact. The audit also verifies source-save, recorded-input,
  case-binary and manifest hashes against their recorded references.
- Actual-helper synthetic checks cover independent credits for two states,
  single-use consumption, stale-frame removal, invalidating one state without
  clearing another, and the producer-to-consumer path across the routine rate
  assignment. These establish metadata behavior, not real multi-actor gameplay
  coverage. Preserve failed event-registry-trace and corrected event-registry-v2
  evidence rather than overwriting the failed run.
- This removes singleton storage as a prerequisite for broader ownership work.
  It does not complete lifecycle policy: simple rate/bounds/loop/phase writers
  must be distinguished from actual reset and binding operations, including
  writes that leave values unchanged. Other actors, arbitrary cadence and
  battle/movie/transition behavior remain unvalidated. No throughput claim is
  made from these bounded correctness runs.

### R-161 2026-10-30 — Move per-state event credits into the guest library

- Add `usum_guest::timing::FrameCredits<K, V>` as game-data-free core storage.
  Credits belong to a caller-supplied guest frame and state identity. Lookup and
  consumption require the publication frame; consuming or invalidating one
  state preserves other states. Publishing in a different frame discards older
  entries, and explicit clear handles session replacement. Callers remain
  responsible for validating live ownership and invalidating lifecycle changes.
- All seven standalone timing-module tests pass. New coverage includes
  interleaved states, single consumption, explicit same-value reset/address
  reuse, frame changes, rewind on publication and session clear. The existing
  fractional-timer arithmetic tests remain passing. These tests exercise core
  metadata and arithmetic, not complete game scheduling or actor lifetimes.
- Replace the private registry's hand-written map handling with this exact
  public source, preserving its eligibility, reset policy and singleton
  comparison path. Compile an identical source snapshot as a small standalone
  dependency to retain the frozen translated units and bounded-memory workflow.
  The integrated actual-helper guard checks also pass.
- All nine extraction controls are exact: idle/turning/straight movement at
  normal cadence, corrected higher cadence with the registry disabled, and
  corrected higher cadence with it enabled. Compare every existing CSV, WAV,
  PPM and RNG binary, including owner traces. Independently verify source-save,
  input, case-binary, harness and manifest hashes; the public source hash equals
  the compiled snapshot's hash. Preserve event-credit-core evidence.
- This core component is now used by the private native-FPS experiment. It does
  not enable interactive native FPS, broaden actor eligibility, or settle the
  field-writer/reset distinction noted in R-160. Arbitrary cadence and broader
  gameplay validation remain necessary; these runs do not measure throughput.

### R-162 2026-10-30 — Separate reset ownership from allocation context

- Complete six read-only reset-owner traces at normal and corrected higher
  cadence for idle, turning and straight movement. Extend phase tracing from
  engine frames 610–632 to 600–800 and capture reset-call descriptors plus a
  larger owner snapshot. All previous captures remain exact; the original
  phase-window rows also match exactly.
- At audited reset callers, the descriptor identifies an animation owner and
  layer. The two looping states reset at idle frame 626 belong to different
  owners' primary banks. One advances to phase 1 at both cadences through the
  existing correction; the other advances to 1 normally and 0.5 at higher
  cadence, then remains half a phase behind at shared update points.
- Their owners share a pointer used by the state-allocation path. This is an
  allocation context, not sufficient evidence of shared actor identity. Do not
  broaden eligibility solely on that pointer. Other candidate resets at frame
  668 belong to non-primary channels and have no updates in this particular
  phase-updater trace, reinforcing the need to identify actual consumers.
- Separately reproduce a credit-lifetime counterexample using the actual
  translated rate setter and synthetic guest objects. A same-rate assignment
  after a credited pose leaves the guest state bytes unchanged, but the current
  invalidation policy discards the credit before its event query. The baseline
  fails the expected-credit assertion; a candidate preserving the credit only
  when the stored and assigned rates match passes. Changed rates still clear
  that state's credit, other states survive, and the before-pose case passes.
- This counterexample is a targeted instruction/helper test, not an observed
  failure in the recorded gameplay runs. Preserve reset-owner-trace and
  rate-assignment-guard evidence. Full replay validation of the opt-in rate
  policy is separate; actor eligibility remains unchanged.

### R-163 2026-10-30 — Preserve event credit across an unchanged rate write

- Implement the rate-assignment correction as a separate opt-in private policy.
  When the current frame's applied credit records the same rate as the guest
  state after the setter, preserve it. A changed rate still invalidates that
  state's credit; reset/binding operations and first-step eligibility retain
  their preceding behavior. The original policy remains selectable.
- Compile the actual rate setter with the modified helpers. With the flag off,
  the same-rate-after-pose test reproduces the expected missing-credit failure.
  With it on, that test and changed-rate, independent-state, before-pose and
  single-consumption checks pass. An assignment that leaves guest state
  unchanged therefore no longer destroys the credited event interval.
- All nine full replay controls pass exactly: idle, turning and straight
  movement at normal cadence with the new flag on, and corrected higher cadence
  with it off/on. Every preceding CSV, WAV, PPM and RNG binary matches. Verify
  current source saves, recorded inputs, case binaries, harness and manifest
  against their recorded hashes independently.
- Preserve rate-preserving evidence. The replay cases establish compatibility;
  the targeted instruction test establishes the corrected ordering case. Do not
  infer that the latter ordering occurred in those gameplay recordings or that
  this produces a measurable speedup. Other field writers, non-fixture actor
  binding, arbitrary cadence and broader gameplay validation remain open.

### R-164 2026-10-30 — Bind first-step timing to live player model components

- Complete six read-only reset-context controls without changing any preceding
  CSV, WAV, PPM or RNG binary. The audited caller walks fourteen inline model
  components, then the main model. Captured caller context agrees with the live
  player pointer chain in all three recordings at both cadences. This establishes
  actual model composition independently of the allocation context in R-162.
- Add a private opt-in producer policy that resolves the live player model and
  its component animation owners. Only primary-bank layer zero with a matching
  live resource is eligible. Record the root/model/owner binding when arming a
  first step, and revalidate it before consumption. Keep the prior resource-only
  policy selectable. Synthetic helper checks cover independent main/component
  credits, missing banks, wrong resources, unrelated states, rebinding, reset,
  frame changes and invalid pointer chains.
- Complete nine replay comparisons. All six normal-cadence and old-policy
  controls remain exact. Independently verify all nine input, starting-save,
  binary, harness and manifest records. In the enabled idle case, the previously
  lagging component matches 92/92 shared phase samples instead of 4/92. In
  straight movement, two component states improve from 12/64 to 49/64 and from
  12/61 to 47/61 matching shared phase samples; their changed samples match.
- Turning illustrates why shared timestamps alone are insufficient. At frame
  790 the higher-cadence path resets both main and component models; normal
  cadence resets them at 792. The correction synchronizes both models from
  phases 1 through 6 at frames 790–800, removing their former half-phase offset.
  It increases the component's difference from normal at five shared timestamps
  because the action itself starts earlier. That action-start difference remains
  unresolved by this change and is not evidence of arbitrary-cadence correctness.
- All three enabled cases retain the prior higher-cadence position samples,
  audio captures/events and RNG state binaries. Changed trajectory/RNG CSV
  fields are timestamps only. Some trace VBlank boundaries and instruction
  budgets change; rendered poses change with the corrected component phase.
  Preserve component-step evidence and comparisons rather than claiming byte
  identity for the new policy or a throughput improvement.
- This removes a fixed resource address from this eligibility decision, not the
  experiment's remaining fixture gates, half-step arithmetic or normal-launch
  integration gap. Freely varying native cadence remains the required feature.

### R-165 2026-10-30 — Use elapsed-time arithmetic for rotation smoothing

- Add core `blend_for_elapsed` arithmetic. Convert a nominal exponential blend
  to `1 - (1 - blend)^elapsed_ticks`, using stable logarithm/exponential forms
  for arbitrary elapsed intervals. Preserve the audited half-tick calculation
  and exact zero/one-tick cases. Reject unsupported inputs instead of silently
  clamping guest behavior. The caller supplies elapsed guest time and retains
  responsibility for moving targets, events and floating-point guest semantics.
- All ten standalone timing-module tests pass. New tests cover partitioning a
  constant-target decay across changing intervals, zero elapsed time, small
  blend factors, nominal/half-tick identity, invalid values and step sizes
  corresponding to 24–240 Hz. These prove arithmetic properties, not full-game
  behavior at those rates.
- Compile an identical public-source snapshot as a small dependency and use it
  in the private rotation consumer, retaining the existing half-tick input and
  guest float conversion. Reuse the preceding stable frozen game dependencies;
  this comparison is independent of R-164's new component policy.
- All six idle/turning/straight-movement controls at normal and higher cadence
  preserve every preceding CSV, WAV, PPM and RNG binary. Independently verify
  all six starting saves, input recordings, binaries, harness and manifest
  hashes, and the identity of the public source and compiled snapshot. Preserve
  elapsed-blend evidence and the original harness for comparison.
- The smoothing calculation now supports varying elapsed intervals, but the
  existing consumer still supplies a half tick. Animation increments, integer
  timers and engine scheduling must share elapsed guest time before freely
  varying native gameplay cadence is available. Hardware VBlank/audio timing
  must remain independent of presentation and native update rates. No FPS
  throughput result or complete dynamic-FPS feature is claimed here.

### R-166 2026-10-30 — Locate the hardware-frame wait that constrains native cadence

- Trace the engine manager, frame-counter getter and sleep helper over engine
  frames 600–630 using the established idle recording. Both normal and corrected
  higher-cadence controls preserve every previous capture. The normal path has
  16 manager entries, 176 pre-wait counter reads and 160 sleeps of 1,000,000 ns.
  The higher-cadence path has 30 entries; its 11 pre-wait reads and 10 sleeps
  occur at the initial transition rather than on every subsequent update.
- Both paths still query the frame counter after their render preparation. The
  getter selects shared counters rather than measuring elapsed update time.
  Inspecting the following call chain identifies a separate counter-change wait
  reached through the engine's graphics synchronization path.
- Extend the read-only trace to that wait. It is called 16 times normally and
  30 times at higher cadence, each with the same counter selector. Both extended
  controls preserve all preceding captures. Independently verify starting saves,
  input recordings, binaries, harness and build manifests for both trace pairs.
- This establishes a remaining hardware-frame scheduling dependency even after
  removing the normal path's extra sleep loop. Freely varying native updates
  need their own deadlines and a shared elapsed-time step, while preserving
  hardware interrupt/audio clocks and required graphics synchronization. Merely
  changing half-step factors cannot overcome this dependency; blindly removing
  the wait would also discard its synchronization behavior.
- Preserve scheduler-trace and scheduler-wait evidence. These are read-only
  scheduling observations, not a new native frame-rate mode or speed benchmark.

### R-167 2026-10-30 — Connect native timing hooks to the runtime guest clock

- Add an opt-in runtime clock context around translated dispatch. A native hook
  reads guest nanoseconds using its current instruction budget. Virtual reads
  include work since the dispatch origin with the kernel's integer cycle/time
  conversion; live reads use the kernel's monotonic origin. Reading the context
  never advances time or changes hardware events. No context is available
  outside an enabled dispatch.
- Scope the context to the host thread and restore it across nested runs,
  disabled sessions and unwinding. Refresh it after each dispatch so SVC waits,
  budget refills and thread switches cannot retain an earlier clock origin.
  Preserve the opt-in setting across application restart. This is a runtime
  timing source, not an FPS setting or a changed scheduling policy.
- Five clock tests cover uncharged instructions, repeated reads, invalid budget
  movement, nested scopes, thread isolation, unwind cleanup, new dispatch origins,
  host time and overflow. Two restart tests pass, including preservation of the
  timing setting. An additional integration test runs real dispatch and a
  GetSystemTick SVC with timing disabled/enabled; observations agree with kernel
  accounting and the context is cleared after returning.
- Build from isolated committed sources plus these timing changes. Rebuild the
  committed renderer baseline needed by the current runtime API; exclude the
  separate in-progress GPU changes. All nine idle/turning/straight-movement
  controls preserve every previous CSV, WAV, PPM and RNG binary. Actual engine
  hooks observe time when enabled and no context when disabled; within-dispatch
  time differences agree with instruction charges to integer-rounding precision.
- Independently verify all nine input/save/binary/harness/manifest records and
  the compiled runtime snapshots. Final formatting-only changes to the kernel
  and dispatch source match the snapshots after rustfmt normalization. Preserve
  runtime-clock-build-v2/runtime-clock-tests-v2 evidence; the first build's old
  renderer dependency lacked the required API and produced no executable.
- Native consumers can now obtain a common guest-time source in the ordinary
  runtime. Publishing one elapsed step per engine update, converting the existing
  half-step/counter policies, and separating update deadlines from the hardware
  frame wait remain required for freely varying native FPS. These controls do
  not establish throughput or arbitrary-rate gameplay correctness.

### R-168 2026-10-30 — Publish one immutable elapsed interval per engine update

- Add core `UpdateClock` and `UpdateStep` types. Sample guest time once at an
  update boundary, then share the same sequence, interval and logical-tick ratio
  among consumers. The first sample establishes an origin without inventing
  elapsed time. Zero-time, rewind and exhausted-sequence cases clear the current
  step instead of reusing an earlier interval. Explicit clear excludes a paused
  gap while retaining monotonically increasing sequence IDs within the clock's
  lifetime; callers still own per-state credit invalidation.
- All eight clock/step module tests pass. New coverage checks changing intervals,
  immutable repeated reads, elapsed-time conservation, reset gaps, rewind,
  sequence lifetime and large timestamps. These tests establish interval
  arithmetic and ownership rather than arbitrary-rate game behavior.
- In a private harness, publish at the engine update entry preceding its process
  update callbacks. Use the runtime guest clock from R-167 and an identical
  snapshot of the public step source. Record each observed domain's step without
  changing the simulation's existing timing calculations.
- All six normal/higher-cadence idle, turning and straight-movement comparisons
  preserve previous captures exactly. Independently verify inputs, saves,
  binaries, harness, manifests and public/source snapshot identity. Between
  engine frames 600–800, each normal case publishes 99 intervals and each higher
  case 198. Observed consumers share the exact published interval, with only one
  sequence per engine frame. Frames 600 and 602 precede a usable anchor and
  explicitly have no step; boot/transition initialization remains to be handled.
- Initial steady samples are nominally 33,427,360 ns normally and approximately
  half that at higher cadence. Longer recordings show dispatch-boundary jitter:
  turning ranges about 30.50–36.36 ms normally and 15.58–20.15 ms at higher
  cadence. Means remain approximately 33.428 ms and 16.714 ms. These are guest
  update intervals, not host FPS or a newly unlocked scheduling mode.
- Preserve update-step evidence. The next behavioral change must replace fixed
  half-step and frame-count assumptions with these shared intervals, validating
  animation/event/timer interactions. Update deadlines remain hardware-paced;
  observation alone does not implement freely varying native frame rates.

### R-169 2026-10-30 — Direct elapsed-time consumers expose event-ordering regressions

- The private `elapsed-consumers` experiment replaces fixed half steps in scoped
  movement/animation consumers and quaternion smoothing with R-168's shared
  elapsed interval. Per-state timers accumulate integer guest nanoseconds, reject
  duplicate consumption, and invalidate their credit at audited reset sites.
  The old path remains selectable. This is a diagnostic, not a production enable.
- Nine identical-save/input replays completed: idle, turning, and straight walking,
  each with normal, old high-cadence, and elapsed-time policies. All six normal/old
  controls match every recorded CSV, WAV, PPM, and BIN artifact exactly. Input,
  save, executable, harness, and manifest hashes were checked for all nine cases.
- The elapsed candidate is **not behaviorally accepted**. Turning loses two action
  transitions (seven becomes five), recorded RNG calls drop from 161 to 157, and
  audio-state records change from 507 to 472 versus the old high-cadence path.
  Straight walking moves an action from frame 686 to 688 and changes audio-state
  records from 477 to 602. These counts describe instrumentation, not audible
  event counts. All WAV files retain 523,520 stereo samples at 32,728 Hz; equal
  duration does not establish equivalent sound or event timing.
- A concrete boundary mismatch appears in turning: the old countdown is zero at
  the action decision on frame 626, but the elapsed countdown reaches zero only
  at frame 627. Meanwhile the animation's frame-626 interval ends at about
  4.0001402 rather than exactly 4.0; the next interval starts beyond 4.0. The old
  path selects a new action on frame 627 while the candidate does not. This
  motivates isolating timer policy from continuous motion and inspecting event
  results; it does not yet prove the complete causal chain or justify an epsilon
  comparison, dropped elapsed time, or unconditional deferred events.
- A separate demonstrated long-interval bug is fixed in the private
  `elapsed-countdown` candidate: decrementing a remaining count of one by two
  wrapped the guest's unsigned subtraction to 4,294,967,295. Bounding the emitted
  decrement by the remaining count reaches zero while accumulating timers still
  receive both ticks. The original helper fails this targeted guard; the corrected
  helper passes it and the variable-interval/reset guard. All three gameplay
  replays remain artifact-identical to the preceding elapsed candidate, including
  its known regressions. This fixes countdown expiry, not event ordering.
- Private scripts and artifacts use `elapsed-consumers`, `elapsed-countdown`, and
  their corresponding `-build`/`-tests` directories under the native-FPS research
  cache. Root dispatch remains unoptimized and runs are CPU/memory constrained;
  no throughput gain or freely varying presentation rate is established here.
  Hardware-paced scheduling and broader lifecycle validation remain outstanding.
- Follow-up `elapsed-timer-isolation` keeps continuous consumers on elapsed time
  while selecting the old timer policy. This recovers seven turning transitions
  and 161 RNG calls, but audio-state records become 542 and some transition times
  still differ. The all-elapsed control is artifact-identical. This isolates the
  transition-count change to timer policy; keeping old fixed timers is not the
  requested freely varying implementation.
- Further tracing narrows the early difference more precisely than the countdown
  correlation above: the turn qualification counter at component +0x3c must reach
  four to retain +0x38. At frame 627 the elapsed counter remains three, while the
  old policy reaches four. The selector at 0x6f04f718 reads +0x38 and changes its
  selected state from four to five only in the latter case. The eight published
  elapsed intervals at frames 620–627 total 133,705,988 ns, **3,452 ns short** of
  four nominal ticks (133,709,440 ns). Thus the elapsed accumulator is rounding
  correctly; forcing it to four would discard the real threshold distinction.
  Qualification resets at frame 628. This is a boundary-sensitive behavioral
  difference, not proof that the animation event query itself dropped an event.
- `elapsed-event-return` records 356 query invocations in each isolated replay
  without changing any pre-existing capture. Of these, 333 return directly to the
  audited caller and 23 leave through another continuation; those 23 are not
  completed return-value observations. Frame 627 directly returns four in both
  policies. Next work must distinguish legitimate changed sampling at a strict
  threshold from incorrect lifetime accounting and test sustained qualification
  under variable cadence; do not add an epsilon or fabricate a timer tick merely
  to recover a borderline replay's old transition count.

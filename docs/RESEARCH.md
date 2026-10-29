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

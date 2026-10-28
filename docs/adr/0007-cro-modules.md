# ADR-0007: CRO modules translated position-independently, ahead of time

- Status: accepted
- Date: 2026-10-25

## Context

Ultra Sun loads its CRS and CRO modules (3dbrew "CRO0") from the RomFS
through `ldr:ro` (R-028). The game reads a module into a heap buffer
and passes the buffer to RO. RO maps it at an address derived from the
buffer's: below 0x08000000 the same, otherwise the buffer address minus
(0x08000000 minus the end of the executable's aligned segments) (3dbrew
"RO Services"). Ultra Sun's CRS buffer 0x081DE000 maps to 0x008B2000.
Heap addresses depend on what the game has allocated before, and the
game loads modules per scene. So **a module's load address is not known
before it loads**, and may differ between loads.

RO then relocates the module in place (internal relocations, imports and
exports: R_ARM_ABS32, REL32, THM_PC22, CALL, JUMP24, TARGET1, PREL31).
In principle that patches code too, since branch relocations would
rewrite BL/B instructions. The measurement below shows these games use
none.

Static recompilation needs the code ahead of time, and every
translated function today embeds absolute guest addresses (labels,
return addresses, branch targets).

## Measurement that shaped the decision

Every relocation in both games, the 102,473 of all 133 modules and the
CRS's, is **R_ARM_ABS32** (R-030). Inside code segments these are
literal-pool words, not instructions. So RO never rewrites an
instruction: module code is the stored code, only moved, and every
address it uses that crosses a module boundary is data it loads.

## Decision

- Each CRO module (not the CRS) is translated ahead of time from its
  file in the RomFS, at a **virtual base** of its own (module k at
  0x40000000 + k × 16 MB; `usumctl translate`). Discovery is seeded from
  the module's own tables: exports, OnLoad / OnExit / OnUnresolved,
  internal relocations into text, and other modules' anonymous imports
  of it. It is not seeded from its words, which are not relocated yet.
- Module code is emitted in a **relocatable mode**. The function's `pc`
  stays virtual, so labels and branches are constants as for the
  executable. Every code address the guest can see (return addresses,
  PC reads, `r15` on exit) is `delta + virtual`, and addresses taken
  from registers are `value - delta`. `delta` is `cpu.module_delta`:
  the load address minus the virtual base.
- The host dispatch sets `cpu.module_delta` whenever it enters module
  code. It looks the PC up in the executable's entries, then in the
  loaded module containing it, whose file must have the SHA-256 that was
  translated. Jumps between modules and the executable are indirect
  (loaded addresses) and so always pass through the host. A direct call
  never leaves its module.
- Module functions follow the executable's in the one function table.
- RO itself is high-level emulated (`usum_runtime::ro`): map at the
  derived address (the buffer's pages become inaccessible at their
  heap address), .data and .bss into the game's buffers, relocations
  per 3dbrew's `writeRelocation`, and linking in both directions. The
  header's offsets are rebased to addresses, which is a hypothesis from
  the game's loader (R-030).

## Consequences

- One translation per module file, whatever its load address. For Ultra
  Sun: 132 modules, 59,599 functions, 2.67M emitted instructions, 201
  generated crates (they are game data: private cache only).
- Module code costs one addition at each address the guest sees. The
  executable's code is unchanged.
- A module whose file differs from the translated one is not run: it
  stops as untranslated code, loudly.
- Open: RO's fix levels (the size after fixing is answered as the whole
  module) and any further header fields the game reads.

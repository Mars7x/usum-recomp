# ADR-0008: Native dynamic FPS as translator patch points and a runtime cadence module

- Status: proposed
- Date: 2026-11-01

## Context

Ultra Sun's engine can update faster than its normal 30 Hz when asked
(R-125), but many of its timers count updates or hardware frames rather
than time. The private research harness has corrected those found so far
and replays one save at variable update intervals with matching walking
distance, encounter checks and random-number use (R-170–R-180). It does
this with:

- hooks swapped into the executable's dispatch table at function entries
  (update start 0x401438, the hardware-frame wait 0x548854 and its event
  helper 0x279898, the rate getter 0x4b7f94, 0x4ba088, 0x4c1c8c, the
  position setter 0x429c50, the clock observer 0x401710);
- hand-edited copies of generated units (u031, u034, u035, u041, u049 and
  FieldRo's u077) that change single blocks: elapsed scaling of the
  movement cap and root-motion rate, elapsed chance and counter clocks,
  per-update animation event credits;
- guards on heap addresses that only hold for that save (the engine
  manager 0x349a0fe0, the player's state 0x349baf94, model 0x312ba654,
  chance state 0x312b993c, controller 0x349bae20), on a frame window
  (> 602) and on diagnostic `WATCH` values selected per experiment.

None of this can ship: generated units are rebuilt by the translator, the
addresses differ per save and scene, and the frame window and `WATCH`
modes exist only for comparisons.

## Decision

- **One switch.** Native cadence is a runtime option (`USUM_NATIVE_FPS`,
  later a launcher setting), off by default. Every correction checks it,
  replacing the frame window and the `WATCH` modes. With it off, the game
  runs exactly as today; the replay controls verify that.
- **Patch points in the translator.** A table in `usum-recomp`, keyed by
  the identified version profile, names the guest blocks that need
  timing treatment and the hook to call (for example: scale the result of
  the cap subtraction at 0x3a9988 by the update's logical step). The
  emitter inserts a call to a `usum_guest::native` function at each one,
  in every generated copy of the block. The table holds addresses and
  descriptions only, never game code (ADR-0004).
- **Dispatch-table hooks in the runtime.** `usum_runtime::native_cadence`
  owns the update-start, frame-wait, rate and position hooks. `play` and
  `probe` copy the entry table into owned memory and install the hooks
  when the switch is on.
- **Objects by type, not by address.** The engine manager is read from
  the global at 0x6a3984 (R-178 follow-up). Per-actor corrections select
  objects by vtable or by the structure the game itself checks, so they
  apply to every actor of that kind, not only the player in one save.
- **Elapsed time from one source.** All corrections take time from the
  shared update step (`elapsed / 33,427,360 ns` logical ticks), with the
  carry and duplicate-call rules already validated for chance checks and
  counters (R-175, R-169).
- **Each correction keeps its replay evidence.** A patch point is only
  added with an identical-input comparison against normal cadence: exact
  output with the switch off, and recorded movement, encounter, RNG and
  sound statistics with it on.

## Consequences

- Translation output changes when the switch's patch points are added,
  so a retranslate and rebuild is needed once; afterwards the switch is a
  runtime choice.
- Coverage is limited to what has been found. Scenes not yet exercised
  (battles, cutscenes, menus, other actors) may still run too fast or too
  slow with the switch on; it stays labelled experimental until those are
  covered.
- The private harness remains the place to find new timing dependencies;
  the translator table is where accepted ones go.

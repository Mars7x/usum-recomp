# Next steps

Updated 2026-10-30. Ultra Sun boots, runs the opening and menus, reaches
indoor/outdoor movement and early battles, and supports recorded replay tests.
Full-game completion and Ultra Moon gameplay parity remain unverified.
See [compatibility](COMPATIBILITY.md), [building and controls](BUILDING.md),
[roadmap](ROADMAP.md) and the chronological [research log](RESEARCH.md).

## Current implementation

- ARM/Thumb/VFP translation covers the supported executables and their CRO
  modules. Generated workspaces contain deterministic `probe` and interactive
  `play` programs. Translation output remains private in the user's cache.
  Build cost depends on toolchain, job count, RAM and generated changes; early
  research timings are historical measurements, not current promises.
- The 3DS compatibility runtime supplies checked memory, threads and kernel
  objects, services, input, audio, save handling, module loading and restart.
  Software PICA rendering remains the comparison reference.
- Vulkan is the default in generated `play` builds. Acceleration executes vertex and fragment work, keeps supported
  color/depth and display data resident, and presents through the GPU with
  fallback. Internal resolution scaling is 1×–8×; 6× is 2400×1440 for the top
  screen. Scaling changes rasterization resolution, rather than only enlarging
  the window. Original low-resolution textures and 2D elements remain limited
  by their source assets.
- `USUM_VULKAN_ACCELERATE=0` selects the software reference at 1×. The
  deterministic probe keeps explicit renderer selection for comparisons.
- Keyboard, mouse touch and mapped controllers work. Controllers connected
  after launch are discovered; disconnect and focus loss release inputs.
- F3/F4 adjust resolution; F5 toggles title statistics; F6 changes the host
  presentation cap; F7 toggles the testing overlay, which is off by default.
  These controls do not unlock the game's native frame rate.
- Recent validated work includes GPU-resident window presentation (R-115),
  byte-packed script/area callback discovery (R-116), checked register-list
  memory transfers (R-117), presentation timing/HUD controls (R-118) and exited
  thread TLS reuse with retained handle identity (R-119).
- Repeated-address dispatch caching adds a small measured intro improvement
  (R-124). Gameplay FPS gains remain limited/uncertain. The engine's existing
  higher-cadence request nearly doubles movement in a fixed-input diagnostic
  (R-125), so it is not enabled as a frame-rate unlock. Field-update isolation
  restores travel distance in a diagnostic, but intermediate motion and audio
  still differ (R-127). Scaling only player movement still changes encounter
  behavior (R-130). Separate movement accumulation is now traced (R-133);
  correcting it avoids one early encounter, but movement and audio still differ.
  Coordinated animation and movement-cap stepping now matches intended steady
  movement in straight replays and produces additional distinct game images
  (R-136). Position, effects and timing differences remain; the prototype is
  private and native dynamic FPS is still being developed.
  Higher presentation counts alone are not native FPS.
- Coarser GPU input scheduling reduces worker overhead (R-128), with about 3%
  overworld and 2.5% battle throughput gains in isolated replays. Normal-speed
  play remains cadence-limited, and intro results vary between test modes.
- Bulk stencil readback extraction reduces measured host instructions by about
  2.6–2.7% (R-132), with small isolated throughput gains and little change in
  normal-speed gameplay. The scalar comparison path remains available.
- Guarded native single-precision arithmetic improves isolated overworld/battle
  throughput by about 4.0%/2.3% (R-137), with exact recorded images, audio and
  saves. Normal-speed gameplay remains cadence-limited; the reference path
  is available for comparisons.
- Tracked module revisions reduce repeated dispatch checks (R-135), improving
  isolated intro throughput by about 2.3%. Gameplay changes remain small and
  uncertain; loaded-module lifecycle checks and comparison paths are retained.
- LCD tile-row conversion reduces measured host instructions by about 4%
  (R-134), with roughly 1% isolated gameplay throughput gains. Normal-speed
  overworld and battle remain limited by the existing engine cadence.
- Offline Mystery Gift search cancellation and system network-error dialogs now
  return through the game's normal menu/restart flow (R-131). These are offline
  compatibility paths; gift delivery and communication with peers are absent.

## Active work

1. **Native dynamic FPS.** Keep this a core feature over the compatibility
   runtime. Identify actual engine frame completion and simulation timing before
   uncapping. Preserve movement, animation, encounters, battle/event timing and
   audio at normal speed while producing additional rendered frames. Aim for
   substantially better measured gameplay performance at 6×. Generated frames,
   extrapolation, repeated window presents or a faster game clock do not count
   as native rendering improvements. Compare movement and animation at common
   game-time checkpoints, and compare encounter decisions, event ordering and
   audio start times against the original cadence. Additional frames must come
   from engine rendering with correctly advanced scene state. R-143 tests clocks
   anchored to observed counter resets and adjusts effective turning/blend steps.
   Straight travel is close, but transition positions and effect timing still
   diverge. General lifecycle/reset integration remains necessary; the private
   diagnostic is not a shipped unlock.
2. **Profile-led optimization.** Revisit intro/movie helpers, checked memory,
   frequent guest helpers, vertex preparation and GPU synchronization using
   fresh profiles. Retain a reference path and frozen before/after executables.
   Require identical replay inputs, starting saves, captures, audio and guest
   counters before accepting a timing-neutral optimization. Benchmark without
   captures/profiling, repeat in balanced order, and wait for encoders rather
   than interrupting them. Reject overlapping measurements and changes without
   demonstrated benefit. See R-084/R-085/R-098 and later investigations.
3. **Compatibility and graphics.** Keep fixes narrowly scoped and replayable.
   The recorded thread-resource stop is reproduced and the failed SVC succeeds
   with TLS recycling in the same state; gameplay beyond it is not validated by
   that check. Continue investigating rendering differences and reported menu/
   transition problems without treating one fix as full-game compatibility.
   Outlines should remain enabled by default; a user-facing outline toggle is
   planned.
4. **Sound quality.** PCM/DSP-ADPCM source decoding, mixing and source filters
   exist; master-volume/effect behavior and hardware accuracy remain incomplete.
   A slow host can still produce audio gaps. Intro performance must be measured
   separately from gameplay.
5. **Usable distribution.** Finish the launcher, portal import, build progress,
   offline toolchain and Flatpak workflow (M6/ADR-0003), then expand host/GPU and
   Ultra Moon validation. Neither packaging nor an ARM Linux build is released.

## Measurement limits

The HUD's `FPS` line currently counts top-screen GSP framebuffer submissions
per real second; `WIN` counts successful window presents. Submissions do not
prove unique images or native engine frame completion. Unthrottled `probe`
throughput also does not establish normal-speed uncapped FPS. Keep these
observations separate when reporting native gameplay performance.

## Development boundaries

Keep the 3DS runtime as the compatibility foundation. Targeted native helpers
may be evaluated where profiling and semantic validation support them; a full
native rewrite is not the objective. Do not change another contributor's active
work. Commit substantial validated progress and update public status documents
without publishing game-derived material.

Only audited project source and original synthetic tests belong in Git. ROMs,
translations, saves, recordings, screenshots, audio and raw GPU captures stay
private. Differential tests use external QEMU/ARM binutils; see BUILDING.md.
Historical hypotheses and measurements remain in RESEARCH.md rather than being
silently rewritten as current facts.

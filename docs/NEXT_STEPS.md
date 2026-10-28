# Next steps

For the next session. M1–M3 are done; M4 is nearly done; M5 is in
progress (ROADMAP.md). Evidence: RESEARCH.md R-001 to R-052.

Ultra Sun runs from the first-boot language menu, through the intro,
character selection and name entry, past the title screen and the
aerial view of Melemele Island, into the story. Meowth wakes the player,
and the player can then get up and walk around the bedroom. `play`
shows it in a window with keyboard and mouse (touch) input and sound
(BUILDING.md), at about real time in the overworld (0.99×, R-041).

## State

- Every real instruction in both executables decodes and translates
  (`usumctl analyze`).
- `usumctl translate <dump>` writes the whole executable and the 132 CRO
  modules as a Cargo workspace, with `probe` (scripted, deterministic,
  reports) and `play` (window, live). A release build takes 6-8 minutes
  (R-022).
- Threads: ADR-0006. CRO modules: ADR-0007.
- Software PICA200 renderer, including fragment lighting (R-032, R-033,
  R-051). The 650 s grass replay completes cleanly and shows the outdoor
  scene correctly (R-052).
- `usum-render` opens the RX 9070 XT through Vulkan and submits a real
  off-screen graphics pipeline. It accepts PICA-style clip positions and
  colors, preserves color/depth targets across draws, applies PICA viewport,
  culling, depth, color masks, alpha test and blending, samples all three
  texture units, executes six TEV stages, batches draws and evaluates the
  complete fragment-lighting LUT path. Real game draws, including the lit
  grass scene, closely match the software reference, and an experimental
  opt-in path replays live command lists and writes Vulkan color back to guest
  framebuffers; persistent live targets, shared textures, depth comparison and
  separately opt-in depth writeback are now present. A guarded mode can skip
  software rasterization and rerun dependent or failed command lists safely;
  it is 33% faster in a deterministic 10-second startup run (R-052 through
  R-063).
- DSP: high-level. Sources report their progress in the statuses
  (R-035); PCM and DSP-ADPCM are decoded and mix 0 is mixed and played
  through PulseAudio's native protocol (R-039); simple and biquad source
  filters are implemented (R-046). Not yet: the master volume and effect
  mixes.
- Local wireless: the game hosts and scans every ~100 VBlanks; modelled
  as no other console in range (R-035).
- Game mode of `usum-difftest` verifies real leaf functions against QEMU.

## Next, in order

1. **Vulkan renderer integration.** Keep the software renderer as the
   reference. The first live outdoor black-terrain defect is fixed: TEV buffer
   writes must become visible after the current stage reads them (R-070).
   The island shot at VBlank 10,801 now has RGB MAE 0.5004/255 versus software,
   down from 9.0499; the bottom screen is exact. Next rerun the later grass
   close-ups (23,401 and 27,001) and the complete 650-second recording, then
   investigate residual geometry/depth/coverage differences. Integer-depth
   equality and texture-unit 2 routing are also fixed (R-068/R-069).
   The shaded-vertex → persistent color-target → readback seam
   now includes register-driven raster state, decoded textures, the six TEV
   stages, multi-draw command buffers, quaternion/view lighting, all lighting
   LUTs, attenuation, Fresnel, immutable per-draw state and guest color/depth
   snapshot upload, single-submit draw/readback, and a feature-gated runtime
   capture/execute handoff for
   shaded vertices, decoded textures and immutable state (R-053 through R-059).
   Selected startup and fragment-lit grass draws now closely match the software
   result, and `USUM_VULKAN_LIVE=1` provides a safely fallible live color path
   (R-060 through R-063). GPU targets now persist across command lists and
   decoded textures and readback staging buffers are shared. A 60-second
   recorded replay accelerated 35,515 lists and safely reran 1,349 lists whose
   textures depended on earlier targets (R-064). All were exact RGBA8 reads of
   D24S8 targets; an exact CPU-mediated barrier eliminates the fallbacks but is
   deliberately diagnostic because synchronization makes it slower (R-065).
   The conversion now runs on the GPU and intermediate targets remain resident
   across segments (R-066). Intermediate barrier waits have been removed (R-067); next pipeline final upload/readback
   across independent command lists, and expand accelerated comparison evidence
   through the outdoor/grass replay.
   Add arbitrary
   border colors and resolve coverage-edge differences before making it the
   default. The 650 s replay spends 572 s drawing (338 s in rasterization), so
   presentation-only acceleration would not address the bottleneck (R-052).
2. **Sound quality.** The user hears it; it "seems odd" in places (to be
   described). Filters are in (R-046); the master volume and the effect
   returns need the DSP configuration layout, which 3dbrew does not give.
3. **Follow the game further:** the user plays `play`; each session is
   recorded and replays exactly in the probe (R-043). Blind scripted
   exploration is slow (R-046).
4. **Gamepads** (gilrs; libudev).
5. **M6, the Flatpak:** the manifest, `--socket=pulseaudio`, Wayland and
   X11, the translation step on first start (ADR-0003).
6. **System data:** let users supply their own console's system data
   (the NGWord list, R-034), never shipped.
7. Discovery precision; run-time speed of translated CPU code.

## Open questions

- Initial FPSCR of a thread (R-017); unaligned access semantics (R-010).
- APT: the startup parameter (hypothesis in R-027); NS's reply when no
  parameter is pending; the reset types of APT's events.
- DSP: the real firmware's structure addresses and sizes (ours are a
  choice, R-028); the audio pipe states other than 0. The source
  configuration's dirty bits and the end-of-sound report (buffer ID 0
  with the changed flag) follow Citra's HLE; the game accepts them, but
  they are not checked against the hardware (R-035).
- HID: the circle pad's range (±156 is a choice; R-035); the touch
  section's raw entry units (R-038).
- DSP: when the format is dirty on other SDK versions (bits 0-1 are never
  set by Ultra Sun; R-039).
- RO: fix levels; save commit semantics; the save size (R-031).
- GPU: pixel accuracy against the console (24-bit float precision,
  rasterization rules, filtering); clipping's z range (R-032).
- Flatpak toolchain, extension vs bundled (ADR-0003), at M6.

## Housekeeping

- Only audited project source belongs in Git. Translations live in
  `~/.cache/usum-recomp/translations/` (game-derived; do not share).
- Publishing this project does not imply that issues have been reported to
  upstream dependencies.
- Differential tests: the default corpus needs about 8 GB of RAM to build;
  add seeds rather than raising counts (R-013). Game mode:
  `USUM_DIFFTEST_DUMP` + `USUM_DIFFTEST_FUNCTIONS` (BUILDING.md).

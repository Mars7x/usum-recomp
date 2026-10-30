# Compatibility

Status as of 2026-10-30. Identification and translation are separate from
playability. No complete playthrough or broad hardware compatibility is claimed.

| Profile | Import / analysis | Runtime evidence |
|---|---|---|
| `ultra-sun-usa-cart` | Recognized; executable and CRO translation implemented | Boots; opening, menus, indoor/outdoor movement and early battles exercised. Vulkan supports hardware vertex shaders, 1×–8× internal scaling and GPU presentation. Recorded comparisons cover intro, overworld, battle, a longer session and application restart. Rendering, sound, performance and later-game compatibility remain incomplete. |
| `ultra-moon-usa-cart` | Recognized and analyzed | Equivalent gameplay validation has not been completed. |

Vulkan is the default in newly generated `play` builds; set
`USUM_VULKAN_ACCELERATE=0` for the 1× software reference. The renderer remains
experimental, with guarded fallback. `probe` retains explicit renderer selection
for developer comparisons.

Keyboard, mouse touch and controllers are supported; controllers can connect
after launch. Linux x86-64 is the tested host. An ARM Linux port, other GPUs and
full-game completion have not received equivalent validation.

Recent fixes include byte-packed script/area callback discovery (R-116) and
recycling TLS from exited threads (R-119). The latter reproduces the reported
stop with ten live and 22 exited threads, then successfully retries the failed
SVC in that same state. It does not claim a completed playthrough beyond it.

The recorded Mystery Gift back-out failure on network-exclusive-state release
is fixed (R-126), including the subsequent application restart. Online gift
delivery is not implemented. Offline initialization and read-only self-content
access are covered in R-129. Offline infrared search/cancellation and the Internet
system error dialog are covered in R-131, including repeated Internet attempts,
the code/password Internet confirmation, and return through application restart.
The dialog remains visible with the testing FPS overlay disabled. Actual infrared
peers, automatic search timeouts, online gift delivery and code entry after a
successful connection remain unsupported or unverified.

Native dynamic FPS remains unfinished. Neither host presentation caps nor
unthrottled deterministic replay throughput establish normal-speed uncapped
gameplay. See
[measurement controls](BUILDING.md#playing-in-a-window) and the
[research evidence](RESEARCH.md).

Evidence for each profile, measured from locally supplied dumps (R-002,
R-005 in RESEARCH.md): program ID, product code and NCCH version from the
NCCH header; code SHA-256 over the decompressed `.code`, cross-checked
with 3dsrecomp's independent decompressor. All stored SHA-256s matched.
Reproduce with:

```bash
USUM_DUMP_US=… USUM_DUMP_UM=… cargo test -p usum-import --test real_dumps -- --ignored
```

Other regions, revisions and updates are unsupported until someone
measures their dump and adds a profile, with evidence, here and in
`crates/usum-import/src/profile.rs`.

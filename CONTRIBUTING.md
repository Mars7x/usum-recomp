# Contributing

This is an experimental Linux port. Keep changes focused and describe the
problem, expected behavior, and checks performed.

## Keep private data private

Do not submit ROMs, decrypted code, generated game translations, assets,
keys, firmware, saves, audio, screenshots, or raw GPU captures. Use original
synthetic fixtures for regression tests. Reports may include a supported
profile name, software versions, reproduction steps, and sanitized errors;
remove personal paths and credentials. Do not post copyrighted instruction
listings or attach private replay recordings.

Enable the local guard after cloning:

```bash
tools/install-hooks.sh
```

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features usum-runtime/window,usum-runtime/vulkan-compare -- -D warnings
cargo test --locked --features usum-runtime/window,usum-runtime/vulkan-compare
tools/test-check-no-game-data.sh
tools/check-no-game-data.sh --staged
```

The full differential suite additionally needs QEMU and ARM binutils; see
[BUILDING.md](docs/BUILDING.md). CI uses a smaller deterministic corpus to fit
hosted-runner memory. Run the default full corpus locally for decoder or
recompiler changes. GPU smoke tests require a Vulkan-capable host and are
not claimed by ordinary headless unit-test results.

Do not change another contributor's in-progress work without coordination.
Do not bypass the data guard; improve it with a synthetic regression if it
rejects legitimate source.

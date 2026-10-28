# Compatibility

Status as of 2026-10-28. Identification and translation are separate from
playability. No complete playthrough or broad hardware compatibility is claimed.

| Profile | Import / analysis | Runtime evidence |
|---|---|---|
| `ultra-sun-usa-cart` | Recognized; executable and CRO translation implemented | Boots; intro, bedroom movement, and early outdoor gameplay tested. Experimental Vulkan completes a 650-second recorded sequence with zero software fallbacks and close software-reference images (R-071); rendering differences and performance work remain. |
| `ultra-moon-usa-cart` | Recognized and analyzed | Equivalent gameplay validation has not been completed. |

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

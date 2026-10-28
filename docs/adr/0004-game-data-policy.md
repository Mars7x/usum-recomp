# ADR-0004: Game data never enters the repository or a release

- Status: accepted
- Date: 2026-10-25

## Decision

- The repository and every release contain no Nintendo content: no
  dumps, code, translated code or objects made from code, RomFS files,
  keys, firmware or saves. Version profiles store hashes measured from
  users' dumps. Those are facts about a file, not its contents.
- Users supply their own decrypted dumps. We never download, request or
  point to dumps, keys or firmware, and never decrypt. The importer
  refuses encrypted input with instructions to decrypt on the user's own
  console.
- Users' files are never uploaded anywhere. Nothing in the project talks
  to the network.
- Dumps are only read, never modified or copied. Everything derived from
  them lives in the XDG cache directory (rebuildable). Saves live in the
  XDG data directory. Under Flatpak both are app-private.
- `.gitignore` covers the usual names. `tools/check-no-game-data.sh` also
  checks staged files' sizes and contents (NCSD/NCCH/CRO magic, the shape
  of recompiler output). It runs as a pre-commit hook
  (`tools/install-hooks.sh`) and over all tracked files in CI, and
  `tools/test-check-no-game-data.sh` tests it.

## Consequences

The pre-commit hook is opt-in per clone, so CI's `--tracked` run is the
backstop. Test fixtures are synthetic (`usum_import::fixture`).

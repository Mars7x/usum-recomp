#!/usr/bin/env python3
"""Inspect Git's index, never unstaged replacements, for private payloads."""
import re
import subprocess
import sys
from pathlib import PurePosixPath


def git(*args):
    return subprocess.check_output(['git', *args])


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else '--staged'
    if mode not in ('--staged', '--tracked'):
        sys.exit('usage: check-no-game-data.sh [--staged|--tracked]')
    command = ('diff', '--cached', '--name-only', '--diff-filter=ACMR', '-z') if mode == '--staged' else ('ls-files', '-z')
    names = git(*command).decode().split('\0')
    entries = {}
    for entry in git('ls-files', '--stage', '-z').decode().split('\0'):
        if entry:
            metadata, path = entry.split('\t', 1)
            entries[path] = metadata.split()[0]
    blocked = []
    extensions = {'.3ds', '.cci', '.cxi', '.cia', '.cfa', '.ncch', '.3dsx', '.app', '.cro', '.crs', '.sav', '.bin', '.garc', '.bcsar', '.so', '.rgba', '.f32le', '.ppm', '.wav'}
    directories = {'exefs', 'romfs', 'dump', 'dumps', 'extracted', 'generated', 'recordings', 'probe-frames', 'probe-save'}
    generated = re.compile(rb'(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?fn [ft]_[0-9a-fA-F]{8}\s*\(|^void [ft]_[0-9A-F]{8}\(Context \*ctx\)|^RECOMP_EXPORT const Entry recomp_entries')
    secrets = re.compile(rb'-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{30,}\b|\bgithub_pat_[A-Za-z0-9_]{30,}\b|\bAKIA[0-9A-Z]{16}\b')
    for name in filter(None, names):
        path = PurePosixPath(name)
        reason = None
        if entries.get(name) == '120000':
            reason = 'symlink (publish regular audited files only)'
        elif path.suffix.lower() in extensions or directories.intersection(path.parts) or path.name in {'usum-manifest.json', '.env', 'credentials.json'}:
            reason = 'private/game-data filename'
        else:
            data = git('show', ':' + name)
            if len(data) > 1024 * 1024:
                reason = 'larger than the 1 MiB source-file limit'
            elif data[256:260] in (b'NCSD', b'NCCH') or data[:4] == b'CRO0':
                reason = 'game container/module magic'
            elif data.startswith((b'\x7fELF', b'\x89PNG', b'\xff\xd8\xff', b'RIFF')):
                reason = 'binary executable or capture payload'
            elif b'\0' in data:
                reason = 'binary payload in a source repository'
            elif generated.search(data):
                reason = 'translated game-code function/table'
            elif secrets.search(data):
                reason = 'credential/private-key pattern'
        if reason:
            blocked.append((name, reason))
    for name, reason in blocked:
        print(f'refused: {name}: {reason}', file=sys.stderr)
    if blocked:
        print('Do not publish game-derived files or credentials; see docs/adr/0004-game-data-policy.md.', file=sys.stderr)
    return bool(blocked)


if __name__ == '__main__':
    sys.exit(main())

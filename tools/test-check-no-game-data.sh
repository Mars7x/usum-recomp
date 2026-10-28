#!/usr/bin/env bash
# Tests tools/check-no-game-data.sh in a throwaway repository, with
# synthetic files that only imitate game data's names, magic and shapes.
set -euo pipefail
check=$(realpath "$(dirname "$0")/check-no-game-data.sh")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q .

failures=0
expect() { # expect <refuse|accept> <description> <path>
	local want=$1 what=$2 path=$3 got
	git add -f -- "$path"
	if "$check" --staged >/dev/null 2>&1; then got=accept; else got=refuse; fi
	git rm -q --cached -- "$path"
	if [[ $got == "$want" ]]; then
		echo "ok    $want  $what"
	else
		echo "FAIL  wanted $want, got $got: $what"
		failures=$((failures + 1))
	fi
}

printf 'fn main() {}\n' >ordinary.rs
expect accept "ordinary source file" ordinary.rs

printf 'x' >game.3ds
expect refuse "file named like a dump" game.3ds

mkdir -p some/romfs && printf 'x' >some/romfs/a.txt
expect refuse "file under a romfs/ directory" some/romfs/a.txt

{ head -c 256 /dev/zero; printf 'NCSD'; head -c 256 /dev/zero; } >innocent.txt
expect refuse "renamed file with NCSD magic at 0x100" innocent.txt

{ printf 'CRO0'; head -c 60 /dev/zero; } >module.bin
expect refuse "file starting with CRO0" module.bin

printf '#include "recomp.h"\nvoid f_00100000(Context *ctx) {\n}\n' >fake.c
expect refuse "C shaped like 3dsrecomp output" fake.c

head -c $((2 * 1024 * 1024)) /dev/zero >big.bin
expect refuse "file over the size limit" big.bin

printf 'see docs: functions are named f_XXXXXXXX\n' >notes.md
expect accept "documentation mentioning the naming scheme" notes.md

printf 'pub fn f_00100000(cpu: &mut Cpu, mem: &mut Memory) {}\n' >fake.rs
expect refuse "Rust shaped like translated game code" fake.rs

printf 'pixels' >capture.rgba
expect refuse "raw framebuffer capture" capture.rgba

printf 'private data' >main.sav
expect refuse "game save" main.sav

printf '\177ELFpayload' >renamed.txt
expect refuse "renamed executable" renamed.txt

printf '%s%s\n' '-----BEGIN ' 'PRIVATE KEY-----' >secret.txt
expect refuse "private key" secret.txt

printf 'pub fn t_00100001() {}\n' >staged.rs
git add staged.rs
printf 'fn main() {}\n' >staged.rs
if "$check" --staged >/dev/null 2>&1; then
    echo "FAIL  staged payload hidden by clean working copy"
    failures=$((failures + 1))
else
    echo "ok    refuse staged payload despite clean working copy"
fi
git rm -q -f --cached staged.rs

if ((failures)); then
	echo "$failures failed"
	exit 1
fi
echo "all passed"

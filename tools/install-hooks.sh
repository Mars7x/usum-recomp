#!/usr/bin/env bash
# Points git at the repository's hooks (tools/hooks), once per clone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath tools/hooks
echo "git hooks enabled from tools/hooks"

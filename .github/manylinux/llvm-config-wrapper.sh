#!/bin/bash
set -euo pipefail

REAL="${LLVM_CONFIG_REAL:-/opt/llvm21/bin/llvm-config.real}"

if [[ "${1:-}" == "--system-libs" ]]; then
  "$REAL" "$@" | sed -E "s@/usr/lib[^ ]*/lib([A-Za-z0-9_+.-]+)\\.a@-l\\1@g"
  exit 0
fi

if [[ "${1:-}" == "--cflags" && "${LLVM_CONFIG_STRIP_CFLAGS:-0}" == "1" ]]; then
  "$REAL" "$@" | sed -E "s@(^| )-flto(=[^ ]+)?@@g; s@(^| )-fuse-ld=[^ ]+@@g; s@  +@ @g; s@^ @@"
  exit 0
fi

exec "$REAL" "$@"

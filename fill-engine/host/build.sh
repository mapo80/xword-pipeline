#!/usr/bin/env bash
# Build xfill-host next to the engine library:
#   host/build.sh [output dir]   (default: target/release)
# Windows (MSVC, Developer Command Prompt):
#   cl /nologo /O2 /W4 /WX /std:c11 host\xfill-host.c /Fe:target\release\xfill-host.exe
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-target/release}"
mkdir -p "$out"
flags=(-std=c11 -O2 -Wall -Wextra -Werror)
case "$(uname -s)" in
  Linux) cc "${flags[@]}" host/xfill-host.c -o "$out/xfill-host" -pthread -ldl ;;
  Darwin) cc "${flags[@]}" host/xfill-host.c -o "$out/xfill-host" -pthread ;;
  MINGW*|MSYS*|CYGWIN*) cc "${flags[@]}" -municode host/xfill-host.c -o "$out/xfill-host.exe" ;;
  *) echo "unsupported system $(uname -s)" >&2; exit 1 ;;
esac
echo "$out/xfill-host"

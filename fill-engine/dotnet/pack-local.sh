#!/usr/bin/env bash
# Pack Cruciverba.Xfill with the native library of this machine only
# (cargo build --release -p xfill-ffi first). CI packs all six RIDs.
set -euo pipefail
cd "$(dirname "$0")"
rid=$(dotnet --info | awk '/RID:/ {print $2; exit}')
case "$rid" in
  win-*) lib=xfill.dll ;;
  osx-*) lib=libxfill.dylib ;;
  *) lib=libxfill.so ;;
esac
mkdir -p "runtimes/$rid/native"
cp "../target/release/$lib" "runtimes/$rid/native/$lib"
dotnet pack Cruciverba.Xfill -c Release -o artifacts -p:Version="${1:-0.1.0}"

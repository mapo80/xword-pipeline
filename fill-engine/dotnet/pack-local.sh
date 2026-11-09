#!/usr/bin/env bash
# Pack Cruciverba.Xfill with the engine library and xfill-host of this machine
# only (cargo build --release -p xfill-ffi and host/build.sh first). CI packs
# all six RIDs.
set -euo pipefail
cd "$(dirname "$0")"
rid=$(dotnet --info | awk '/RID:/ {print $2; exit}')
case "$rid" in
  win-*) lib=xfill.dll; host=xfill-host.exe ;;
  osx-*) lib=libxfill.dylib; host=xfill-host ;;
  *) lib=libxfill.so; host=xfill-host ;;
esac
mkdir -p "runtimes/$rid/native"
cp "../target/release/$lib" "../target/release/$host" "runtimes/$rid/native/"
dotnet pack Cruciverba.Xfill -c Release -o artifacts -p:Version="${1:-0.2.0}"

#!/usr/bin/env bash
set -euo pipefail
# Debian/Ubuntu prerequisites: git cmake g++ gperf libssl-dev zlib1g-dev make.
# Run manually; this script downloads and compiles TDLib, never application tests.
TC_TDLIB_REF="${TDLIB_REF:-master}"
TC_BUILD_ROOT="${TC_BUILD_ROOT:-$PWD/.tdlib-build}"
TC_INSTALL_ROOT="${TC_INSTALL_ROOT:-$PWD/tdlib-native}"
TC_JOBS="${BUILD_JOBS:-2}"
mkdir -p "$TC_BUILD_ROOT" "$TC_INSTALL_ROOT/lib"
if [ ! -d "$TC_BUILD_ROOT/source/.git" ]; then
  git init "$TC_BUILD_ROOT/source"
  git -C "$TC_BUILD_ROOT/source" remote add origin https://github.com/tdlib/td.git
fi
git -C "$TC_BUILD_ROOT/source" fetch --depth=1 origin "$TC_TDLIB_REF"
git -C "$TC_BUILD_ROOT/source" checkout --detach FETCH_HEAD
cmake -S "$TC_BUILD_ROOT/source" -B "$TC_BUILD_ROOT/build" -DCMAKE_BUILD_TYPE=Release -DTD_ENABLE_JNI=OFF
cmake --build "$TC_BUILD_ROOT/build" --target tdjson --parallel "$TC_JOBS"
cp -a "$TC_BUILD_ROOT/build/"libtdjson.so* "$TC_INSTALL_ROOT/lib/"
printf 'Set TDLIB_PATH=%s/lib/libtdjson.so\n' "$TC_INSTALL_ROOT"

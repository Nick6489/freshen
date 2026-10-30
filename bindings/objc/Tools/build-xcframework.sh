#!/bin/sh
# Builds bindings/objc/CFreshen.xcframework: the freshen-ffi static library and its
# C header. macOS only (needs lipo and xcodebuild).
#
#   Tools/build-xcframework.sh                      universal (arm64 + x86_64), release
#   FRESHEN_PROFILE=debug FRESHEN_TARGETS=aarch64-apple-darwin Tools/build-xcframework.sh
#
# A static library is used on purpose: it links into the application executable, so
# there is no extra dylib to embed, sign, or list among the files an update owns.
set -eu

if [ "$(uname)" != "Darwin" ]; then
  echo "build-xcframework.sh needs macOS (lipo, xcodebuild)" >&2
  exit 1
fi

root=$(cd "$(dirname "$0")/../../.." && pwd)
objc="$root/bindings/objc"
profile=${FRESHEN_PROFILE:-release}
targets=${FRESHEN_TARGETS:-"aarch64-apple-darwin x86_64-apple-darwin"}
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

cd "$root"
build="$objc/build"
mkdir -p "$build/headers"

libs=""
for target in $targets; do
  rustup target add "$target" >/dev/null 2>&1 || true
  if [ "$profile" = "release" ]; then
    cargo build -p freshen-ffi --release --target "$target"
  else
    cargo build -p freshen-ffi --target "$target"
  fi
  libs="$libs $root/target/$target/$profile/libfreshen_ffi.a"
done

# shellcheck disable=SC2086
lipo -create $libs -output "$build/libfreshen_ffi.a"
cp "$root/ffi/include/freshen.h" "$build/headers/freshen.h"

rm -rf "$objc/CFreshen.xcframework"
xcodebuild -create-xcframework \
  -library "$build/libfreshen_ffi.a" \
  -headers "$build/headers" \
  -output "$objc/CFreshen.xcframework"

echo "built $objc/CFreshen.xcframework ($profile: $targets)"

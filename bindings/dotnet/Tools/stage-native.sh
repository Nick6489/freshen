#!/bin/sh
# Builds the freshen native library and the freshen-helper executable for THIS
# machine and stages them where `dotnet pack` expects them:
#
#   bindings/dotnet/runtimes/<rid>/native/{libfreshen_ffi.so | libfreshen_ffi.dylib | freshen_ffi.dll}
#   bindings/dotnet/runtimes/<rid>/native/{freshen-helper | freshen-helper.exe}
#
# The layout mirrors the package's own runtimes/ folder. A full package needs every
# platform: run this on each one (CI does) and gather the results under
# bindings/dotnet/runtimes/ before packing. Override the detected runtime
# identifier with FRESHEN_RID (for example when cross-compiling).
#
# Release builds should run on the OLDEST OS you intend to support: a Linux library built
# on a very new distribution needs a newer glibc than an older machine has.
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
profile=${FRESHEN_PROFILE:-release}

case "$(uname -s)" in
  Linux)  os=linux; library=libfreshen_ffi.so;    helper=freshen-helper ;;
  Darwin) os=osx;   library=libfreshen_ffi.dylib; helper=freshen-helper ;;
  MINGW*|MSYS*|CYGWIN*) os=win; library=freshen_ffi.dll; helper=freshen-helper.exe ;;
  *) echo "stage-native.sh: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64|AMD64) arch=x64 ;;
  arm64|aarch64|ARM64) arch=arm64 ;;
  *) echo "stage-native.sh: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
rid=${FRESHEN_RID:-$os-$arch}

cd "$root"
if [ "$profile" = "release" ]; then
  cargo build --release -p freshen -p freshen-ffi
else
  cargo build -p freshen -p freshen-ffi
fi

destination="$root/bindings/dotnet/runtimes/$rid/native"
mkdir -p "$destination"
cp "$root/target/$profile/$library" "$destination/$library"
cp "$root/target/$profile/$helper" "$destination/$helper"
echo "staged $rid ($profile) in $destination"

#!/bin/sh
# Syntax-checks the Objective-C sources and tests with ARC, blocks, nullability and
# warnings-as-errors WITHOUT a macOS SDK, against the stub Foundation and XCTest in
# Tools/Stubs. Useful on Linux CI and on machines without Xcode.
#
# This is NOT a build. It proves the code is well-formed Objective-C that ARC
# accepts; it cannot prove Foundation is used correctly or that the tests pass.
# Build and test on macOS (Tools/run-tests.sh) for that.
#
# Extra arguments are additional source files to check (used to test the checker).
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
stubs="$here/Tools/Stubs"
ffi="$here/../../ffi/include"
clang_bin=${CLANG:-clang}

flags="-fsyntax-only -x objective-c -std=gnu11 -fobjc-arc -fblocks -fobjc-runtime=macosx-11.0 \
  -Wall -Wextra -Werror -Wno-unused-parameter -Wno-nullability-completeness \
  -Wconversion -Wno-sign-conversion -Wno-shorten-64-to-32 \
  -Wimplicit-retain-self -Wobjc-designated-initializers -Wnullable-to-nonnull-conversion \
  -Wunguarded-availability -Wstrict-prototypes \
  -I $stubs -I $stubs/Foundation -I $stubs/XCTest -I $ffi \
  -I $here/Sources/FreshenObjC/include -I $here/Sources/FreshenObjC \
  -I $here/Tests/FreshenObjCTestSupport/include -I $here/Tests/FreshenObjCTests"

status=0
for source in \
  "$here"/Sources/FreshenObjC/*.m \
  "$here"/Tests/FreshenObjCTestSupport/*.m \
  "$here"/Tests/FreshenObjCE2E/*.m \
  "$here"/Tests/FreshenObjCTests/*.m \
  "$@"; do
  # shellcheck disable=SC2086
  if ! "$clang_bin" $flags "$source"; then
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "objc syntax check: ok"
fi
exit "$status"

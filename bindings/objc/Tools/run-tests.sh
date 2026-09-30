#!/bin/sh
# Builds everything the Objective-C tests need and runs them. macOS only.
#
#   Tools/run-tests.sh
#
# 1. the freshen-ffi static library for this Mac as CFreshen.xcframework (debug)
# 2. the freshen-helper binary and the make_fixtures / confirm_app examples
# 3. signed fixtures for the tests
# 4. `swift test`, which also builds the end-to-end parent application
set -eu

if [ "$(uname)" != "Darwin" ]; then
  echo "run-tests.sh needs macOS. On other systems run Tools/check-syntax.sh." >&2
  exit 1
fi

root=$(cd "$(dirname "$0")/../../.." && pwd)
objc="$root/bindings/objc"
host=$(rustc -vV | sed -n 's/^host: //p')

FRESHEN_PROFILE=debug FRESHEN_TARGETS="$host" sh "$objc/Tools/build-xcframework.sh"

cd "$root"
cargo build -p freshen --bins
cargo build -p freshen-ffi --examples

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM
"$root/target/debug/examples/make_fixtures" "$scratch/fixtures" "$root/target/debug/examples/confirm_app"

cd "$objc"
swift build --product FreshenObjCE2E
bin=$(swift build --show-bin-path)

FRESHEN_TEST_FIXTURES="$scratch/fixtures" \
FRESHEN_TEST_HELPER="$root/target/debug/freshen-helper" \
FRESHEN_TEST_CONFIRM_APP="$root/target/debug/examples/confirm_app" \
FRESHEN_TEST_E2E="$bin/FreshenObjCE2E" \
  swift test

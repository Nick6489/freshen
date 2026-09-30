#!/bin/sh
# Stages this machine's natives (unless FRESHEN_SKIP_STAGE=1 because CI already gathered
# every platform under bindings/dotnet/runtimes/) and builds the NuGet package into
# bindings/dotnet/artifacts/.
#
#   Tools/pack.sh
#   FRESHEN_SKIP_STAGE=1 Tools/pack.sh -p:FreshenRequiredRuntimes="win-x64;linux-x64;osx-arm64;osx-x64"
#
# Extra arguments go to `dotnet pack`. Set DOTNET to use a specific dotnet executable.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
dotnet=${DOTNET:-dotnet}

if [ "${FRESHEN_SKIP_STAGE:-}" != "1" ]; then
  sh "$here/Tools/stage-native.sh"
fi
"$dotnet" pack "$here/src/Freshen/Freshen.csproj" -c Release -o "$here/artifacts" "$@"

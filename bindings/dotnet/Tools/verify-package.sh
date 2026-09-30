#!/bin/sh
# Verifies a built Freshen NuGet package two ways.
#
# 1. Inspects the archive: the assembly, its documentation, the README, the license and
#    repository metadata, and, for every runtime identifier present, that BOTH the
#    native library and the freshen-helper executable are there under runtimes/<rid>/native.
# 2. Consumes it: creates a separate scratch application that references the package
#    from a local-only feed (no nuget.org), runs it, and checks that the native library
#    loads WITHOUT the FRESHEN_FFI_LIBRARY override (so it really came from the package)
#    and that the bundled helper is found and executable. Done for a portable build and
#    for a RID-specific publish.
#
#   Tools/verify-package.sh artifacts/Freshen.0.1.0-preview.1.nupkg
#
# The scratch application is left in place (its path is printed) so a failure can be
# inspected; CI runners are ephemeral. Set DOTNET to use a specific dotnet executable.
set -eu

package=${1:?usage: verify-package.sh path/to/Freshen.<version>.nupkg}
dotnet=${DOTNET:-dotnet}
package=$(cd "$(dirname "$package")" && pwd)/$(basename "$package")
feed=$(dirname "$package")

echo "== inspecting $(basename "$package")"
python3 - "$package" <<'PY'
import re, sys, zipfile

path = sys.argv[1]
names = zipfile.ZipFile(path).namelist()
nuspec_name = next((n for n in names if n.endswith(".nuspec")), None)
problems = []
if nuspec_name is None:
    problems.append("no .nuspec")
else:
    nuspec = zipfile.ZipFile(path).read(nuspec_name).decode()
    for label, pattern in [("license MIT", r"<license[^>]*>MIT</license>"),
                           ("repository url", r"<repository[^>]*url="),
                           ("readme", r"<readme>README\.md</readme>")]:
        if not re.search(pattern, nuspec):
            problems.append("nuspec is missing " + label)
for required in ["lib/net8.0/Freshen.dll", "lib/net8.0/Freshen.xml", "README.md"]:
    if required not in names:
        problems.append("missing " + required)

expected = {"linux": ("libfreshen_ffi.so", "freshen-helper"),
            "osx": ("libfreshen_ffi.dylib", "freshen-helper"),
            "win": ("freshen_ffi.dll", "freshen-helper.exe")}
rids = sorted({n.split("/")[1] for n in names if n.startswith("runtimes/") and n.count("/") >= 3})
if not rids:
    problems.append("no runtimes/<rid>/native content: the package cannot load")
for rid in rids:
    library, helper = expected[rid.split("-")[0]]
    for wanted in (library, helper):
        if f"runtimes/{rid}/native/{wanted}" not in names:
            problems.append(f"runtimes/{rid}/native/{wanted} is missing")
print("runtimes:", ", ".join(rids) or "(none)")
if problems:
    print("PROBLEMS:\n  " + "\n  ".join(problems))
    sys.exit(1)
print("archive contents ok")
PY

version=$(python3 - "$package" <<'PY'
import re, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
n = next(x for x in z.namelist() if x.endswith(".nuspec"))
print(re.search(r"<version>([^<]+)</version>", z.read(n).decode()).group(1))
PY
)

case "$(uname -s)" in
  Linux)  os=linux ;;
  Darwin) os=osx ;;
  MINGW*|MSYS*|CYGWIN*) os=win ;;
  *) echo "verify-package.sh: cannot consume on $(uname -s); archive check only"; exit 0 ;;
esac
case "$(uname -m)" in
  x86_64|amd64|AMD64) arch=x64 ;;
  arm64|aarch64|ARM64) arch=arm64 ;;
  *) echo "unknown architecture; archive check only"; exit 0 ;;
esac
rid=${FRESHEN_RID:-$os-$arch}

if ! python3 - "$package" "$rid" <<'PY'
import sys, zipfile
names = zipfile.ZipFile(sys.argv[1]).namelist()
sys.exit(0 if any(n.startswith(f"runtimes/{sys.argv[2]}/native/") for n in names) else 1)
PY
then
  echo "package has no natives for this machine ($rid); archive check only"
  exit 0
fi

echo "== consuming $version as a separate application ($rid)"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/freshen-verify-XXXXXX")
cat >"$scratch/NuGet.config" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="local" value="$feed" />
  </packageSources>
</configuration>
EOF
cat >"$scratch/Consumer.csproj" <<EOF
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Freshen" Version="$version" />
  </ItemGroup>
</Project>
EOF
cat >"$scratch/Program.cs" <<'EOF'
using Freshen;

// A stand-in application. Everything it learns comes from the NuGet package.
Console.WriteLine($"abi={FreshenHost.NativeAbiVersion}");
Console.WriteLine($"library={FreshenHost.NativeLibraryVersion}");
string? helper = FreshenHost.FindBundledHelper();
Console.WriteLine($"helper={helper ?? "(none)"}");
if (FreshenHost.NativeAbiVersion != 1) return 1;
if (helper is null || !File.Exists(helper)) return 2;
if (!OperatingSystem.IsWindows() && (File.GetUnixFileMode(helper) & UnixFileMode.UserExecute) == 0) return 3;
Console.WriteLine("consumer ok");
return 0;
EOF

echo "-- portable build (dotnet run)"
env -u FRESHEN_FFI_LIBRARY "$dotnet" run --project "$scratch/Consumer.csproj" --configfile "$scratch/NuGet.config"
echo "-- RID-specific publish ($rid)"
"$dotnet" publish "$scratch/Consumer.csproj" -c Release -r "$rid" --self-contained false -o "$scratch/publish" --configfile "$scratch/NuGet.config" >/dev/null
env -u FRESHEN_FFI_LIBRARY "$dotnet" "$scratch/publish/Consumer.dll"

echo "verify-package: ok (scratch application left at $scratch)"

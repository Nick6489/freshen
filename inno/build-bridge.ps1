$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    foreach ($target in @('i686-pc-windows-msvc', 'x86_64-pc-windows-msvc')) {
        # Static CRT for this build only; no machine-specific Cargo configuration.
        $priorFlags = $env:RUSTFLAGS
        try {
            $env:RUSTFLAGS = '-C target-feature=+crt-static'
            cargo build --locked --release -p freshen-inno-bridge --target $target
            if ($LASTEXITCODE -ne 0) { throw "Bridge build failed: $target" }
        } finally { $env:RUSTFLAGS = $priorFlags }
    }
} finally { Pop-Location }

#!/usr/bin/env bash
# Cross-build the Windows exe from WSL using rustc's bundled lld and the
# MSVC/Windows SDK libraries already installed on the Windows side.
# On Windows itself, plain `cargo build --release` is enough.
set -euo pipefail

vc=$(ls -d "/mnt/c/Program Files (x86)/Microsoft Visual Studio"/*/*/VC/Tools/MSVC/*/lib/x64 | sort -V | tail -1)
sdk=$(ls -d "/mnt/c/Program Files (x86)/Windows Kits/10/Lib"/*/ | sort -V | tail -1)
lld="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld"

sep=$'\x1f'
export CARGO_ENCODED_RUSTFLAGS="-Clinker=${lld}${sep}-Clinker-flavor=lld-link${sep}-Lnative=${vc}${sep}-Lnative=${sdk}um/x64${sep}-Lnative=${sdk}ucrt/x64"
exec cargo "${@:-build}" --target x86_64-pc-windows-msvc

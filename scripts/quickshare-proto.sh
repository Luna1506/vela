#!/usr/bin/env bash
# Regenerate src/quickshare/proto/generated/*.rs from the .proto files.
# Maintainer-only: the generated code is checked in, so building vela never
# needs protoc. Requires nix (for protoc) and network (for prost-build).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
proto="$root/src/quickshare/proto"
out="$proto/generated"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/src"
cat > "$work/Cargo.toml" <<'T'
[package]
name = "gen"
version = "0.0.0"
edition = "2024"

[dependencies]
prost-build = "0.14"
T
cat > "$work/src/main.rs" <<'R'
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let protos: Vec<&str> = a[3..].iter().map(String::as_str).collect();
    prost_build::Config::new()
        .out_dir(&a[2])
        .include_file("mod.rs")
        .compile_protos(&protos, &[&a[1]])
        .unwrap();
}
R

rm -rf "$out"
mkdir -p "$out"
files=()
for f in "$proto"/*.proto; do files+=("$(basename "$f")"); done
nix shell nixpkgs#protobuf nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc -c bash -c '
  cd "$1" && shift
  PROTOC="$(command -v protoc)" cargo run -q -- "$@"
' _ "$work" "$proto" "$out" "${files[@]}"
echo "generated into $out"

#!/bin/sh
# In wine/mac-build.sh's container: gexc for Linux x86-64, built from this
# tree (bind-mounted at /gex) into the cache volume, with Rust there too (the
# image has none; rust-toolchain.toml says which). Prints its path. wine/dist.sh
# runs a WoW64 install's i386 prefix pass with it (WINE64_GEXC) where the
# container's x86-64 is Rosetta's, which runs no 32-bit code.
set -eu
V=/root/.cache/gex
export RUSTUP_HOME=$V/rust/rustup CARGO_HOME=$V/rust/cargo
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
	curl -sSf --max-time 300 https://sh.rustup.rs -o "$V/rustup-init.sh"
	sh "$V/rustup-init.sh" -y --default-toolchain none --profile minimal --no-modify-path >&2
	rm -f "$V/rustup-init.sh"
fi
cd /gex
PATH=$CARGO_HOME/bin:$PATH nice -n 10 cargo build -q --release -p gexc --features x86 -j "${JOBS:-3}" --target-dir "$V/gexc-x86-linux" >&2
echo "$V/gexc-x86-linux/release/gexc"

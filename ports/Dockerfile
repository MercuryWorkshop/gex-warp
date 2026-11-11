# The Linux image page/runtime's replay and X server are built in on a Mac (the
# scripts assume a Linux host: /usr/include/X11, xkbcomp, GNU nproc, stat -c).
# From the tree's root:
#
#   docker build -t gex-wasm-build -f ports/Dockerfile ports
#   mkdir -p page/public/gex     # with the shim, kernel and seed copied in
#   docker run --rm -v "$PWD":/gex -w /gex -e HOME=/root -e TMPDIR=/gex/build-logs/tmp \
#       -e JOBS=4 gex-wasm-build sh -c 'git config --global --add safe.directory "*"; \
#       sh ports/mesa/build.sh /gex/build-logs/mesa-out && sh ports/xwayland/build.sh /gex/build-logs/xwl-out'
#
# (--cpuset-cpus limits what nproc, and so the Xwayland build, uses.) emsdk is
# toolchain/pins' EMSCRIPTEN_VERSION and Rust rust-toolchain.toml's.
FROM debian:trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential git curl ca-certificates xz-utils bzip2 unzip python3 python3-pip python3-venv python3-mako python3-yaml python3-ply python3-packaging \
    meson ninja-build cmake pkg-config autoconf automake libtool bison flex gettext \
    x11proto-dev libx11-dev xutils-dev libxshmfence-dev xkb-data x11-xkb-utils libwayland-dev wayland-protocols libwayland-bin \
    linux-libc-dev nodejs libxml2-utils file && rm -rf /var/lib/apt/lists/*
# libxfont2 and libfontenc's autoreconf want font-util's macros, which Debian does not ship.
RUN git clone -q --depth 1 --branch font-util-1.4.1 https://gitlab.freedesktop.org/xorg/font/util.git /tmp/fu \
    && cd /tmp/fu && autoreconf -fi && ./configure --prefix=/usr && make install && rm -rf /tmp/fu
RUN git clone --depth 1 https://github.com/emscripten-core/emsdk /opt/emsdk && cd /opt/emsdk && ./emsdk install 6.0.1 && ./emsdk activate 6.0.1
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.96.0 -t wasm32-unknown-unknown
ENV EMSDK=/opt/emsdk PATH=/root/.cargo/bin:/opt/emsdk:/opt/emsdk/upstream/emscripten:$PATH

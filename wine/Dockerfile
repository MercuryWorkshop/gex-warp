# The Linux x86-64 image make wine / make wine-x64 are built in on a Mac
# (wine/dist.sh assumes a Linux host: GNU tools, MinGW-w64 GCC 12, LLVM 22 with
# its development files for the sysroot's plugins, and an x86-64 Unix side
# that wineboot runs natively to make the prefix). From the tree's root:
#
#   wine/mac-build.sh [x64|wow64]     builds the image and runs the build in it
#
# or by hand:
#
#   docker build --platform linux/amd64 -t gex-wine-build -f wine/Dockerfile wine
#   docker run --rm --platform linux/amd64 -v "$PWD":/gex -v ~/.cache/gex:/root/.cache/gex -w /gex \
#       -e JOBS=6 gex-wine-build make wine-x64
FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential git curl ca-certificates gnupg xz-utils bzip2 unzip python3 python3-pip python3-venv \
    meson ninja-build cmake pkg-config autoconf automake libtool bison flex gettext \
    mingw-w64 gcc-mingw-w64 g++-mingw-w64 nodejs file rsync procps libfreetype-dev \
    linux-libc-dev && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-22 main" > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && apt-get install -y --no-install-recommends clang-22 lld-22 llvm-22-dev libclang-22-dev libclang-common-22-dev \
    && rm -rf /var/lib/apt/lists/* \
    && for t in clang clang++ wasm-ld llvm-ar llvm-ranlib llvm-config llvm-nm llvm-objcopy llvm-strip llvm-objdump llvm-readobj llvm-dis ld.lld; do ln -sf /usr/bin/$t-22 /usr/local/bin/$t; done
# compiler-rt's builtins sources: toolchain/pins' EMSCRIPTEN_VERSION's (GEX_CRT_SRC)
RUN git clone --depth 1 https://github.com/emscripten-core/emsdk /opt/emsdk && cd /opt/emsdk && ./emsdk install 6.0.1 \
    && mkdir -p /opt/crt && cp -r /opt/emsdk/upstream/emscripten/system/lib/compiler-rt/lib/builtins /opt/crt/builtins \
    && rm -rf /opt/emsdk /root/.emscripten*
ENV GEX_CRT_SRC=/opt/crt/builtins HOME=/root
RUN git config --global --add safe.directory '*'
# the EGL and KHR headers wine/deps32.sh and deps64.sh copy
RUN apt-get update && apt-get install -y --no-install-recommends libegl-dev && rm -rf /var/lib/apt/lists/*

FROM debian:trixie-slim

# Kernel build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    flex \
    bison \
    bc \
    libncurses-dev \
    libssl-dev \
    libelf-dev \
    dwarves \
    # General tools
    curl \
    ca-certificates \
    tar \
    cpio \
    gzip \
    xz-utils \
    # UKI building
    systemd-ukify \
    systemd-boot-efi \
    # ISO building
    isolinux \
    syslinux-common \
    xorriso \
    # Rust musl cross-compilation
    musl-tools \
    # gRPC/protobuf
    protobuf-compiler \
    # Debian packaging
    debhelper \
    && rm -rf /var/lib/apt/lists/*

# Install Rust system-wide
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --no-modify-path && \
    rustup target add x86_64-unknown-linux-musl && \
    chmod -R a+rw /usr/local/rustup /usr/local/cargo

# Tell cc-rs to use musl-gcc for musl target
ENV CC_x86_64_unknown_linux_musl=musl-gcc

# Limit parallel jobs to reduce memory usage
ENV CARGO_BUILD_JOBS=2

WORKDIR /work

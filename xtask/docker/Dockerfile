# The environment of the gate groups `linux` and `aarch64`
# (`xtask/src/main.rs`, `Step::docker`): an x86_64 image, so that rustc and
# cargo run natively; only the AArch64 test binaries, and the `ns-next` they
# start, run emulated, by the QEMU this image builds (`docs/design/
# execution.md`, "Tests"). Everything it installs is pinned: the base image
# by its digest, the Ubuntu packages by the archive's snapshot, whose
# signature apt checks; QEMU, rustup-init and cargo-nextest by their
# versions and the SHA-256 of what is downloaded; and Rust by its version,
# installed by that rustup-init, which checks every component it downloads
# against the release's signed manifest.

# The base image and the moment of the Ubuntu archive every package comes
# from. An update changes the digest and the snapshot together, deliberately,
# and is checked by the gates like any other change.
ARG BASE=ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c
ARG SNAPSHOT=20261104T000000Z

# The base both stages start from: apt pointed at the snapshot.
FROM ${BASE} AS base
ARG SNAPSHOT
# No prompt from a package's configuration stops the build.
ARG DEBIAN_FRONTEND=noninteractive
# Every `apt-get update` and `install` reads the archive as it was at the
# snapshot (apt's `--snapshot`, set once for every command).
RUN echo "APT::Snapshot \"${SNAPSHOT}\";" > /etc/apt/apt.conf.d/50snapshot
# The snapshot service is HTTPS only, and the base image has no CA
# certificates, so the first install, ca-certificates alone, does not verify
# the server: apt authenticates every index and package it fetches by the
# archive's signature (the image's ubuntu-archive-keyring) regardless, so
# what it installs is the snapshot's; every later fetch verifies the server
# too.
RUN apt-get --option Acquire::https::Verify-Peer=false update \
    && apt-get --option Acquire::https::Verify-Peer=false install --yes \
        --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# QEMU's user-mode emulator for AArch64, built from the release tarball: the
# newest stable release, where Ubuntu's package lags behind. 8.1 is the
# oldest that runs the suite: 6.2 ran stale code written through the code
# zone's second mapping and ran out of memory on the heap's reservations.
FROM base AS qemu
# An ARG does not carry into a derived stage.
ARG DEBIAN_FRONTEND=noninteractive
# An update changes the version and the checksum together. The checksum is
# that of the tarball whose `.sig` verifies against the QEMU release key
# CEACC9E15534EBABB82D3FA03353C9CEF108B584.
ARG QEMU_VERSION=11.1.3
ARG QEMU_SHA256=5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22
# curl fetches the tarball and xz-utils unpacks it; gcc, make, ninja-build,
# pkg-config and python3 with venv are QEMU's build (meson, which the
# tarball bundles, runs in a virtual environment); libglib2.0-dev is the one
# library user mode needs, its static archives included.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        curl \
        xz-utils \
        gcc \
        libc6-dev \
        make \
        ninja-build \
        pkg-config \
        python3 \
        python3-venv \
        libglib2.0-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /qemu
# Only `qemu-aarch64`, statically linked so that it needs nothing of the
# final image, with every optional feature off.
RUN curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location \
        --output qemu.tar.xz https://download.qemu.org/qemu-${QEMU_VERSION}.tar.xz \
    && echo "${QEMU_SHA256}  qemu.tar.xz" | sha256sum --check --strict \
    && tar --extract --xz --file qemu.tar.xz --strip-components 1 \
    && ./configure --target-list=aarch64-linux-user --static \
        --without-default-features --disable-docs \
    && make -j"$(nproc)" \
    && strip build/qemu-aarch64

# The image the gates run in.
FROM base
ARG DEBIAN_FRONTEND=noninteractive
# curl fetches rustup and cargo-nextest; gcc is the linker for
# x86_64-unknown-linux-gnu and libc6-dev its C runtime;
# gcc-aarch64-linux-gnu links aarch64-unknown-linux-gnu, and
# libc6-dev-arm64-cross gives it the target's C runtime and gives QEMU the
# target's dynamic loader and libraries under /usr/aarch64-linux-gnu
# (`QEMU_LD_PREFIX`); openjdk-25-jdk-headless, the newest LTS the snapshot
# has, compiles and runs the benchmarks' Java counterparts; rsync copies
# the checkout into the working copy; git lists the tracked sources for
# `source_formatting`.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        curl \
        gcc \
        libc6-dev \
        gcc-aarch64-linux-gnu \
        libc6-dev-arm64-cross \
        openjdk-25-jdk-headless \
        rsync \
        git \
    && rm -rf /var/lib/apt/lists/*

# The `aarch64` group's runner, `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER`.
COPY --from=qemu /qemu/build/qemu-aarch64 /usr/local/bin/qemu-aarch64

# rustup and cargo in a fixed place outside a home directory, on the PATH of
# every `docker run`.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# The workspace's toolchain (`rust-toolchain.toml`) with the components it
# names, so that rustup downloads nothing at run time, and both targets the
# groups build for; rustup-init itself a fixed release. An update changes
# the version and the checksum together; the checksum is the one the
# release publishes beside the binary, `rustup-init.sha256` under
# static.rust-lang.org/rustup/archive/<version>/x86_64-unknown-linux-gnu/.
ARG RUSTUP_VERSION=1.29.3
ARG RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
RUN curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location \
        --output /tmp/rustup-init \
        https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/x86_64-unknown-linux-gnu/rustup-init \
    && echo "${RUSTUP_SHA256}  /tmp/rustup-init" | sha256sum --check --strict \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain 1.98.0 \
        --component rustfmt,clippy \
        --target x86_64-unknown-linux-gnu,aarch64-unknown-linux-gnu \
    && rm /tmp/rustup-init

# cargo-nextest runs the suites; a pinned release, prebuilt, so that the image
# does not compile it. An update changes the version and the checksum
# together; the checksum is the one the GitHub release publishes beside the
# tarball, `cargo-nextest-<version>-x86_64-unknown-linux-gnu.sha256`.
ARG NEXTEST_VERSION=0.9.150
ARG NEXTEST_SHA256=b0959bc64232974224cf9fb22c915845398c0e8668fa7831eb5b43fb1eef781f
RUN curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location \
        --output /tmp/nextest.tar.gz \
        https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-x86_64-unknown-linux-gnu.tar.gz \
    && echo "${NEXTEST_SHA256}  /tmp/nextest.tar.gz" | sha256sum --check --strict \
    && tar --extract --gzip --file /tmp/nextest.tar.gz --directory /usr/local/cargo/bin \
    && rm /tmp/nextest.tar.gz

# The working copy is rsynced from a checkout owned by another user; git
# would otherwise refuse to read its repository.
RUN git config --system --add safe.directory '*'

WORKDIR /work

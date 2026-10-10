# Minimal native-codegen environment: the native-pipeline subset of the course
# image (lo-testing/docker/Dockerfile): same base, as/ld/gcc, and Rust for the
# runtime skeleton. amd64 by design (runs under Rosetta/QEMU on ARM Macs).
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential binutils gcc git curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal \
    && rustc --version
WORKDIR /work

FROM ubuntu:24.04

ARG RUST_VERSION=1.88.0
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl git jackd2 jack-tools \
    libasound2-dev libjack-jackd2-dev libudev-dev pkg-config python3 xz-utils \
 && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION"
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
COPY . .
RUN freewheeling-plus/scripts/linux/validate.sh && \
    cargo test --manifest-path freewheeling-plus/Cargo.toml --locked --test linux_virtual_acceptance
CMD ["freewheeling-plus/scripts/linux/run-virtual-acceptance.sh"]

# Acceptance image for the Linux/JACK virtual workflow.
#
# The base tag is pinned to a digest so identical sources cannot resolve to
# different base layers over time; bump it deliberately (see the digest in the
# build log of `docker pull ubuntu:24.04`).
FROM ubuntu:24.04@sha256:496754492fb28b4d3049432f2ca787449331e23fb14f0dd3fffea86bf5a93eb4

ARG RUST_VERSION=1.88.0
ENV DEBIAN_FRONTEND=noninteractive
RUN case "$RUST_VERSION" in \
      1.88.0) ;; \
      *) echo "RUST_VERSION must be the pinned 1.88.0, got '$RUST_VERSION'" >&2; exit 1 ;; \
    esac
# The archive keeps moving, so an unpinned build can install a newer
# clang/cmake/jackd2 without any source change. For a reproducible run set
# UBUNTU_SNAPSHOT to a snapshot.ubuntu.com timestamp (e.g. 20250601T000000Z):
# the sources are rewritten to that snapshot, and the CA bundle is installed
# first because the snapshot is fetched over HTTPS.
ARG UBUNTU_SNAPSHOT=
RUN if [ -n "$UBUNTU_SNAPSHOT" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends ca-certificates \
      && rm -rf /var/lib/apt/lists/*; \
      for file in /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources; do \
        [ -f "$file" ] || continue; \
        sed -i \
          -e "s|http://archive.ubuntu.com/ubuntu|https://snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT|g" \
          -e "s|http://security.ubuntu.com/ubuntu|https://snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT|g" \
          "$file"; \
      done; \
      grep -q "snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT" /etc/apt/sources.list.d/ubuntu.sources || { \
        echo "error: cannot point apt at snapshot $UBUNTU_SNAPSHOT" >&2; exit 1; }; \
    fi
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl git jackd2 jack-tools \
    libasound2-dev libjack-jackd2-dev libudev-dev pkg-config python3 xz-utils \
 && rm -rf /var/lib/apt/lists/*
# rustup-init is downloaded to a file and executed only after its checksum is
# verified: piping a remote script straight into a root shell would execute
# whatever a compromised mirror or CDN serves.
# SHA-256 of the rustup-init release the URL above serves (checked against
# rustup-init.sha256 next to it); update both together when rustup moves on.
ARG RUSTUP_INIT_SHA256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71
RUN curl --proto '=https' --tlsv1.2 -sSfO \
      https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init \
 && echo "${RUSTUP_INIT_SHA256}  rustup-init" | sha256sum -c - \
 && chmod +x rustup-init \
 && ./rustup-init -y --profile minimal --default-toolchain "$RUST_VERSION" --no-modify-path \
 && rm rustup-init
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
# Copy the crate itself, one path at a time. The build context is the *parent*
# directory (the Dockerfile is `freewheeling-plus/scripts/linux/Dockerfile` and
# refers to sibling paths), so `COPY . .` would also pull in whatever local
# state sits next to the crate - including `freewheeling-plus/.reasonix/`, which
# holds an access token. `.dockerignore` next to this file only applies when the
# crate directory itself is the context, so it cannot be relied on here.
COPY freewheeling-plus/Cargo.toml freewheeling-plus/Cargo.lock /src/freewheeling-plus/
COPY freewheeling-plus/build.rs /src/freewheeling-plus/
COPY freewheeling-plus/.cargo /src/freewheeling-plus/.cargo
COPY freewheeling-plus/src /src/freewheeling-plus/src
COPY freewheeling-plus/data /src/freewheeling-plus/data
COPY freewheeling-plus/docs /src/freewheeling-plus/docs
COPY freewheeling-plus/scripts /src/freewheeling-plus/scripts
COPY freewheeling-plus/tests /src/freewheeling-plus/tests
COPY freewheeling-plus/fixtures /src/freewheeling-plus/fixtures
COPY freewheeling-plus/.github /src/freewheeling-plus/.github
COPY freewheeling-plus/FEATURE_MATRIX.md freewheeling-plus/PACKAGING.md \
     freewheeling-plus/LINUX_PACKAGING.md freewheeling-plus/README.md /src/freewheeling-plus/
# Build the artifacts the container's CMD needs (`--features jack` for the
# acceptance binary) so a network-isolated `docker run` does not have to
# compile or fetch anything.
RUN freewheeling-plus/scripts/linux/validate.sh && \
    cargo build --manifest-path freewheeling-plus/Cargo.toml --locked --features jack \
      --bin realtime_acceptance && \
    cargo test --manifest-path freewheeling-plus/Cargo.toml --locked --features jack \
      --test linux_virtual_acceptance
# Run as an unprivileged user: the acceptance path spawns jackd and the app the
# way an operator would, and root would mask permission-related failures.
RUN useradd --create-home --uid 1000 builder && chown -R builder:builder /src
USER builder
CMD ["/bin/sh", "/src/freewheeling-plus/scripts/linux/run-virtual-acceptance.sh"]

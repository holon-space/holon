FROM mcr.microsoft.com/devcontainers/base:bookworm

ARG USERNAME=vscode

# System build deps for the workspace:
# - libsqlite3 / libssl / pkg-config / cmake / clang / lld  → rust crates (libsql, reqwest, ring, wgpu-deps)
# - protobuf-compiler                                       → opentelemetry-otlp
# - libudev / libusb                                        → occasional hw-adjacent deps
# - mold                                                    → faster linking
# GPUI/wgpu runtime libs are intentionally omitted — this container is headless.
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential \
      pkg-config \
      cmake \
      clang \
      lld \
      mold \
      libssl-dev \
      libsqlite3-dev \
      protobuf-compiler \
      libudev-dev \
      libusb-1.0-0-dev \
      git \
      curl \
      ca-certificates \
      xz-utils \
      unzip \
      jq \
      ripgrep \
      fd-find \
    && rm -rf /var/lib/apt/lists/*

# Node.js (for Claude Code CLI and other JS tooling)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# No jj / no git — by design. The container's .jj and .git are masked with tmpfs
# (see devcontainer.json), so VCS commands would fail anyway. All version control
# happens on the host. Keeping the image lean.

USER ${USERNAME}

# Rust via rustup. No channel specified — rust-toolchain.toml at the repo root
# pins nightly-2026-03-28 and rustup will honor it on first cargo invocation.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain none --profile minimal --no-modify-path
ENV PATH=/home/${USERNAME}/.cargo/bin:${PATH}

# Pre-warm the pinned toolchain + components so `cargo` is fast on first use.
# The build context is the repo root (set in devcontainer.json) so this COPY resolves.
COPY --chown=${USERNAME}:${USERNAME} rust-toolchain.toml /tmp/rust-toolchain.toml
RUN cd /tmp \
    && rustup toolchain install \
    && TOOLCHAIN="$(rustup show active-toolchain | awk '{print $1}')" \
    && rustup default "$TOOLCHAIN" \
    && rustup component add rust-src rust-analyzer clippy rustfmt \
    && rm /tmp/rust-toolchain.toml

# Dev tools used by the repo's workflows. Failing here should fail the build,
# not silently produce a half-equipped image.
RUN cargo install --locked cargo-nextest

# Claude Code CLI — installed into a user-local npm prefix so no sudo is needed.
ENV NPM_CONFIG_PREFIX=/home/${USERNAME}/.npm-global
ENV PATH=/home/${USERNAME}/.npm-global/bin:${PATH}
RUN mkdir -p "$NPM_CONFIG_PREFIX" \
    && npm install -g @anthropic-ai/claude-code

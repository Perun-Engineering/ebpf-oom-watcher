# syntax=docker/dockerfile:1
#
# Single multi-stage build for the eBPF OOM Watcher.
#   --target dev      interactive toolchain shell (used by docker-compose.yml)
#   --target runtime  minimal production image (default, used by release CI)
#
# Nightly is pinned to match CI (.github/workflows/pr-checks.yml).

# ── Shared build base: Rust nightly + eBPF toolchain ─────────────────
FROM rustlang/rust:nightly AS base
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang \
        llvm \
        libelf-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install bpf-linker --locked && rustup component add rust-src
ENV CARGO_TARGET_BPFEL_UNKNOWN_NONE_LINKER=bpf-linker
WORKDIR /workspace

# ── Builder: compile the optimized release binary ────────────────────
FROM base AS builder
COPY . .
# 'rustup update nightly' first: the base layer may be GHA-cached with an older
# nightly, and build.rs's inner 'cargo +nightly build' otherwise syncs to a
# newer nightly on its own — one that lacks rust-src, breaking '-Z build-std=core'.
# Updating here pulls that same latest nightly and adds rust-src to it, so the
# inner build finds the toolchain current and the component present (no re-sync).
RUN rustup update nightly \
    && rustup component add rust-src --toolchain nightly \
    && cargo build --release --package oom-watcher --jobs 1

# ── Dev: interactive shell with the full toolchain + bpftool ─────────
# docker-compose mounts the workspace and builds/runs the binary here.
FROM base AS dev
RUN apt-get update && apt-get install -y --no-install-recommends \
        bpftool \
        git \
    && rm -rf /var/lib/apt/lists/*
CMD ["bash"]

# ── Runtime: minimal production image (default target) ───────────────
FROM ubuntu:26.04 AS runtime
# No libssl needed: the binary uses rustls, not OpenSSL.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r oomwatcher \
    && useradd -r -g oomwatcher oomwatcher
COPY --from=builder /workspace/target/release/oom-watcher /usr/local/bin/oom-watcher
RUN mkdir -p /etc/oom-watcher
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/metrics || exit 1
# eBPF program loading requires root.
USER root
ENTRYPOINT ["/usr/local/bin/oom-watcher"]

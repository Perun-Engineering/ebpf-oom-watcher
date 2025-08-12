# oom-watcher

An eBPF-based Out-of-Memory (OOM) event monitor that uses kernel probes to detect and log OOM kill events in real-time.

## Quick Start with Docker (Recommended)

The easiest way to build and develop this project is using Docker, which provides a consistent Linux environment with all required dependencies.

### Prerequisites

- Docker installed on your system
- Git (to clone the repository)

### Building with Docker

1. **Clone the repository:**
   ```bash
   git clone <your-repo-url>
   cd ebpf-oom-watcher
   ```

2. **Build the Docker image:**
   ```bash
   docker build -t ebpf-oom-watcher .
   ```

3. **Build the project:**
   ```bash
   docker run --rm -it -v "$PWD":/workspace -w /workspace ebpf-oom-watcher bash -c "
   rustup toolchain install nightly --component rust-src &&
   rustup default nightly &&
   cargo build --release
   "
   ```

4. **Run the OOM watcher:**
   ```bash
   docker run --rm -it --privileged -v "$PWD":/workspace -w /workspace ebpf-oom-watcher ./target/release/oom-watcher
   ```

### Development Workflow

For interactive development, you can start a persistent container:

```bash
# Start a development container
docker run -it --privileged -v "$PWD":/workspace -w /workspace ebpf-oom-watcher bash

# Inside the container, set up the toolchain (first time only)
rustup toolchain install nightly --component rust-src
rustup default nightly

# Build the project
cargo build --release

# Run the program
./target/release/oom-watcher
```

### Testing OOM Events

To test the OOM watcher, you can trigger OOM conditions using the provided script:

```bash
# In the container or on a Linux system
python3 scripts/trigger_oom.py
```

## Local Development Environment (Linux Only)

If you prefer to develop directly on a Linux system without Docker:

### Prerequisites

1. **Rust toolchains:**
   ```bash
   rustup toolchain install stable
   rustup toolchain install nightly --component rust-src
   ```

2. **eBPF target:**
   ```bash
   rustup target add bpfel-unknown-none --toolchain nightly
   ```

3. **Additional components:**
   ```bash
   rustup component add llvm-tools-preview --toolchain nightly
   ```

4. **BPF linker:**
   ```bash
   cargo install bpf-linker
   ```

### Build & Run

```bash
# Build the project
cargo build --release

# Run with sudo (required for eBPF programs)
sudo ./target/release/oom-watcher
```

## Cross-compiling on macOS

Cross compilation should work on both Intel and Apple Silicon Macs.

### Prerequisites

1. LLVM: `brew install llvm`
2. C toolchain: `brew install filosottile/musl-cross/musl-cross`
3. Rust targets:
   ```bash
   rustup target add ${ARCH}-unknown-linux-musl
   ```

### Build Command

```bash
CC=${ARCH}-linux-musl-gcc cargo build --package oom-watcher --release \
  --target=${ARCH}-unknown-linux-musl \
  --config=target.${ARCH}-unknown-linux-musl.linker=\"${ARCH}-linux-musl-gcc\"
```

The cross-compiled program `target/${ARCH}-unknown-linux-musl/release/oom-watcher` can be copied to a Linux server or VM and run there.

## Project Structure

```
ebpf-oom-watcher/
├── oom-watcher/           # Userland application
├── oom-watcher-ebpf/      # eBPF kernel program
├── oom-watcher-common/    # Shared data structures
├── scripts/               # Utility scripts
├── Dockerfile            # Docker build environment
└── README.md             # This file
```

## Architecture

- **eBPF Program**: Attaches to the `oom_kill_process` kernel function using a kprobe
- **Userland Program**: Loads the eBPF program and reads OOM events via PerfEventArray
- **Event Structure**: Captures process details including PID, memory usage, and process name
- **Async Processing**: Handles events from multiple CPUs concurrently using Tokio

## Troubleshooting

### Common Issues

1. **Permission denied**: eBPF programs require root privileges or appropriate capabilities
2. **Kernel version**: Requires a recent Linux kernel with eBPF support (4.4+)
3. **Memory constraints**: Large Rust builds may require sufficient memory/swap

### Docker Issues

- **SIGBUS errors**: Try increasing Docker's memory limits:
  ```bash
  docker run --memory=4g --memory-swap=8g ...
  ```
- **Build failures**: Ensure you're using the nightly toolchain with rust-src component

## License

This project is licensed under the terms of the [MIT license].

[MIT license]: LICENSE-MIT

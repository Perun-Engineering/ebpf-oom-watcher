# eBPF OOM Watcher

An eBPF-based Out-of-Memory (OOM) event monitor for Kubernetes that captures OOM events with pod and container context, exposing detailed metrics via Prometheus.

## Features

- **eBPF-based OOM Detection**: Uses kernel tracepoints to capture OOM events in real-time
- **Kubernetes Integration**: Automatically identifies pods and containers where OOMs occur
- **Prometheus Metrics**: Comprehensive metrics for monitoring and alerting
- **DaemonSet Deployment**: Runs on all nodes to provide cluster-wide OOM visibility
- **Multi-Architecture**: Supports AMD64 and ARM64 platforms

## Quick Start

### Kubernetes Deployment

```bash
# Build and push image
docker build -f Dockerfile.production -t ghcr.io/perun-engineering/ebpf-oom-watcher:latest .
docker push ghcr.io/perun-engineering/ebpf-oom-watcher:latest

# Deploy using kubectl
kubectl apply -f k8s/daemonset.yaml

# Or using Helm
helm install oom-watcher helm/oom-watcher \
  --set image.tag=latest \
  --set serviceMonitor.enabled=true
```

### Local Development

```bash
# Set up development environment
./scripts/setup-dev.sh

# Build and test
./scripts/build-and-test.sh

# Run locally (requires Linux and root privileges)
sudo ./target/release/oom-watcher
```

## Metrics

The OOM Watcher exposes the following Prometheus metrics on port 8080:

- `oom_kills_total{node, namespace, pod, container}` - Total number of OOM kills
- `oom_kills_per_node_total{node}` - Total OOM kills per node
- `oom_memory_usage_bytes{node, namespace, pod, container, memory_type}` - Memory usage at OOM time
- `oom_last_timestamp{node, namespace, pod, container}` - Timestamp of last OOM event

### Example Queries

```promql
# OOM rate across cluster
rate(oom_kills_total[5m])

# OOM kills by namespace
sum by (namespace) (oom_kills_total)

# Memory usage at OOM by type
oom_memory_usage_bytes{memory_type="anon_rss"}
```

## Configuration

### Environment Variables

- `NODE_NAME`: Kubernetes node name (automatically set by DaemonSet)
- `METRICS_PORT`: Port for Prometheus metrics (default: 8080)
- `RUST_LOG`: Log level (default: info)

### Helm Chart Values

See [helm/oom-watcher/values.yaml](helm/oom-watcher/values.yaml) for all configuration options.

## Development

### Prerequisites

- Docker (recommended) or Linux with eBPF support
- Rust 1.75+ with nightly toolchain
- kubectl and Helm for Kubernetes deployment

### Setup

```bash
git clone https://github.com/Perun-Engineering/ebpf-oom-watcher.git
cd ebpf-oom-watcher
./scripts/setup-dev.sh
```

### Local Testing

```bash
# Run pre-commit hooks
pre-commit run --all-files

# Build for multiple architectures
cross build --target x86_64-unknown-linux-gnu --release
cross build --target aarch64-unknown-linux-gnu --release

# Test Helm chart
helm lint helm/oom-watcher
helm template helm/oom-watcher | kubectl apply --dry-run=client -f -

# Trigger test OOM (use with caution)
python3 scripts/trigger_oom.py
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development workflow, commit conventions, and contribution guidelines.

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

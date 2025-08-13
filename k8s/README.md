# eBPF OOM Watcher - Kubernetes Deployment

This directory contains Kubernetes manifests for deploying the eBPF OOM Watcher as a DaemonSet.

## Features

- **eBPF-based OOM Detection**: Uses kernel tracepoints to capture OOM events in real-time
- **Kubernetes Integration**: Automatically identifies the pod and container where OOM occurred
- **Prometheus Metrics**: Exposes detailed metrics for monitoring and alerting
- **Per-node Deployment**: Runs as a DaemonSet to monitor all nodes in the cluster

## Metrics Exposed

The OOM Watcher exposes the following Prometheus metrics on port 8080:

- `oom_kills_total{node, namespace, pod, container}` - Total number of OOM kills
- `oom_kills_per_node_total{node}` - Total OOM kills per node
- `oom_memory_usage_bytes{node, namespace, pod, container, memory_type}` - Memory usage at OOM time
- `oom_last_timestamp{node, namespace, pod, container}` - Timestamp of last OOM event

## Deployment

### Prerequisites

1. Kubernetes cluster with eBPF support
2. Container runtime that supports cgroups v1 or v2
3. Privileged containers allowed (for eBPF program loading)

### Build and Deploy

1. **Build the container image:**
   ```bash
   docker build -f Dockerfile.production -t oom-watcher:latest .
   ```

2. **Deploy to Kubernetes:**
   ```bash
   kubectl apply -f k8s/daemonset.yaml
   ```

3. **Optional: Deploy ServiceMonitor for Prometheus Operator:**
   ```bash
   kubectl apply -f k8s/servicemonitor.yaml
   ```

### Configuration

Environment variables:
- `NODE_NAME`: Automatically set by the DaemonSet (required)
- `METRICS_PORT`: Port for Prometheus metrics (default: 8080)
- `RUST_LOG`: Log level (default: info)

### Verification

1. **Check DaemonSet status:**
   ```bash
   kubectl get daemonset -n kube-system oom-watcher
   ```

2. **View logs:**
   ```bash
   kubectl logs -n kube-system -l app=oom-watcher
   ```

3. **Test metrics endpoint:**
   ```bash
   kubectl port-forward -n kube-system daemonset/oom-watcher 8080:8080
   curl http://localhost:8080/metrics
   ```

## Security Considerations

The OOM Watcher requires elevated privileges to:
- Load eBPF programs into the kernel
- Access `/proc` filesystem for process information
- Read cgroup information for container identification

The container runs with:
- `privileged: true`
- `hostPID: true` and `hostNetwork: true`
- Mounted host paths: `/proc`, `/sys`, `/sys/kernel/debug`, `/sys/fs/cgroup`

## Troubleshooting

### Common Issues

1. **eBPF program fails to load:**
   - Ensure kernel supports eBPF and tracepoints
   - Check if `CONFIG_BPF=y` and `CONFIG_BPF_SYSCALL=y` in kernel config
   - Verify the `oom:mark_victim` tracepoint exists: `ls /sys/kernel/tracing/events/oom/`

2. **Container/Pod identification fails:**
   - Check if cgroup filesystem is properly mounted
   - Ensure Kubernetes API access is working
   - Verify RBAC permissions

3. **Metrics not appearing:**
   - Check if the metrics endpoint is accessible
   - Verify Prometheus scrape configuration
   - Check logs for any errors

### Debug Commands

```bash
# Check available tracepoints
kubectl exec -n kube-system daemonset/oom-watcher -- ls /sys/kernel/tracing/events/oom/

# Check cgroup structure
kubectl exec -n kube-system daemonset/oom-watcher -- ls /sys/fs/cgroup/

# Trigger test OOM (use with caution)
kubectl run oom-test --rm -it --image=busybox --restart=Never -- sh -c "
  stress --vm 1 --vm-bytes 512M --timeout 10s
"
```

## Integration with Monitoring

### Grafana Dashboard

Create alerts and dashboards using the exposed metrics:

```promql
# Alert on high OOM rate
rate(oom_kills_total[5m]) > 0.1

# OOM kills by namespace
sum by (namespace) (oom_kills_total)

# Memory usage trend before OOM
oom_memory_usage_bytes{memory_type="anon_rss"}
```

### Alertmanager Rules

```yaml
groups:
- name: oom-watcher
  rules:
  - alert: HighOOMRate
    expr: rate(oom_kills_total[5m]) > 0
    for: 0m
    labels:
      severity: warning
    annotations:
      summary: "High OOM kill rate detected"
      description: "OOM kills detected on {{ $labels.node }} for pod {{ $labels.namespace }}/{{ $labels.pod }}"
```
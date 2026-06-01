# Domain Context — ebpf-oom-watcher

Domain vocabulary for the OOM watcher. Architecture reviews and grilling sessions
use these terms exactly.

## Terms

- **OOM kill event** (`OomKillEvent`) — the raw record the kernel emits when it kills
  a process for memory pressure. Captured by the eBPF probe at the `oom:mark_victim`
  tracepoint and shipped to userspace over the ring buffer. Pure numbers + the process
  `comm`; no Kubernetes context.

- **Container identity** (`ContainerIdentity`) — the Kubernetes coordinates of the
  container a killed process belonged to: `namespace`, `pod_name`, `container_name`,
  `container_id`. Resolved from a PID by reading `/proc/<pid>/cgroup` for the container
  id, then matching it against the pods scheduled on this node.

- **Enrichment** — the step that takes a raw **OOM kill event** and a (possibly absent)
  **container identity** and produces an **enriched OOM event**. The single rule it
  encodes: `node_name` is known *iff* this process has a **container resolver** (i.e. we
  are running in-cluster), regardless of whether the container identity could be resolved.
  Lives in `oom-watcher/src/enrich.rs` as the sole construction site for an enriched
  event.

- **Enriched OOM event** (`EnrichedOomEvent`) — an OOM kill event plus its node name,
  optional container identity fields, and a wall-clock timestamp. The unit recorded as
  Prometheus metrics and logged.

- **Resolution** — the I/O act of turning a PID into a **container identity**. Three
  outcomes, carried by the **resolution outcome** type: found (`Found`), not found
  (`NotFound`), or lookup error (`Failed`). The watch loop records the outcome to metrics
  and logs the two failure outcomes distinctly, then collapses to "no identity" via
  `ResolutionOutcome::identity()` before handing off to **enrichment**.

- **Container resolver** (`ContainerResolver`) — the seam for **resolution**. A trait
  exposing `node_name()` and `async resolve(pid) -> ResolutionOutcome`. `KubernetesClient`
  is the in-cluster adapter (maps `Ok(Some)`→`Found`, `Ok(None)`→`NotFound`, `Err`→`Failed`);
  a test fake is the second adapter. Held as an `Option` — `Some` iff in-cluster — which is
  the single source of the **enrichment** `node_name` iff-rule. The watch loop is generic
  over the resolver (static dispatch; no `dyn`).

- **Resolution outcome** (`ResolutionOutcome`) — the three-variant result of **resolution**:
  `Found(ContainerIdentity)`, `NotFound`, `Failed(anyhow::Error)`. Preserves the
  not-found-vs-error distinction past the seam so `oom_resolution_failures_total{reason}`
  can count them separately, where the **enrichment** collapse would otherwise discard it.

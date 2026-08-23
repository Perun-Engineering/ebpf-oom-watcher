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
  `container_id`, `image_id`. Resolved from a PID by reading `/proc/<pid>/cgroup` for the
  container id, then matching it against the pods scheduled on this node. The two ids are
  taken verbatim from the matched `containerStatuses` entry — the runtime-prefixed
  `containerd://<id>` form, not the bare id from the cgroup — because those are the
  strings `kube_pod_container_info` carries, so metrics labelled with them join to it by
  construction. There is no partially-filled identity: a container that cannot be matched
  yields `None` and is counted as a resolution failure.

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
  (`NotFound`), or lookup error (`Failed`). The **watch loop** records the outcome to the
  **metrics recorder** and logs the two failure outcomes distinctly, then collapses to
  "no identity" via `ResolutionOutcome::identity()` before handing off to **enrichment**.

  The `NotFound`/`Failed` split is drawn at the `/proc/<pid>/cgroup` read: `ENOENT` is
  `NotFound`, because the kernel sends SIGKILL *before* firing `oom:mark_victim` and the
  victim is often already reaped — the benign race. Any other read error is `Failed`, since a
  missing `hostPID` or a `/proc` that is not the host's is an operator mistake and must not
  hide behind the race. The same rule governs the **pod cache**: a cache that has not synced
  is `Failed`, never `NotFound`, because an unreachable API server is an operator problem
  and answering "no such container" would file it under the race.

- **Pod cache** (`PodCache`) — the in-memory mirror of the pods scheduled on this node, fed
  by a `spec.nodeName`-scoped `kube::runtime::reflector`. It is where **resolution** looks up
  the container id, and it exists because the previous implementation listed every pod on the
  node on *every* OOM event — one API call per kill during a storm. It is deliberately the
  only pod source: there is no falling back to a live list on a miss, so "no API call per
  event" holds by construction. Objects are pruned (`spec`, `managedFields`, annotations) on
  the way in, leaving only what matching reads. Not on a seam of its own — it is internal to
  `KubernetesClient`, the way **series eviction** is internal to the Prometheus adapter — but
  it is unit-testable regardless, because a reflector `Writer` can be driven by hand.
  Readiness is re-read per lookup rather than latched at startup, so a cache that syncs late
  starts serving; `main` supervises the task feeding it, since a cache nobody feeds goes
  stale in silence.

- **Container resolver** (`ContainerResolver`) — the seam for **resolution**. A trait
  exposing `node_name()` and `async resolve(pid) -> ResolutionOutcome`. `KubernetesClient`
  is the in-cluster adapter (maps `Ok(Some)`→`Found`, `Ok(None)`→`NotFound`, `Err`→`Failed`);
  a test fake is the second adapter. Held as an `Option` — `Some` iff in-cluster — which is
  the single source of the **enrichment** `node_name` iff-rule. The **watch loop** is generic
  over the resolver (static dispatch; no `dyn`).

- **Resolution outcome** (`ResolutionOutcome`) — the three-variant result of **resolution**:
  `Found(ContainerIdentity)`, `NotFound`, `Failed(anyhow::Error)`. Preserves the
  not-found-vs-error distinction past the seam so `oom_resolution_failures_total{reason}`
  can count them separately, where the **enrichment** collapse would otherwise discard it.

- **Watch loop** (`watch::run`) — the module that owns the per-event pipeline: pull an
  **OOM kill event** from an **OOM event source**, run **resolution** (recording the
  **resolution outcome**), **enrich**, then record the **enriched OOM event** to the
  **metrics recorder**. Generic over all three seams (source, resolver, recorder) plus an
  injected clock (`now: impl Fn() -> u64`); static dispatch, no `dyn`. Loops until the
  source ends — which a real source never does, so in production the loop runs forever and
  `main`'s `tokio::select!` supervises and aborts it. A finite test source drives the whole
  loop to completion, making the pipeline the test surface.

- **OOM event source** (`OomEventSource`) — the seam for where **OOM kill events** reach
  userspace. A trait exposing `async next(&mut self) -> Option<OomKillEvent>`; `None` means
  the stream has ended. It also exposes `dropped_total() -> Option<u64>`, defaulted to `None`
  — see **dropped total**. `RingBufSource` is the in-cluster adapter — it owns the eBPF ring
  buffer, performs the single `unsafe` decode (`read_unaligned` + short-read guard), and
  hides the epoll wait (via `tokio::io::unix::AsyncFd`) so it only yields whole events.
  It drains the ring fully into a pending queue before clearing readiness, so handing out one
  event at a time cannot strand records until the next wakeup. `VecSource` (test) and
  `ParkSource` (non-eBPF build; parks forever) are the other adapters.

- **Dropped total** — the monotonic count of **OOM kill events** the probe could not enqueue
  because the ring buffer was full, kept in the `DROPPED` per-CPU map and summed across CPUs
  by `RingBufSource`. Reported to the **metrics recorder** after each event and exposed as
  `oom_events_dropped_total`. Only the ring-buffer adapter can observe it; every other source
  returns `None`. Silent undercounting is the failure this exists to prevent.

- **Tracepoint layout check** (`tracepoint::verify_kernel_layout`) — the startup assertion
  that the running kernel's `oom:mark_victim` trace entry matches the fixed `#[repr(C)]`
  struct the probe decodes. The layout is the one Linux 6.9 introduced; before 6.9 the
  tracepoint carries only `pid`, yet still attaches, so the probe would pair correct PIDs with
  adjacent trace-buffer noise. A mismatch is a hard startup failure. Parsing the `format` file
  is split from reading it, so the decision is testable against captured `format` files.

- **Metrics recorder** (`MetricsRecorder`) — the seam for recording, decoupling the **watch
  loop** from Prometheus. A trait exposing `record_resolution_outcome(node, &outcome)`,
  `record_oom_event(&enriched)`, and `record_dropped_total(node, total)` — the last takes an
  absolute **dropped total**, and the Prometheus adapter advances its counter by the delta.
  `MetricsCollector` is the Prometheus adapter (recording
  only — HTTP serving lives in the `http` module so axum no longer leaks through its
  interface); a test spy is the second adapter.

- **Series eviction** (`MetricsCollector::evict_stale`) — what bounds cardinality. The
  per-container metrics are keyed on pod name, so an OOM-looping pod mints a fresh label set
  on every restart and a Prometheus registry never expires one. The adapter therefore keeps
  its own last-seen time per label set, stamped with the event's own timestamp, and deletes
  those older than a TTL. Not on the `MetricsRecorder` seam: it is housekeeping internal to
  the Prometheus adapter, not something the **watch loop** reports. `main` drives it from a
  dedicated task, because series go stale exactly when events stop and the loop is then
  parked on epoll.

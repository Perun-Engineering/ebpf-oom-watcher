use std::{fs, io, sync::LazyLock, time::Duration};

use anyhow::{anyhow, Context, Result};
use futures::{FutureExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    runtime::{reflector, watcher, WatchStreamExt},
    Api, Client, Config,
};
use log::{debug, info, warn};
use oom_watcher_common::ContainerIdentity;
use regex::Regex;
use tokio::task::JoinHandle;

use crate::resolve::{ContainerResolver, ResolutionOutcome};

/// Patterns that lift a 64-hex container id out of a `/proc/<pid>/cgroup` line, tried in
/// order. Compiled once — this runs on every OOM event.
///
/// The layouts differ by runtime *and* cgroup driver, and the systemd-driver ones are the
/// common case on current clusters:
///
/// | Runtime / driver              | Line                                                    |
/// |-------------------------------|---------------------------------------------------------|
/// | containerd, systemd (cgroup2) | `…/cri-containerd-<id>.scope`                            |
/// | CRI-O, systemd                | `…/crio-<id>.scope`                                      |
/// | Docker, systemd               | `…/docker-<id>.scope`                                    |
/// | Docker, cgroupfs              | `/docker/<id>`                                           |
/// | kubelet cgroupfs, burstable   | `/kubepods/burstable/pod<uid>/<id>`                      |
/// | kubelet cgroupfs, guaranteed  | `/kubepods/pod<uid>/<id>`  (no QoS segment)              |
///
/// The last pattern is a catch-all — any 64-hex run delimited by `/` or `-` — so a runtime
/// not enumerated above still resolves instead of falling through to `unknown`. It subsumes
/// the others; they are kept ahead of it for precedence, so that a line carrying more than
/// one 64-hex segment yields the runtime-prefixed one rather than whichever came first.
///
/// `(?:[^0-9a-f]|$)` after each capture is load-bearing: without it a *longer* hex run
/// matches on its first 64 characters. `regex` has no lookahead, so the boundary is
/// consumed outside the capture group.
static CONTAINER_ID_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?:cri-containerd|crio|docker|libpod)[-/]([0-9a-f]{64})(?:[^0-9a-f]|$)",
        r"/kubepods.*?/([0-9a-f]{64})(?:[^0-9a-f]|$)",
        r"[-/]([0-9a-f]{64})(?:[^0-9a-f]|$)",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("container id patterns are valid"))
    .collect()
});

/// Pull the container id out of the contents of a `/proc/<pid>/cgroup` file.
///
/// Split out from the read so the pattern set is testable against captured cgroup files
/// without a live `/proc`.
fn extract_container_id(cgroup: &str) -> Option<String> {
    CONTAINER_ID_PATTERNS.iter().find_map(|re| {
        re.captures(cgroup)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    })
}

/// Match the bare container id lifted from the cgroup against the container statuses of
/// the pods scheduled on this node, and lift the identity out of the entry that matched.
///
/// Split out from the pod source so the matching rules are testable against constructed
/// pod statuses, the same way [`extract_container_id`] is testable without a live `/proc`.
///
/// That status entry is the only place the runtime-prefixed container id and the image
/// digest exist, which is also why there is no partially-filled identity: a container that
/// does not match yields `None` and is counted as a resolution failure.
fn identity_from_pods<'a>(
    pods: impl IntoIterator<Item = &'a Pod>,
    container_id: &str,
) -> Option<ContainerIdentity> {
    pods.into_iter().find_map(|pod| {
        // `?` here exits this closure, not the function — a pod without statuses is
        // skipped and the search continues with the next one.
        let statuses = pod.status.as_ref()?.container_statuses.as_ref()?;

        let (status, container_id_full) = statuses.iter().find_map(|status| {
            let full = status.container_id.as_deref()?;
            // Container ID format: docker://abc123... or containerd://abc123...
            (full.ends_with(container_id) || full.contains(container_id)).then_some((status, full))
        })?;

        Some(ContainerIdentity {
            namespace: pod
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            pod_name: pod
                .metadata
                .name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            container_name: status.name.clone(),
            // Both ids are taken as the kubelet reported them, never reconstructed: these
            // are the strings `kube_pod_container_info` carries, so emitting them verbatim
            // is what makes the join work.
            container_id: container_id_full.to_string(),
            image_id: status.image_id.clone(),
        })
    })
}

/// Strip the parts of a `Pod` nothing here reads, before it is cached.
///
/// Applied to the watch stream rather than to the store, so the trimmed object is what
/// gets cloned in. Resolution reads exactly three things — `metadata.namespace`,
/// `metadata.name` and `status.containerStatuses` — and `metadata.{name,namespace,uid}`
/// additionally key the store, so `spec`, `managedFields` and annotations can all go.
/// Those are the bulk of a Pod (`last-applied-configuration` alone can exceed the rest),
/// and this cache lives in a per-node DaemonSet sized in tens of MiB.
fn prune_for_cache(pod: &mut Pod) {
    pod.spec = None;
    pod.metadata.managed_fields = None;
    pod.metadata.annotations = None;
}

/// The pods scheduled on this node, mirrored from the API server by a watch.
///
/// This is what keeps resolution off the API path: the previous implementation listed
/// every pod on the node on *every* OOM event, so a kill storm turned into one API call
/// per kill. A `spec.nodeName`-scoped reflector pays for one list plus a watch, and every
/// lookup after that is served from memory.
///
/// The cache is deliberately the *only* pod source — there is no falling back to a live
/// list on a miss, so "no API call per event" holds by construction rather than by
/// discipline.
struct PodCache {
    store: reflector::Store<Pod>,
}

impl PodCache {
    /// Start mirroring the pods on `node_name`.
    ///
    /// Returns the cache and the task driving the watch; the caller must keep that task
    /// alive and supervise it, because a cache nobody is feeding goes stale in silence.
    fn spawn(pods_api: Api<Pod>, node_name: &str) -> (Self, JoinHandle<()>) {
        // The same field selector the per-event list used: this node's pods, not the
        // cluster's. It also bounds what the cache costs — one node's worth of pods.
        let config = watcher::Config::default().fields(&format!("spec.nodeName={}", node_name));
        let (store, writer) = reflector::store();

        // `modify` has to sit between the watcher and the reflector: applied after, the
        // store would already hold the untrimmed objects.
        let events = watcher(pods_api, config).modify(prune_for_cache);

        let cache = Self { store };
        let synced = cache.store.clone();
        let node = node_name.to_string();

        let task = tokio::spawn(
            reflector(writer, events)
                // Retry list/watch failures forever rather than ending the stream: an API
                // server rollout must not permanently freeze the cache. `DefaultBackoff`
                // builds its exponential `.without_max_times()`, so there is no give-up
                // state to reach.
                .default_backoff()
                .for_each(move |event| {
                    match event {
                        // The reflector applies each event to the store *before* yielding
                        // it, so this count is the synced one. Reporting from here rather
                        // than from a `wait_until_ready()` waiter is deliberate twice over:
                        // it also covers the relist after a desync, and it keeps
                        // `ensure_synced` the only poller of the readiness latch — which
                        // has a single waker slot to displace.
                        Ok(watcher::Event::InitDone) => {
                            info!("Pod cache synced: {} pods on node {}", synced.len(), node)
                        }
                        Ok(_) => {}
                        Err(e) => warn!("Pod cache watch error (retrying): {}", e),
                    }
                    std::future::ready(())
                }),
        );

        (cache, task)
    }

    /// The identity of the container with this id, according to the cache.
    ///
    /// `Ok(None)` is a real miss: the cache is in sync and no container on this node
    /// carries that id. An unsynced cache is an `Err` instead, because it is not evidence
    /// of anything — reporting it as a miss would file an unreachable API server under the
    /// benign reap race that `oom_resolution_failures_total{reason}` exists to separate.
    fn identity_of(&self, container_id: &str) -> Result<Option<ContainerIdentity>> {
        self.ensure_synced()?;

        let pods = self.store.state();
        let identity = identity_from_pods(pods.iter().map(|pod| pod.as_ref()), container_id);
        if identity.is_none() {
            warn!("Could not find pod info for container ID: {}", container_id);
        }
        Ok(identity)
    }

    /// Wait for the initial list, giving up after `timeout`.
    ///
    /// Called once before the watch loop starts draining events, so an OOM in the first
    /// moments after a restart resolves instead of reporting an unsynced cache. Giving up
    /// is not fatal — the watch keeps retrying and [`ensure_synced`](Self::ensure_synced)
    /// keeps reporting the truth — so this can only delay the start of watching, never
    /// prevent it.
    async fn wait_until_synced(&self, timeout: Duration) {
        match tokio::time::timeout(timeout, self.store.wait_until_ready()).await {
            // The count is logged by the stream on `InitDone`; saying it twice adds nothing.
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("Pod cache will never sync: {}", e),
            Err(_) => warn!(
                "Pod cache still syncing after {}s; watching anyway — until it lands, events \
                 report an unresolved container rather than a wrong one",
                timeout.as_secs()
            ),
        }
    }

    /// Fail unless the initial list has landed — without waiting for it.
    ///
    /// `wait_until_ready` latches when that list completes, so polling it once is a read
    /// of that latch, and re-reading it per lookup rather than latching the answer at
    /// startup is what lets a cache that synced late start serving.
    ///
    /// This poll and [`wait_until_synced`](Self::wait_until_synced) are the only two users
    /// of the latch, and they never overlap: the wait finishes before the watch loop
    /// resolves its first event. That separation is load-bearing — the latch is a `oneshot`
    /// with a single waker slot, so a concurrent waiter would have its wakeup dropped by
    /// this poll's noop waker. It is also why the sync count is logged from the event
    /// stream rather than from a waiter.
    fn ensure_synced(&self) -> Result<()> {
        match self.store.wait_until_ready().now_or_never() {
            Some(Ok(())) => Ok(()),
            // The reflector task is gone, so the cache will never sync — fatal, and the
            // supervising `select!` in main is about to notice.
            Some(Err(e)) => Err(anyhow!("the pod cache stopped being maintained: {}", e)),
            None => Err(anyhow!(
                "the pod cache has not finished its initial list of the pods on this node"
            )),
        }
    }
}

pub struct KubernetesClient {
    pods: PodCache,
    node_name: String,
}

impl KubernetesClient {
    /// Connect to the API server and start the pod cache.
    ///
    /// Returns the client and the task feeding its cache. Startup deliberately does *not*
    /// block on the initial list: `/metrics` doubles as the liveness probe, so a slow API
    /// server must not delay the HTTP bind. Until the list lands, lookups report an error
    /// rather than a wrong answer.
    pub async fn new() -> Result<(Self, JoinHandle<()>)> {
        let config = Config::incluster()
            .map_err(|e| anyhow!("Failed to create in-cluster config: {}", e))?;

        let client = Client::try_from(config)?;
        let pods_api: Api<Pod> = Api::all(client);

        // Require NODE_NAME rather than defaulting to "unknown": a wrong node scopes the
        // watch to a node with no pods, so every lookup would silently return NotFound.
        // Failing here drops us to standalone mode.
        let node_name = std::env::var("NODE_NAME").map_err(|_| {
            anyhow!("NODE_NAME is unset; the DaemonSet must expose it via the downward API")
        })?;

        let (pods, cache_task) = PodCache::spawn(pods_api, &node_name);

        Ok((Self { pods, node_name }, cache_task))
    }

    /// Wait for the pod cache's initial list, giving up after `timeout`.
    ///
    /// The caller does this before draining OOM events, not during startup: the probe is
    /// already attached by then, so events that land in the meantime wait in the ring
    /// buffer and resolve correctly, instead of being reported against a cache that has
    /// nothing in it yet.
    pub async fn wait_until_synced(&self, timeout: Duration) {
        self.pods.wait_until_synced(timeout).await;
    }

    pub fn get_container_info(&self, pid: u32) -> Result<Option<ContainerIdentity>> {
        let container_id = self.get_container_id_from_pid(pid)?;

        if let Some(container_id) = container_id {
            return self.pods.identity_of(&container_id);
        }

        Ok(None)
    }

    /// Read the killed process's cgroup and lift its container id out.
    ///
    /// `Ok(None)` means "looked, found nothing" — the process is gone (the common case: the
    /// kernel sends SIGKILL *before* firing `oom:mark_victim`, so we are always racing the
    /// reaper) or it was never in a container. Every *other* read failure is an `Err`: a
    /// missing `hostPID`, a `/proc` that isn't the host's, or an EPERM are operator
    /// mistakes, and collapsing them into `NotFound` makes them indistinguishable from the
    /// benign race in both the logs and `oom_resolution_failures_total`.
    fn get_container_id_from_pid(&self, pid: u32) -> Result<Option<String>> {
        let cgroup_path = format!("/proc/{}/cgroup", pid);
        let content = match fs::read_to_string(&cgroup_path) {
            Ok(content) => content,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                debug!(
                    "PID {} was already reaped before we could read its cgroup",
                    pid
                );
                return Ok(None);
            }
            Err(e) => {
                return Err(e).context(format!(
                    "could not read {cgroup_path} (is hostPID set and /proc mounted from the host?)"
                ))
            }
        };

        if let Some(container_id) = extract_container_id(&content) {
            return Ok(Some(container_id));
        }

        debug!(
            "Could not extract container ID from cgroup for PID {}: {}",
            pid, content
        );
        Ok(None)
    }
}

/// The in-cluster adapter for the Resolution seam. Maps `get_container_info`'s
/// `Result<Option<_>>` onto the three [`ResolutionOutcome`] variants.
impl ContainerResolver for KubernetesClient {
    fn node_name(&self) -> &str {
        &self.node_name
    }

    async fn resolve(&self, pid: u32) -> ResolutionOutcome {
        match self.get_container_info(pid) {
            Ok(Some(identity)) => ResolutionOutcome::Found(identity),
            Ok(None) => ResolutionOutcome::NotFound,
            Err(e) => ResolutionOutcome::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::{
        api::core::v1::{ContainerStatus, PodSpec, PodStatus},
        apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta},
    };
    use kube::runtime::reflector::store::Writer;

    use super::*;

    const ID: &str = "3b2f1c8e9d4a5b6c7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d";

    /// Every layout the pattern set claims to handle, as the line actually appears in
    /// `/proc/<pid>/cgroup`. Each case is a runtime + cgroup-driver combination seen in the
    /// wild; the previous three-pattern set silently missed the last four.
    #[test]
    fn extracts_the_container_id_from_every_supported_layout() {
        let cases = [
            (
                "containerd + systemd (cgroup v2)",
                format!("0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-poda1b2.slice/cri-containerd-{ID}.scope"),
            ),
            (
                "CRI-O + systemd",
                format!("0::/kubepods.slice/kubepods-besteffort.slice/kubepods-besteffort-poda1b2.slice/crio-{ID}.scope"),
            ),
            (
                "Docker + systemd",
                format!("0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-poda1b2.slice/docker-{ID}.scope"),
            ),
            ("Docker + cgroupfs", format!("11:memory:/docker/{ID}")),
            (
                "kubelet cgroupfs, burstable QoS",
                format!("11:memory:/kubepods/burstable/poda1b2-c3d4/{ID}"),
            ),
            (
                "kubelet cgroupfs, guaranteed QoS (no QoS segment)",
                format!("11:memory:/kubepods/poda1b2-c3d4/{ID}"),
            ),
        ];

        for (label, cgroup) in cases {
            assert_eq!(
                extract_container_id(&cgroup).as_deref(),
                Some(ID),
                "failed to extract container id for {label}"
            );
        }
    }

    #[test]
    fn extracts_from_a_multi_line_cgroup_v1_file() {
        let cgroup = format!(
            "12:pids:/kubepods/burstable/poda1b2/{ID}\n\
             11:memory:/kubepods/burstable/poda1b2/{ID}\n\
             0::/\n"
        );
        assert_eq!(extract_container_id(&cgroup).as_deref(), Some(ID));
    }

    #[test]
    fn finds_nothing_for_a_process_outside_a_container() {
        assert_eq!(
            extract_container_id("0::/system.slice/sshd.service\n"),
            None
        );
        assert_eq!(extract_container_id("0::/\n"), None);
    }

    /// A pod as the kubelet reports it: one container status carrying the runtime-prefixed
    /// container id and the resolved image digest.
    fn pod_with(container_id: &str, image_id: &str) -> Pod {
        pod_with_statuses(vec![container_status("api", container_id, image_id)])
    }

    fn container_status(name: &str, container_id: &str, image_id: &str) -> ContainerStatus {
        ContainerStatus {
            name: name.to_string(),
            container_id: Some(container_id.to_string()),
            image_id: image_id.to_string(),
            ..Default::default()
        }
    }

    fn pod_with_statuses(container_statuses: Vec<ContainerStatus>) -> Pod {
        Pod {
            metadata: ObjectMeta {
                namespace: Some("prod".to_string()),
                name: Some("api-7d9".to_string()),
                ..Default::default()
            },
            status: Some(PodStatus {
                container_statuses: Some(container_statuses),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn carries_the_kubelet_prefixed_container_id_not_the_bare_cgroup_id() {
        let pods = [pod_with(&format!("containerd://{ID}"), "repo@sha256:abc")];

        let identity = identity_from_pods(&pods, ID).expect("the container id matches");

        // The pod status form is what `kube_pod_container_info` is built from, so emitting
        // it makes the join hold by construction. The bare id from the cgroup would not.
        assert_eq!(identity.container_id, format!("containerd://{ID}"));
    }

    #[test]
    fn carries_the_image_id_verbatim() {
        // Docker reports a `docker-pullable://` prefix where containerd reports a bare
        // digest. Both must pass through untouched — `kube_pod_container_info` carries the
        // same string, and any normalisation here breaks the join.
        for image_id in [
            "repo@sha256:2f1c8e9d4a5b6c7e",
            "docker-pullable://repo@sha256:2f1c8e9d4a5b6c7e",
        ] {
            let pods = [pod_with(&format!("containerd://{ID}"), image_id)];

            let identity = identity_from_pods(&pods, ID).expect("the container id matches");

            assert_eq!(identity.image_id, image_id);
        }
    }

    #[test]
    fn resolves_a_container_whose_image_id_is_empty() {
        // A container that never started has no image digest. It cannot be an OOM victim,
        // but resolution must not fail on the empty string either.
        let pods = [pod_with(&format!("containerd://{ID}"), "")];

        let identity = identity_from_pods(&pods, ID).expect("an empty image id still resolves");

        assert_eq!(identity.image_id, "");
        assert_eq!(identity.container_name, "api");
    }

    #[test]
    fn resolves_the_pod_and_container_names_alongside_the_ids() {
        let pods = [pod_with(&format!("containerd://{ID}"), "repo@sha256:abc")];

        let identity = identity_from_pods(&pods, ID).expect("the container id matches");

        assert_eq!(identity.namespace, "prod");
        assert_eq!(identity.pod_name, "api-7d9");
        assert_eq!(identity.container_name, "api");
    }

    #[test]
    fn picks_the_matching_container_among_several_in_one_pod() {
        // A sidecar shares the pod, so the id is what selects the container — matching on
        // the pod alone would attribute the kill to whichever status came first.
        let other = "a".repeat(64);
        let pods = [pod_with_statuses(vec![
            container_status(
                "sidecar",
                &format!("containerd://{other}"),
                "sidecar@sha256:1",
            ),
            container_status("api", &format!("containerd://{ID}"), "api@sha256:2"),
        ])];

        let identity = identity_from_pods(&pods, ID).expect("the container id matches");

        assert_eq!(identity.container_name, "api");
        assert_eq!(identity.image_id, "api@sha256:2");
    }

    #[test]
    fn finds_nothing_when_no_container_on_the_node_matches() {
        let pods = [pod_with(
            &format!("containerd://{}", "b".repeat(64)),
            "x@sha256:1",
        )];

        assert_eq!(identity_from_pods(&pods, ID), None);
    }

    #[test]
    fn finds_nothing_when_a_pod_has_no_container_statuses_yet() {
        // A pod accepted but not yet started has a status with no container statuses.
        let pending = Pod {
            status: Some(PodStatus::default()),
            ..Default::default()
        };
        let no_status = Pod::default();

        assert_eq!(identity_from_pods(&[pending, no_status], ID), None);
    }

    #[test]
    fn ignores_hex_runs_that_are_not_container_ids() {
        // 63 and 65 hex chars must not pass for a 64-char id.
        let short = format!("0::/kubepods.slice/cri-containerd-{}.scope", "a".repeat(63));
        let long = format!("0::/kubepods.slice/cri-containerd-{}.scope", "a".repeat(65));
        assert_eq!(extract_container_id(&short), None);
        assert_eq!(extract_container_id(&long), None);
    }

    /// A cache and the writer feeding it, standing in for the reflector task.
    ///
    /// The store is the real one: `Writer::apply_watcher_event` is the same call the
    /// reflector makes, so these tests drive genuine cache states — mid-initial-list,
    /// synced, updated, desynced — with no API server anywhere.
    fn cache_and_writer() -> (PodCache, Writer<Pod>) {
        let (store, writer) = reflector::store();
        (PodCache { store }, writer)
    }

    /// The pod the cache tests resolve against: one container, carrying `ID`.
    fn victim_pod() -> Pod {
        pod_with(&format!("containerd://{ID}"), "repo@sha256:abc")
    }

    /// Deliver an initial list, the way a freshly started watch does.
    fn sync(writer: &mut Writer<Pod>, pods: Vec<Pod>) {
        writer.apply_watcher_event(&watcher::Event::Init);
        for pod in pods {
            writer.apply_watcher_event(&watcher::Event::InitApply(pod));
        }
        writer.apply_watcher_event(&watcher::Event::InitDone);
    }

    #[test]
    fn resolves_a_container_from_the_cache() {
        let (cache, mut writer) = cache_and_writer();
        sync(&mut writer, vec![victim_pod()]);

        let identity = cache
            .identity_of(ID)
            .expect("a synced cache is not an error")
            .expect("the container is on this node");

        assert_eq!(identity.pod_name, "api-7d9");
        assert_eq!(identity.container_id, format!("containerd://{ID}"));
    }

    #[test]
    fn a_synced_cache_that_does_not_hold_the_container_is_a_miss_not_an_error() {
        let (cache, mut writer) = cache_and_writer();
        sync(
            &mut writer,
            vec![pod_with(
                &format!("containerd://{}", "b".repeat(64)),
                "x@sha256:1",
            )],
        );

        assert_eq!(
            cache
                .identity_of(ID)
                .expect("a synced cache is not an error"),
            None
        );
    }

    #[test]
    fn an_unsynced_cache_is_an_error_not_a_miss() {
        // Nothing has been written yet — the initial list is still in flight, or the API
        // server is unreachable. Answering `None` here would file that under the benign
        // reap race; it has to reach `oom_resolution_failures_total{reason="error"}`.
        let (cache, _writer) = cache_and_writer();

        assert!(cache.identity_of(ID).is_err());

        // Still an error part-way through the initial list: what has arrived so far is not
        // yet the set of pods on this node.
        let (cache, mut writer) = cache_and_writer();
        writer.apply_watcher_event(&watcher::Event::Init);
        writer.apply_watcher_event(&watcher::Event::InitApply(victim_pod()));

        assert!(cache.identity_of(ID).is_err());
    }

    #[test]
    fn a_lookup_starts_working_once_a_late_cache_syncs() {
        // The reason readiness is re-checked per lookup instead of latched at startup: a
        // watcher that only connects on its third backoff must not leave the process
        // erroring forever.
        let (cache, mut writer) = cache_and_writer();

        assert!(cache.identity_of(ID).is_err());

        sync(&mut writer, vec![victim_pod()]);

        assert!(cache
            .identity_of(ID)
            .expect("the cache has synced now")
            .is_some());
    }

    #[test]
    fn a_container_that_starts_after_the_initial_list_resolves() {
        // The whole point of watching rather than listing once: a pod scheduled a minute
        // from now must resolve too.
        let (cache, mut writer) = cache_and_writer();
        sync(&mut writer, vec![]);

        assert_eq!(cache.identity_of(ID).expect("synced"), None);

        writer.apply_watcher_event(&watcher::Event::Apply(victim_pod()));

        assert!(cache.identity_of(ID).expect("synced").is_some());
    }

    #[test]
    fn a_deleted_pod_stops_resolving() {
        let (cache, mut writer) = cache_and_writer();
        let pod = victim_pod();
        sync(&mut writer, vec![pod.clone()]);

        writer.apply_watcher_event(&watcher::Event::Delete(pod));

        assert_eq!(cache.identity_of(ID).expect("synced"), None);
    }

    #[tokio::test]
    async fn waiting_returns_at_once_when_the_cache_is_already_synced() {
        let (cache, mut writer) = cache_and_writer();
        sync(&mut writer, vec![victim_pod()]);

        // A generous timeout that must not be spent: nothing is pending.
        cache.wait_until_synced(Duration::from_secs(30)).await;

        assert!(cache.identity_of(ID).expect("synced").is_some());
    }

    #[tokio::test]
    async fn waiting_returns_only_after_the_list_lands() {
        // The property the startup wait exists for, and the one the two tests either side
        // of this cannot see: both would pass if the wait returned immediately.
        let (cache, mut writer) = cache_and_writer();
        let listed = std::sync::atomic::AtomicBool::new(false);

        tokio::join!(
            async {
                cache.wait_until_synced(Duration::from_secs(30)).await;
                assert!(
                    listed.load(std::sync::atomic::Ordering::SeqCst),
                    "the wait returned before the initial list landed"
                );
            },
            async {
                // Let the waiter park on the latch before anything is written to it.
                tokio::task::yield_now().await;
                sync(&mut writer, vec![victim_pod()]);
                listed.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );

        assert!(cache.identity_of(ID).expect("synced").is_some());
    }

    #[tokio::test]
    async fn waiting_gives_up_rather_than_blocking_the_watch_loop_forever() {
        // A cache that never syncs must not stop us watching: the events would be reported
        // with no identity, which is worse than reporting them with a stale-cache error.
        let (cache, _writer) = cache_and_writer();

        cache.wait_until_synced(Duration::from_millis(1)).await;

        assert!(cache.identity_of(ID).is_err());
    }

    #[test]
    fn pruning_keeps_everything_resolution_reads() {
        // Guards the memory trim: whatever `prune_for_cache` drops, a pruned pod must
        // still resolve to the same identity a full one does.
        let mut pod = victim_pod();
        pod.spec = Some(PodSpec {
            node_name: Some("node-1".to_string()),
            ..Default::default()
        });
        pod.metadata.managed_fields = Some(vec![ManagedFieldsEntry::default()]);
        pod.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "kubectl.kubernetes.io/last-applied-configuration".to_string(),
            "{}".to_string(),
        )]));

        let full = identity_from_pods(&[pod.clone()], ID).expect("the unpruned pod resolves");
        prune_for_cache(&mut pod);
        let pruned = identity_from_pods(&[pod], ID).expect("the pruned pod still resolves");

        assert_eq!(full, pruned);
    }
}

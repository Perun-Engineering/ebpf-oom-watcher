use std::{fs, io, sync::LazyLock};

use anyhow::{anyhow, Context, Result};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, Api, Client, Config};
use log::{debug, warn};
use oom_watcher_common::ContainerIdentity;
use regex::Regex;

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
/// Split out from the API call so the matching rules are testable against constructed pod
/// statuses, the same way [`extract_container_id`] is testable without a live `/proc`.
///
/// That status entry is the only place the runtime-prefixed container id and the image
/// digest exist, which is also why there is no partially-filled identity: a container that
/// does not match yields `None` and is counted as a resolution failure.
fn identity_from_pods(pods: &[Pod], container_id: &str) -> Option<ContainerIdentity> {
    pods.iter().find_map(|pod| {
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

pub struct KubernetesClient {
    pods_api: Api<Pod>,
    node_name: String,
}

impl KubernetesClient {
    pub async fn new() -> Result<Self> {
        let config = Config::incluster()
            .map_err(|e| anyhow!("Failed to create in-cluster config: {}", e))?;

        let client = Client::try_from(config)?;
        let pods_api: Api<Pod> = Api::all(client);

        // Require NODE_NAME rather than defaulting to "unknown": a wrong node scopes
        // the spec.nodeName field selector to a node with no pods, so every lookup
        // would silently return NotFound. Failing here drops us to standalone mode.
        let node_name = std::env::var("NODE_NAME").map_err(|_| {
            anyhow!("NODE_NAME is unset; the DaemonSet must expose it via the downward API")
        })?;

        Ok(Self {
            pods_api,
            node_name,
        })
    }

    pub async fn get_container_info(&self, pid: u32) -> Result<Option<ContainerIdentity>> {
        let container_id = self.get_container_id_from_pid(pid)?;

        if let Some(container_id) = container_id {
            return self.get_pod_info_from_container_id(&container_id).await;
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

    async fn get_pod_info_from_container_id(
        &self,
        container_id: &str,
    ) -> Result<Option<ContainerIdentity>> {
        // Scope the query to this node so we don't list every pod in the
        // cluster on each OOM event; the kubelet supports the spec.nodeName
        // field selector for pods.
        let params = ListParams::default().fields(&format!("spec.nodeName={}", self.node_name));
        let pods = self.pods_api.list(&params).await?;

        let identity = identity_from_pods(&pods.items, container_id);
        if identity.is_none() {
            warn!("Could not find pod info for container ID: {}", container_id);
        }
        Ok(identity)
    }
}

/// The in-cluster adapter for the Resolution seam. Maps `get_container_info`'s
/// `Result<Option<_>>` onto the three [`ResolutionOutcome`] variants.
impl ContainerResolver for KubernetesClient {
    fn node_name(&self) -> &str {
        &self.node_name
    }

    async fn resolve(&self, pid: u32) -> ResolutionOutcome {
        match self.get_container_info(pid).await {
            Ok(Some(identity)) => ResolutionOutcome::Found(identity),
            Ok(None) => ResolutionOutcome::NotFound,
            Err(e) => ResolutionOutcome::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::{
        api::core::v1::{ContainerStatus, PodStatus},
        apimachinery::pkg::apis::meta::v1::ObjectMeta,
    };

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
}

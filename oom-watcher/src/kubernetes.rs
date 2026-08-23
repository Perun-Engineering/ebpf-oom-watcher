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

        for pod in pods.items {
            if let Some(status) = &pod.status {
                if let Some(container_statuses) = &status.container_statuses {
                    for container_status in container_statuses {
                        if let Some(container_id_full) = &container_status.container_id {
                            // Container ID format: docker://abc123... or containerd://abc123...
                            if container_id_full.ends_with(container_id)
                                || container_id_full.contains(container_id)
                            {
                                let namespace = pod
                                    .metadata
                                    .namespace
                                    .clone()
                                    .unwrap_or_else(|| "default".to_string());
                                let pod_name = pod
                                    .metadata
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string());
                                let container_name = container_status.name.clone();

                                return Ok(Some(ContainerIdentity {
                                    namespace,
                                    pod_name,
                                    container_name,
                                    container_id: container_id.to_string(),
                                }));
                            }
                        }
                    }
                }
            }
        }

        warn!("Could not find pod info for container ID: {}", container_id);
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
        match self.get_container_info(pid).await {
            Ok(Some(identity)) => ResolutionOutcome::Found(identity),
            Ok(None) => ResolutionOutcome::NotFound,
            Err(e) => ResolutionOutcome::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn ignores_hex_runs_that_are_not_container_ids() {
        // 63 and 65 hex chars must not pass for a 64-char id.
        let short = format!("0::/kubepods.slice/cri-containerd-{}.scope", "a".repeat(63));
        let long = format!("0::/kubepods.slice/cri-containerd-{}.scope", "a".repeat(65));
        assert_eq!(extract_container_id(&short), None);
        assert_eq!(extract_container_id(&long), None);
    }
}

use std::fs;

use anyhow::{anyhow, Result};
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client, Config};
use log::{debug, warn};
use regex::Regex;

pub struct KubernetesClient {
    client: Client,
    pods_api: Api<Pod>,
    node_name: String,
}

impl KubernetesClient {
    pub async fn new() -> Result<Self> {
        let config = Config::incluster()
            .map_err(|e| anyhow!("Failed to create in-cluster config: {}", e))?;

        let client = Client::try_from(config)?;
        let pods_api: Api<Pod> = Api::all(client.clone());

        let node_name = std::env::var("NODE_NAME").unwrap_or_else(|_| "unknown".to_string());

        Ok(Self {
            client,
            pods_api,
            node_name,
        })
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    pub async fn get_container_info(
        &self,
        pid: u32,
    ) -> Result<Option<(String, String, String, String)>> {
        let container_id = self.get_container_id_from_pid(pid)?;

        if let Some(container_id) = container_id {
            if let Some((namespace, pod_name, container_name)) =
                self.get_pod_info_from_container_id(&container_id).await?
            {
                return Ok(Some((namespace, pod_name, container_name, container_id)));
            }
        }

        Ok(None)
    }

    fn get_container_id_from_pid(&self, pid: u32) -> Result<Option<String>> {
        let cgroup_path = format!("/proc/{}/cgroup", pid);
        let content = match fs::read_to_string(&cgroup_path) {
            Ok(content) => content,
            Err(_) => {
                debug!("Could not read cgroup file for PID {}", pid);
                return Ok(None);
            }
        };

        // Extract container ID from cgroup path
        // Formats can vary: docker, containerd, cri-o
        let patterns = [
            r"/docker/([a-f0-9]{64})",                  // Docker
            r"/kubepods/[^/]*/pod[^/]*/([a-f0-9]{64})", // Containerd/CRI-O
            r"cri-containerd-([a-f0-9]{64})",           // CRI-containerd
        ];

        for pattern in &patterns {
            let re = Regex::new(pattern)?;
            if let Some(captures) = re.captures(&content) {
                if let Some(container_id) = captures.get(1) {
                    return Ok(Some(container_id.as_str().to_string()));
                }
            }
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
    ) -> Result<Option<(String, String, String)>> {
        let pods = self.pods_api.list(&Default::default()).await?;

        for pod in pods.items {
            if let Some(pod_spec) = &pod.spec {
                if let Some(node_name) = &pod_spec.node_name {
                    if node_name != &self.node_name {
                        continue;
                    }
                }
            }

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

                                return Ok(Some((namespace, pod_name, container_name)));
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

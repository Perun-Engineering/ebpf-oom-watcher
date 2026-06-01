use oom_watcher_common::{ContainerIdentity, EnrichedOomEvent, OomKillEvent};

/// Build an [`EnrichedOomEvent`] from a raw OOM kill event and an optional resolved
/// container identity. This is the sole construction site for an enriched event.
///
/// It encodes one rule: `node_name` is known iff a Kubernetes client exists (the
/// caller passes `Some`), independent of whether the container identity could be
/// resolved. A failed resolution clears the container fields but never the node.
pub fn enrich(
    raw_event: OomKillEvent,
    node_name: Option<&str>,
    identity: Option<ContainerIdentity>,
    timestamp: u64,
) -> EnrichedOomEvent {
    let (namespace, pod_name, container_name, container_id) = match identity {
        Some(id) => (
            Some(id.namespace),
            Some(id.pod_name),
            Some(id.container_name),
            Some(id.container_id),
        ),
        None => (None, None, None, None),
    };

    EnrichedOomEvent {
        raw_event,
        node_name: node_name.map(str::to_string),
        namespace,
        pod_name,
        container_name,
        container_id,
        timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> OomKillEvent {
        OomKillEvent {
            pid: 1234,
            tgid: 1200,
            comm: *b"python\0\0\0\0\0\0\0\0\0\0",
            total_vm: 100,
            anon_rss: 50,
            file_rss: 20,
            shmem_rss: 5,
            uid: 1000,
            pgtables: 8,
            oom_score_adj: 0,
        }
    }

    fn identity() -> ContainerIdentity {
        ContainerIdentity {
            namespace: "prod".into(),
            pod_name: "api-7d9".into(),
            container_name: "api".into(),
            container_id: "abc123".into(),
        }
    }

    #[test]
    fn fills_all_fields_when_identity_resolved_on_node() {
        let e = enrich(raw(), Some("node-1"), Some(identity()), 42);
        assert_eq!(e.node_name.as_deref(), Some("node-1"));
        assert_eq!(e.namespace.as_deref(), Some("prod"));
        assert_eq!(e.pod_name.as_deref(), Some("api-7d9"));
        assert_eq!(e.container_name.as_deref(), Some("api"));
        assert_eq!(e.container_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn keeps_node_when_identity_unresolved() {
        // The load-bearing invariant: a failed resolution must not erase the node we
        // already know we are running on.
        let e = enrich(raw(), Some("node-1"), None, 42);
        assert_eq!(e.node_name.as_deref(), Some("node-1"));
        assert_eq!(e.namespace, None);
        assert_eq!(e.pod_name, None);
        assert_eq!(e.container_name, None);
        assert_eq!(e.container_id, None);
    }

    #[test]
    fn all_none_in_standalone_mode() {
        let e = enrich(raw(), None, None, 42);
        assert_eq!(e.node_name, None);
        assert_eq!(e.namespace, None);
        assert_eq!(e.pod_name, None);
        assert_eq!(e.container_name, None);
        assert_eq!(e.container_id, None);
    }

    #[test]
    fn passes_raw_event_and_timestamp_through() {
        let e = enrich(raw(), None, None, 99);
        assert_eq!(e.timestamp, 99);
        assert_eq!(e.raw_event.pid, 1234);
        assert_eq!(e.raw_event.total_vm, 100);
    }
}

//! The Resolution seam: turning a killed PID into a container identity.
//!
//! `Resolution` is the I/O act of mapping a PID to the Kubernetes container that
//! owned it. [`ContainerResolver`] is the seam it lives behind, and
//! [`ResolutionOutcome`] is what crosses that seam — preserving the
//! not-found-vs-error distinction the enrichment collapse would otherwise discard.

use oom_watcher_common::ContainerIdentity;

/// The three outcomes of resolving a PID to a container identity.
///
/// Keeping `NotFound` and `Failed` distinct past the seam lets metrics count them
/// separately; [`identity`](Self::identity) is where both collapse to "no identity"
/// for enrichment.
#[derive(Debug)]
pub enum ResolutionOutcome {
    /// A pod on this node owns the killed process.
    Found(ContainerIdentity),
    /// We looked but no pod matched — the process is not in a container, or it was
    /// already reaped before we could read its cgroup.
    NotFound,
    /// The lookup itself failed: proc read, regex, or Kubernetes API error.
    Failed(anyhow::Error),
}

impl ResolutionOutcome {
    /// Collapse to the shape [`enrich`](crate::enrich::enrich) consumes: an identity
    /// iff resolution found one. Both failure modes become "no identity".
    pub fn identity(self) -> Option<ContainerIdentity> {
        match self {
            Self::Found(identity) => Some(identity),
            Self::NotFound | Self::Failed(_) => None,
        }
    }
}

/// The seam for Resolution: turn a killed PID into a [`ResolutionOutcome`].
///
/// `KubernetesClient` is the in-cluster adapter; tests use a fake. The watch loop
/// holds an `Option<impl ContainerResolver>` — `Some` iff in-cluster — which is the
/// single source of the enrichment `node_name` iff-rule.
// Static dispatch only (the loop is generic over a concrete resolver, never `dyn`),
// so the missing-`Send`-bound concern the lint guards against does not apply.
#[allow(async_fn_in_trait)]
pub trait ContainerResolver {
    /// The node this resolver is scoped to — known because we are in-cluster.
    fn node_name(&self) -> &str;

    /// Resolve a PID to its container identity. Never surfaces an error directly;
    /// failures are carried as [`ResolutionOutcome::Failed`] so callers handle all
    /// three outcomes through one match.
    async fn resolve(&self, pid: u32) -> ResolutionOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ContainerIdentity {
        ContainerIdentity {
            namespace: "prod".into(),
            pod_name: "api-7d9".into(),
            container_name: "api".into(),
            container_id: "abc123".into(),
        }
    }

    #[test]
    fn identity_is_some_only_when_found() {
        assert_eq!(
            ResolutionOutcome::Found(identity()).identity(),
            Some(identity())
        );
        assert_eq!(ResolutionOutcome::NotFound.identity(), None);
        assert_eq!(
            ResolutionOutcome::Failed(anyhow::anyhow!("boom")).identity(),
            None
        );
    }

    /// A second adapter — proving the seam is real, not hypothetical. Reused as the
    /// test harness once the watch loop is extracted (candidate 1).
    enum Behavior {
        Found(ContainerIdentity),
        NotFound,
        Fail,
    }

    struct FakeResolver {
        node: String,
        behavior: Behavior,
    }

    impl ContainerResolver for FakeResolver {
        fn node_name(&self) -> &str {
            &self.node
        }

        async fn resolve(&self, _pid: u32) -> ResolutionOutcome {
            match &self.behavior {
                Behavior::Found(id) => ResolutionOutcome::Found(id.clone()),
                Behavior::NotFound => ResolutionOutcome::NotFound,
                Behavior::Fail => ResolutionOutcome::Failed(anyhow::anyhow!("api down")),
            }
        }
    }

    #[tokio::test]
    async fn fake_resolver_yields_each_outcome() {
        let found = FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::Found(identity()),
        };
        assert_eq!(found.node_name(), "node-1");
        assert!(matches!(
            found.resolve(1234).await,
            ResolutionOutcome::Found(_)
        ));

        let missing = FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::NotFound,
        };
        assert!(matches!(
            missing.resolve(1234).await,
            ResolutionOutcome::NotFound
        ));

        let broken = FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::Fail,
        };
        assert!(matches!(
            broken.resolve(1234).await,
            ResolutionOutcome::Failed(_)
        ));
    }
}

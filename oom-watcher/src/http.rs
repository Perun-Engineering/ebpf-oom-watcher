//! The HTTP surface. Confines axum to one module so the [`MetricsCollector`] and
//! [`Health`] interfaces stay transport-free — the watch loop and `main` never touch axum
//! types.
//!
//! Two endpoints, deliberately distinct: `/metrics` is the scrape target (and the readiness
//! probe — being able to serve a scrape is exactly what readiness means here), while
//! `/healthz` is the liveness probe and answers for the watch loop rather than for axum.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Response, routing::get, Router};

use crate::{
    health::{Health, Liveness, HEARTBEAT_STALE_AFTER_SECONDS},
    metrics::MetricsCollector,
};

/// The clock the liveness handler reads. Injected rather than called directly so the
/// staleness decision is testable, matching the clock the watch loop already takes.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Clone)]
struct HealthContext {
    health: Arc<Health>,
    now: Clock,
}

/// Build the router: `/metrics` backed by `collector`, `/healthz` backed by `health`.
pub fn router(collector: Arc<MetricsCollector>, health: Arc<Health>, now: Clock) -> Router {
    let metrics = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(collector);
    let healthz = Router::new()
        .route("/healthz", get(health_handler))
        .with_state(HealthContext { health, now });
    metrics.merge(healthz)
}

async fn metrics_handler(
    State(collector): State<Arc<MetricsCollector>>,
) -> Result<Response<String>, StatusCode> {
    let metrics = collector.get_metrics();
    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(metrics)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn health_handler(State(context): State<HealthContext>) -> (StatusCode, String) {
    report(context.health.liveness((context.now)()))
}

/// Render a [`Liveness`] as the probe's answer. Split from the handler so the mapping is
/// unit-testable without standing up a server.
fn report(liveness: Liveness) -> (StatusCode, String) {
    match liveness {
        Liveness::Starting => (
            StatusCode::SERVICE_UNAVAILABLE,
            "starting: the watch loop has not begun draining events yet\n".to_string(),
        ),
        Liveness::Live { age_seconds } => (
            StatusCode::OK,
            format!("live: watch loop last woke {}s ago\n", age_seconds),
        ),
        Liveness::Stale { age_seconds } => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "stale: the watch loop has not woken for {}s (limit {}s)\n",
                age_seconds, HEARTBEAT_STALE_AFTER_SECONDS
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_is_not_yet_healthy() {
        let (status, body) = report(Liveness::Starting);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("starting"), "{body}");
    }

    #[test]
    fn a_live_loop_answers_200_with_its_age() {
        let (status, body) = report(Liveness::Live { age_seconds: 7 });

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("7s ago"), "{body}");
    }

    #[test]
    fn a_stale_loop_fails_the_probe_and_names_the_limit() {
        let (status, body) = report(Liveness::Stale { age_seconds: 400 });

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("400s"), "{body}");
        assert!(
            body.contains(&HEARTBEAT_STALE_AFTER_SECONDS.to_string()),
            "{body}"
        );
    }
}

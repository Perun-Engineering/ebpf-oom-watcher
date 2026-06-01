//! The metrics HTTP surface. Confines axum to one module so the [`MetricsCollector`]
//! interface stays Prometheus-only — the watch loop and `main` never touch axum types.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Response, routing::get, Router};

use crate::metrics::MetricsCollector;

/// Build the `/metrics` router backed by `collector`.
pub fn router(collector: Arc<MetricsCollector>) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(collector)
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

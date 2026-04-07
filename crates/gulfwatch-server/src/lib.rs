mod routes;
mod ws;

use std::net::SocketAddr;

use axum::Router;
use gulfwatch_core::AppState;
use tower_http::cors::CorsLayer;
use tracing::info;

/// Build the axum router with all routes and shared state.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(routes::health_routes())
        .merge(routes::program_routes())
        .merge(routes::metrics_routes())
        .merge(routes::transaction_routes())
        .merge(routes::prometheus_routes())
        .merge(routes::alert_routes())
        .merge(ws::ws_routes())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Start the HTTP server on the given address.
pub async fn run_server(state: AppState, addr: SocketAddr) {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("Server listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}

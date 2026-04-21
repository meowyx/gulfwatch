use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, delete, put},
};
use gulfwatch_core::AppState;
use gulfwatch_core::alert::AlertRule;
use gulfwatch_core::idl::{parse_idl_value, IdlDocument};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

// ─── Health ──────────────────────────────────────────────

pub fn health_routes() -> Router<AppState> {
    Router::new().route("/health", get(health_check))
}

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

// ─── Programs ────────────────────────────────────────────

pub fn program_routes() -> Router<AppState> {
    Router::new()
        .route("/api/programs", get(list_programs).post(add_program))
        .route("/api/programs/{id}", delete(remove_program))
}

async fn list_programs(State(state): State<AppState>) -> Json<Vec<String>> {
    let programs = state.monitored_programs.read().await;
    Json(programs.clone())
}

#[derive(Deserialize)]
struct AddProgramRequest {
    program_id: String,
}

async fn add_program(
    State(state): State<AppState>,
    Json(req): Json<AddProgramRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    info!(program_id = %req.program_id, "Adding program to monitor");
    state.add_program(req.program_id.clone()).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "program_id": req.program_id, "status": "monitoring" })),
    )
}

async fn remove_program(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    info!(program_id = %id, "Removing program from monitoring");
    state.remove_program(&id).await;
    StatusCode::NO_CONTENT
}

// ─── Program IDL ─────────────────────────────────────────

pub fn idl_routes() -> Router<AppState> {
    Router::new().route(
        "/api/programs/{id}/idl",
        get(get_idl).post(upsert_idl).delete(delete_idl),
    )
}

#[derive(Serialize)]
struct IdlUpsertResponse {
    program_id: String,
    name: String,
    format: String,
    instruction_count: usize,
    error_count: usize,
}

#[derive(Serialize)]
struct IdlUpsertError {
    program_id: String,
    error: String,
}

async fn upsert_idl(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(value): Json<Value>,
) -> Result<(StatusCode, Json<IdlUpsertResponse>), (StatusCode, Json<IdlUpsertError>)> {
    match parse_idl_value(value) {
        Ok(idl) => {
            info!(
                program_id = %id,
                name = %idl.name,
                format = ?idl.format,
                "Upserting IDL"
            );
            let response = IdlUpsertResponse {
                program_id: id.clone(),
                name: idl.name.clone(),
                format: format!("{:?}", idl.format),
                instruction_count: idl.instructions.len(),
                error_count: idl.errors.len(),
            };
            state.upsert_idl(&id, idl).await;
            Ok((StatusCode::OK, Json(response)))
        }
        Err(e) => {
            let reason = e.to_string();
            warn!(program_id = %id, error = %reason, "IDL upload rejected");
            state.set_idl_failure(&id, &reason).await;
            Err((
                StatusCode::BAD_REQUEST,
                Json(IdlUpsertError {
                    program_id: id,
                    error: reason,
                }),
            ))
        }
    }
}

async fn get_idl(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<IdlDocument>, StatusCode> {
    match state.get_idl(&id).await {
        Some(idl) => Ok(Json(idl)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn delete_idl(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    if state.remove_idl(&id).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// ─── Metrics ─────────────────────────────────────────────

pub fn metrics_routes() -> Router<AppState> {
    Router::new()
        .route("/api/metrics/summary", get(metrics_summary))
        .route("/api/metrics/timeseries", get(metrics_timeseries))
}

#[derive(Deserialize)]
struct MetricsQuery {
    program: Option<String>,
}

async fn metrics_summary(
    State(state): State<AppState>,
    Query(query): Query<MetricsQuery>,
) -> Json<serde_json::Value> {
    let windows = state.windows.read().await;

    match query.program {
        Some(ref program_id) => {
            if let Some(window) = windows.get(program_id) {
                Json(serde_json::to_value(window.summary(program_id)).unwrap())
            } else {
                Json(serde_json::json!({ "error": "program not found" }))
            }
        }
        None => {
            // Return summaries for all monitored programs
            let summaries: Vec<_> = windows
                .iter()
                .map(|(pid, window)| window.summary(pid))
                .collect();
            Json(serde_json::to_value(summaries).unwrap())
        }
    }
}

#[derive(Deserialize)]
struct TimeseriesQuery {
    program: String,
    /// Bucket interval in seconds (default 60)
    interval: Option<i64>,
}

async fn metrics_timeseries(
    State(state): State<AppState>,
    Query(query): Query<TimeseriesQuery>,
) -> Json<serde_json::Value> {
    let windows = state.windows.read().await;
    let interval = query.interval.unwrap_or(60);

    if let Some(window) = windows.get(&query.program) {
        Json(serde_json::to_value(window.timeseries(interval)).unwrap())
    } else {
        Json(serde_json::json!([]))
    }
}

// ─── Transactions ────────────────────────────────────────

pub fn transaction_routes() -> Router<AppState> {
    Router::new()
        .route("/api/transactions/recent", get(recent_transactions))
        .route("/api/transactions/{signature}", get(get_transaction_by_signature))
}

#[derive(Deserialize)]
struct TransactionsQuery {
    program: Option<String>,
    limit: Option<usize>,
    category: Option<String>,
    classifier: Option<String>,
    min_confidence: Option<f32>,
    has_debug: Option<bool>,
}

fn tx_matches_filters(tx: &gulfwatch_core::Transaction, query: &TransactionsQuery) -> bool {
    if let Some(category) = query.category.as_deref() {
        let Some(classification) = &tx.classification else {
            return false;
        };
        if classification.category != category {
            return false;
        }
    }

    if let Some(classifier) = query.classifier.as_deref() {
        let Some(classification) = &tx.classification else {
            return false;
        };
        if classification.classifier != classifier {
            return false;
        }
    }

    if let Some(min_confidence) = query.min_confidence {
        let Some(classification) = &tx.classification else {
            return false;
        };
        if classification.confidence < min_confidence {
            return false;
        }
    }

    if let Some(has_debug) = query.has_debug {
        let present = tx.classification_debug.is_some();
        if present != has_debug {
            return false;
        }
    }

    true
}

async fn recent_transactions(
    State(state): State<AppState>,
    Query(query): Query<TransactionsQuery>,
) -> Json<serde_json::Value> {
    let windows = state.windows.read().await;
    let limit = query.limit.unwrap_or(50);

    match query.program {
        Some(ref program_id) => {
            if let Some(window) = windows.get(program_id) {
                let txs: Vec<_> = window
                    .recent(limit.saturating_mul(4))
                    .into_iter()
                    .filter(|tx| tx_matches_filters(tx, &query))
                    .take(limit)
                    .collect();
                Json(serde_json::to_value(txs).unwrap())
            } else {
                Json(serde_json::json!([]))
            }
        }
        None => {
            // Collect recent transactions from all windows, merge and sort
            let mut all_txs: Vec<_> = windows
                .values()
                .flat_map(|w| w.recent(limit.saturating_mul(4)))
                .collect();
            all_txs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            all_txs.retain(|tx| tx_matches_filters(tx, &query));
            all_txs.truncate(limit);
            Json(serde_json::to_value(all_txs).unwrap())
        }
    }
}

async fn get_transaction_by_signature(
    State(state): State<AppState>,
    axum::extract::Path(signature): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    {
        let windows = state.windows.read().await;
        for window in windows.values() {
            if let Some(tx) = window.find_by_signature(&signature) {
                return Ok(Json(serde_json::to_value(tx).unwrap()));
            }
        }
    }

    let Ok(rpc_url) = std::env::var("SOLANA_RPC_URL") else {
        return Err(StatusCode::NOT_FOUND);
    };
    let raw = match gulfwatch_ingest::fetch_transaction(&rpc_url, &signature).await {
        Ok(v) => v,
        Err(_) => return Err(StatusCode::BAD_GATEWAY),
    };
    let monitored = state.monitored_programs.read().await.clone();
    match gulfwatch_ingest::parser::parse_transaction(&raw, &signature, &monitored) {
        Some(tx) => Ok(Json(serde_json::to_value(tx).unwrap())),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// ─── Prometheus ──────────────────────────────────────────

pub fn prometheus_routes() -> Router<AppState> {
    Router::new().route("/metrics", get(prometheus_metrics))
}

async fn prometheus_metrics(State(state): State<AppState>) -> (StatusCode, [(String, String); 1], String) {
    let windows = state.windows.read().await;
    let mut output = String::new();

    output.push_str("# HELP gulfwatch_tx_count Total transactions in rolling window\n");
    output.push_str("# TYPE gulfwatch_tx_count gauge\n");
    for (pid, window) in windows.iter() {
        let summary = window.summary(pid);
        output.push_str(&format!(
            "gulfwatch_tx_count{{program_id=\"{}\"}} {}\n",
            pid, summary.tx_count
        ));
    }

    output.push_str("# HELP gulfwatch_error_count Failed transactions in rolling window\n");
    output.push_str("# TYPE gulfwatch_error_count gauge\n");
    for (pid, window) in windows.iter() {
        let summary = window.summary(pid);
        output.push_str(&format!(
            "gulfwatch_error_count{{program_id=\"{}\"}} {}\n",
            pid, summary.error_count
        ));
    }

    output.push_str("# HELP gulfwatch_error_rate Error rate in rolling window\n");
    output.push_str("# TYPE gulfwatch_error_rate gauge\n");
    for (pid, window) in windows.iter() {
        let summary = window.summary(pid);
        output.push_str(&format!(
            "gulfwatch_error_rate{{program_id=\"{}\"}} {:.6}\n",
            pid, summary.error_rate
        ));
    }

    output.push_str("# HELP gulfwatch_avg_compute_units Average compute units in rolling window\n");
    output.push_str("# TYPE gulfwatch_avg_compute_units gauge\n");
    for (pid, window) in windows.iter() {
        let summary = window.summary(pid);
        output.push_str(&format!(
            "gulfwatch_avg_compute_units{{program_id=\"{}\"}} {:.2}\n",
            pid, summary.avg_compute_units
        ));
    }

    output.push_str("# HELP gulfwatch_window_minutes Rolling window size in minutes\n");
    output.push_str("# TYPE gulfwatch_window_minutes gauge\n");
    for (pid, window) in windows.iter() {
        let summary = window.summary(pid);
        output.push_str(&format!(
            "gulfwatch_window_minutes{{program_id=\"{}\"}} {}\n",
            pid, summary.window_minutes
        ));
    }

    (
        StatusCode::OK,
        [("content-type".to_string(), "text/plain; version=0.0.4; charset=utf-8".to_string())],
        output,
    )
}

// ─── Alerts ──────────────────────────────────────────────

pub fn alert_routes() -> Router<AppState> {
    Router::new()
        .route("/api/alerts", get(list_alerts).post(create_alert))
        .route("/api/alerts/recent", get(recent_alerts))
        .route("/api/alerts/{id}", put(update_alert).delete(delete_alert))
}

#[derive(Deserialize)]
struct RecentAlertsQuery {
    since: Option<String>,
    limit: Option<usize>,
}

async fn recent_alerts(
    State(state): State<AppState>,
    Query(query): Query<RecentAlertsQuery>,
) -> Json<serde_json::Value> {
    let buf = state.recent_alerts.read().await;
    let since_cutoff = query
        .since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    let limit = query.limit.unwrap_or(100);

    let mut events: Vec<&gulfwatch_core::alert::AlertEvent> = buf
        .iter()
        .filter(|e| since_cutoff.is_none_or(|cutoff| e.fired_at >= cutoff))
        .collect();
    events.sort_by(|a, b| b.fired_at.cmp(&a.fired_at));
    events.truncate(limit);
    Json(serde_json::to_value(events).unwrap())
}

async fn list_alerts(State(state): State<AppState>) -> Json<Vec<AlertRule>> {
    let rules = state.alert_rules.read().await;
    Json(rules.clone())
}

async fn create_alert(
    State(state): State<AppState>,
    Json(rule): Json<AlertRule>,
) -> (StatusCode, Json<AlertRule>) {
    info!(rule_id = %rule.id, name = %rule.name, "Creating alert rule");
    let mut rules = state.alert_rules.write().await;
    rules.push(rule.clone());
    (StatusCode::CREATED, Json(rule))
}

async fn update_alert(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(updated): Json<AlertRule>,
) -> StatusCode {
    let mut rules = state.alert_rules.write().await;
    if let Some(rule) = rules.iter_mut().find(|r| r.id == id) {
        *rule = updated;
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn delete_alert(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    let mut rules = state.alert_rules.write().await;
    let len_before = rules.len();
    rules.retain(|r| r.id != id);
    if rules.len() < len_before {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use gulfwatch_core::{ClassificationDebugTrace, TransactionClassification};
    use gulfwatch_core::Transaction;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_tx(program_id: &str) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: program_id.to_string(),
            block_slot: 100,
            timestamp: chrono::Utc::now(),
            success: true,
            instruction_type: Some("swap".to_string()),
            accounts: vec![],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions: vec![],
            cu_profile: None,
            classification: None,
            classification_debug: None,
            logs: vec![],
            balance_diff: None,
            tx_error: None,
        }
    }

    fn make_classified_tx(
        program_id: &str,
        category: &str,
        classifier: &str,
        confidence: f32,
        has_debug: bool,
    ) -> Transaction {
        let mut tx = make_tx(program_id);
        tx.classification = Some(TransactionClassification {
            category: category.to_string(),
            classifier: classifier.to_string(),
            confidence,
            summary: "test".to_string(),
        });
        if has_debug {
            tx.classification_debug = Some(ClassificationDebugTrace {
                focal_account: None,
                decisions: vec![],
                legs: vec![],
            });
        }
        tx
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let response = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn add_and_list_programs() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        // Add a program
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/programs")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"program_id":"675kPX9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // List programs
        let response = app
            .oneshot(Request::get("/api/programs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, vec!["675kPX9"]);
    }

    #[tokio::test]
    async fn metrics_summary_empty() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;
        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::get("/api/metrics/summary?program=prog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["tx_count"], 0);
        assert_eq!(json["program_id"], "prog");
    }

    #[tokio::test]
    async fn recent_transactions_returns_data() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;

        // Push a transaction directly into the window
        {
            let mut windows = state.windows.write().await;
            let window = windows.get_mut("prog").unwrap();
            window.push(make_tx("prog"));
        }

        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::get("/api/transactions/recent?program=prog&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["signature"], "test_sig");
    }

    #[tokio::test]
    async fn get_transaction_by_signature_returns_full_decoded_tx() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;
        {
            let mut windows = state.windows.write().await;
            let window = windows.get_mut("prog").unwrap();
            let mut tx = make_tx("prog");
            tx.signature = "deepdive_sig".to_string();
            tx.logs = vec!["Program prog invoke [1]".to_string(), "Program prog success".to_string()];
            window.push(tx);
        }
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get("/api/transactions/deepdive_sig")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["signature"], "deepdive_sig");
        assert_eq!(json["logs"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn get_transaction_by_signature_returns_404_when_missing() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get("/api/transactions/missing_sig")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn recent_alerts_returns_buffered_events() {
        use gulfwatch_core::alert::AlertEvent;
        let (state, _rx) = AppState::new(100, 10);
        {
            let mut buf = state.recent_alerts.write().await;
            let now = chrono::Utc::now();
            buf.push_back(AlertEvent {
                rule_id: "r1".to_string(),
                rule_name: "old".to_string(),
                program_id: "prog".to_string(),
                metric: "x".to_string(),
                value: 1.0,
                threshold: 0.0,
                fired_at: now - chrono::Duration::seconds(600),
            });
            buf.push_back(AlertEvent {
                rule_id: "r2".to_string(),
                rule_name: "new".to_string(),
                program_id: "prog".to_string(),
                metric: "x".to_string(),
                value: 2.0,
                threshold: 0.0,
                fired_at: now,
            });
        }
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get("/api/alerts/recent?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
        assert_eq!(json[0]["rule_name"], "new", "newest first");
    }

    #[tokio::test]
    async fn recent_alerts_respects_since_filter() {
        use gulfwatch_core::alert::AlertEvent;
        let (state, _rx) = AppState::new(100, 10);
        let cutoff = chrono::Utc::now();
        {
            let mut buf = state.recent_alerts.write().await;
            buf.push_back(AlertEvent {
                rule_id: "r1".to_string(),
                rule_name: "before".to_string(),
                program_id: "prog".to_string(),
                metric: "x".to_string(),
                value: 1.0,
                threshold: 0.0,
                fired_at: cutoff - chrono::Duration::seconds(60),
            });
            buf.push_back(AlertEvent {
                rule_id: "r2".to_string(),
                rule_name: "after".to_string(),
                program_id: "prog".to_string(),
                metric: "x".to_string(),
                value: 2.0,
                threshold: 0.0,
                fired_at: cutoff + chrono::Duration::seconds(60),
            });
        }
        let app = crate::build_router(state);
        let url = format!(
            "/api/alerts/recent?since={}",
            urlencoding::encode(&cutoff.to_rfc3339())
        );
        let response = app
            .oneshot(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["rule_name"], "after");
    }

    #[tokio::test]
    async fn get_transaction_by_signature_scans_across_programs() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog_a".to_string()).await;
        state.add_program("prog_b".to_string()).await;
        {
            let mut windows = state.windows.write().await;
            let mut tx = make_tx("prog_b");
            tx.signature = "cross_sig".to_string();
            windows.get_mut("prog_b").unwrap().push(tx);
        }
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get("/api/transactions/cross_sig")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["program_id"], "prog_b");
    }

    #[tokio::test]
    async fn recent_transactions_supports_classification_filters() {
        let (state, _rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;

        {
            let mut windows = state.windows.write().await;
            let window = windows.get_mut("prog").unwrap();
            window.push(make_classified_tx("prog", "defi_swap", "swap", 0.95, true));
            window.push(make_classified_tx("prog", "token_transfer", "transfer", 0.80, false));
        }

        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get(
                    "/api/transactions/recent?program=prog&category=defi_swap&classifier=swap&min_confidence=0.9&has_debug=true",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["classification"]["category"], "defi_swap");
        assert_eq!(json[0]["classification"]["classifier"], "swap");
    }

    // ─── IDL routes ──────────────────────────────────────

    const SAMPLE_IDL: &str = r#"{
        "version": "0.1.0",
        "name": "jupiter",
        "instructions": [
            {"name": "route"},
            {"name": "swap"}
        ],
        "errors": [
            {"code": 6000, "name": "SlippageExceeded", "msg": "too much slippage"},
            {"code": 6001, "name": "InvalidRoute"}
        ]
    }"#;

    #[tokio::test]
    async fn post_idl_stores_and_returns_counts() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::post("/api/programs/jup_prog/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(SAMPLE_IDL))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["program_id"], "jup_prog");
        assert_eq!(json["name"], "jupiter");
        assert_eq!(json["instruction_count"], 2);
        assert_eq!(json["error_count"], 2);
    }

    #[tokio::test]
    async fn get_idl_roundtrips_stored_payload() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let _ = app
            .clone()
            .oneshot(
                Request::post("/api/programs/prog_x/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(SAMPLE_IDL))
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::get("/api/programs/prog_x/idl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "jupiter");
        assert_eq!(json["instructions"].as_array().unwrap().len(), 2);
        assert_eq!(json["errors"][0]["code"], 6000);
    }

    #[tokio::test]
    async fn idl_upload_does_not_require_program_to_be_monitored() {
        // Loose model: IDL registry is decoupled from the monitor list.
        let (state, _rx) = AppState::new(100, 10);
        assert!(
            state.monitored_programs.read().await.is_empty(),
            "no monitored programs"
        );
        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::post("/api/programs/unmonitored/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(SAMPLE_IDL))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_idl_for_unknown_program_returns_404() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let response = app
            .oneshot(
                Request::get("/api/programs/missing/idl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_idl_removes_and_second_delete_is_404() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let _ = app
            .clone()
            .oneshot(
                Request::post("/api/programs/prog_y/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(SAMPLE_IDL))
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::delete("/api/programs/prog_y/idl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(
                Request::delete("/api/programs/prog_y/idl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_idl_with_unknown_format_returns_400_with_reason_and_records_failure() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state.clone());

        let invalid = r#"{"instructions":[]}"#;
        let response = app
            .oneshot(
                Request::post("/api/programs/bad/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["program_id"], "bad");
        assert!(
            json["error"]
                .as_str()
                .map(|s| s.contains("unrecognized IDL format") || s.contains("name"))
                .unwrap_or(false),
            "error body should explain the rejection: {}",
            json["error"]
        );

        // Parse failures must become visible on the TUI via idl_failures.
        assert_eq!(
            state.get_idl_failure("bad").await.as_deref().is_some(),
            true
        );
    }

    #[tokio::test]
    async fn post_idl_accepts_anchor_v030_format_with_embedded_discriminator() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let v030 = r#"{
            "address": "Prog",
            "metadata": {"name":"newprog","version":"0.1.0","spec":"0.1.0"},
            "instructions": [
                {"name":"swap","discriminator":[229,23,203,151,122,227,173,42]}
            ]
        }"#;

        let response = app
            .oneshot(
                Request::post("/api/programs/newprog_id/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(v030))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "newprog");
        assert_eq!(json["format"], "AnchorV030");
        assert_eq!(json["instruction_count"], 1);
    }

    #[tokio::test]
    async fn post_idl_is_upsert_not_append() {
        let (state, _rx) = AppState::new(100, 10);
        let app = crate::build_router(state);

        let _ = app
            .clone()
            .oneshot(
                Request::post("/api/programs/prog_z/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(SAMPLE_IDL))
                    .unwrap(),
            )
            .await
            .unwrap();

        let v2 = r#"{"name":"jupiter_v2","instructions":[{"name":"newOp"}]}"#;
        let _ = app
            .clone()
            .oneshot(
                Request::post("/api/programs/prog_z/idl")
                    .header("content-type", "application/json")
                    .body(Body::from(v2))
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::get("/api/programs/prog_z/idl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "jupiter_v2");
        assert_eq!(json["instructions"].as_array().unwrap().len(), 1);
    }
}

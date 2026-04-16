use anyhow::Result;
use reqwest::Client;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct GulfwatchMcp {
    base_url: String,
    http: Client,
    #[allow(dead_code)]
    tool_router: ToolRouter<GulfwatchMcp>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecentTransactionsParams {
    program: Option<String>,
    limit: Option<usize>,
    category: Option<String>,
    classifier: Option<String>,
    min_confidence: Option<f32>,
    has_debug: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetTransactionParams {
    signature: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MetricsSummaryParams {
    program: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecentAlertsParams {
    since: Option<String>,
    limit: Option<usize>,
}

#[tool_router]
impl GulfwatchMcp {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            http: Client::new(),
            tool_router: Self::tool_router(),
        }
    }

    async fn fetch(&self, path: &str) -> Result<String, McpError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("request failed: {e}"), None))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| McpError::internal_error(format!("read body: {e}"), None))?;
        if status.is_client_error() || status.is_server_error() {
            return Err(McpError::internal_error(
                format!("gulfwatch returned {status}: {body}"),
                None,
            ));
        }
        Ok(body)
    }

    #[tool(
        description = "Fetch the most recent transactions from the rolling window. Optional filters: program (program id), limit (default 50), category, classifier, min_confidence, has_debug. Returns a JSON array of decoded transactions including instructions, logs, balance_diff, and tx_error."
    )]
    async fn recent_transactions(
        &self,
        Parameters(p): Parameters<RecentTransactionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut q: Vec<(String, String)> = Vec::new();
        if let Some(v) = p.program {
            q.push(("program".into(), v));
        }
        if let Some(v) = p.limit {
            q.push(("limit".into(), v.to_string()));
        }
        if let Some(v) = p.category {
            q.push(("category".into(), v));
        }
        if let Some(v) = p.classifier {
            q.push(("classifier".into(), v));
        }
        if let Some(v) = p.min_confidence {
            q.push(("min_confidence".into(), v.to_string()));
        }
        if let Some(v) = p.has_debug {
            q.push(("has_debug".into(), v.to_string()));
        }
        let qs = build_query(&q);
        let body = self
            .fetch(&format!("/api/transactions/recent{qs}"))
            .await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(
        description = "Look up a single transaction by signature. Works for ANY mainnet transaction, not just monitored programs or recent windows: tries the rolling window first, then falls back to fetching from Solana RPC and parsing through the same pipeline. Returns the full decoded tx with logs, balance_diff, tx_error, cu_profile, and parsed instructions. 404 only if the signature doesn't exist on chain."
    )]
    async fn get_transaction(
        &self,
        Parameters(p): Parameters<GetTransactionParams>,
    ) -> Result<CallToolResult, McpError> {
        let sig = urlencoding::encode(&p.signature);
        let body = self.fetch(&format!("/api/transactions/{sig}")).await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(
        description = "Rolling-window metrics summary for one program (when program is set) or all monitored programs. Returns tx count, error count, error rate, avg compute units, and top instruction types."
    )]
    async fn metrics_summary(
        &self,
        Parameters(p): Parameters<MetricsSummaryParams>,
    ) -> Result<CallToolResult, McpError> {
        let qs = match p.program {
            Some(prog) => format!("?program={}", urlencoding::encode(&prog)),
            None => String::new(),
        };
        let body = self.fetch(&format!("/api/metrics/summary{qs}")).await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(description = "List program ids currently being monitored.")]
    async fn list_programs(&self) -> Result<CallToolResult, McpError> {
        let body = self.fetch("/api/programs").await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(
        description = "List configured threshold alert rules (id, name, program, condition, webhook, enabled)."
    )]
    async fn list_alert_rules(&self) -> Result<CallToolResult, McpError> {
        let body = self.fetch("/api/alerts").await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(
        description = "Recent alert events that have fired (security detections + threshold rules), newest first. Optional 'since' is an RFC3339 timestamp filter; 'limit' defaults to 100."
    )]
    async fn recent_alerts(
        &self,
        Parameters(p): Parameters<RecentAlertsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut q: Vec<(String, String)> = Vec::new();
        if let Some(v) = p.since {
            q.push(("since".into(), v));
        }
        if let Some(v) = p.limit {
            q.push(("limit".into(), v.to_string()));
        }
        let qs = build_query(&q);
        let body = self.fetch(&format!("/api/alerts/recent{qs}")).await?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }
}

#[tool_handler]
impl ServerHandler for GulfwatchMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "GulfWatch MCP server. Read-only tools that wrap the GulfWatch REST API. Useful for asking 'why did this tx fail', 'what alerts fired in the last hour', 'what's the error rate on raydium right now'. Requires a running GulfWatch server reachable at GULFWATCH_BASE_URL (default http://localhost:3001).".to_string(),
            )
    }
}

fn build_query(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let mut s = String::from("?");
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            s.push('&');
        }
        s.push_str(&urlencoding::encode(k));
        s.push('=');
        s.push_str(&urlencoding::encode(v));
    }
    s
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let base_url =
        std::env::var("GULFWATCH_BASE_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());
    tracing::info!(base_url = %base_url, "Starting gulfwatch-mcp");

    let service = GulfwatchMcp::new(base_url).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

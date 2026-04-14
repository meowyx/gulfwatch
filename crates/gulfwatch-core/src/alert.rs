use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{info, error};

use crate::rolling_window::RollingWindow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub program_id: String,
    pub condition: AlertCondition,
    pub actions: Vec<AlertAction>,
    pub webhook_url: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertCondition {
    pub metric: AlertMetric,
    pub operator: AlertOperator,
    pub threshold: f64,
    pub window_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertMetric {
    ErrorRate,
    ErrorCount,
    TxCount,
    AvgComputeUnits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertOperator {
    Gt,
    Lt,
    Gte,
    Lte,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertAction {
    Webhook,
    Websocket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub rule_id: String,
    pub rule_name: String,
    pub program_id: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub fired_at: DateTime<Utc>,
}

pub struct AlertEngine {
    pub rules: Arc<RwLock<Vec<AlertRule>>>,
    windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
    alert_broadcast: broadcast::Sender<AlertEvent>,
    last_fired: HashMap<String, DateTime<Utc>>,
    cooldown_secs: i64,
}

impl AlertEngine {
    pub fn new(
        rules: Arc<RwLock<Vec<AlertRule>>>,
        windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
        alert_broadcast: broadcast::Sender<AlertEvent>,
        cooldown_secs: i64,
    ) -> Self {
        Self {
            rules,
            windows,
            alert_broadcast,
            last_fired: HashMap::new(),
            cooldown_secs,
        }
    }

    pub async fn run(&mut self, tick_interval: Duration) {
        info!("Alert engine started (tick every {:?})", tick_interval);

        loop {
            tokio::time::sleep(tick_interval).await;
            self.evaluate_all().await;
        }
    }

    async fn evaluate_all(&mut self) {
        let rules = self.rules.read().await.clone();
        let windows = self.windows.read().await;
        let now = Utc::now();

        for rule in &rules {
            if !rule.enabled {
                continue;
            }

            if let Some(last) = self.last_fired.get(&rule.id) {
                let elapsed = (now - *last).num_seconds();
                if elapsed < self.cooldown_secs {
                    continue;
                }
            }

            let window = match windows.get(&rule.program_id) {
                Some(w) => w,
                None => continue,
            };

            let summary = window.summary(&rule.program_id);
            let value = match rule.condition.metric {
                AlertMetric::ErrorRate => summary.error_rate,
                AlertMetric::ErrorCount => summary.error_count as f64,
                AlertMetric::TxCount => summary.tx_count as f64,
                AlertMetric::AvgComputeUnits => summary.avg_compute_units,
            };

            let triggered = match rule.condition.operator {
                AlertOperator::Gt => value > rule.condition.threshold,
                AlertOperator::Lt => value < rule.condition.threshold,
                AlertOperator::Gte => value >= rule.condition.threshold,
                AlertOperator::Lte => value <= rule.condition.threshold,
            };

            if triggered {
                let event = AlertEvent {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    program_id: rule.program_id.clone(),
                    metric: format!("{:?}", rule.condition.metric),
                    value,
                    threshold: rule.condition.threshold,
                    fired_at: now,
                };

                info!(
                    rule = %rule.name,
                    metric = %event.metric,
                    value = %value,
                    threshold = %rule.condition.threshold,
                    "Alert fired"
                );

                self.last_fired.insert(rule.id.clone(), now);

                if rule.actions.contains(&AlertAction::Websocket) {
                    let _ = self.alert_broadcast.send(event.clone());
                }

                if rule.actions.contains(&AlertAction::Webhook) {
                    if let Some(ref url) = rule.webhook_url {
                        let url = url.clone();
                        let event = event.clone();
                        tokio::spawn(async move {
                            if let Err(e) = fire_webhook(&url, &event).await {
                                error!(url = %url, error = %e, "Webhook delivery failed");
                            }
                        });
                    }
                }
            }
        }
    }
}

impl PartialEq for AlertAction {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (AlertAction::Webhook, AlertAction::Webhook)
                | (AlertAction::Websocket, AlertAction::Websocket)
        )
    }
}

async fn fire_webhook(
    url: &str,
    event: &AlertEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::to_string(event)?;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let tls = url.starts_with("https://");
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or("invalid URL scheme")?;

    let (host_port, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(i) => (
            host_port[..i].to_string(),
            host_port[i + 1..].parse::<u16>()?,
        ),
        None => (host_port.to_string(), if tls { 443 } else { 80 }),
    };

    let addr = format!("{}:{}", host, port);
    let tcp = TcpStream::connect(&addr).await?;

    let request = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        path, host, body.len(), body
    );

    if tls {
        let cx =
            tokio_native_tls::TlsConnector::from(native_tls::TlsConnector::new()?);
        let mut stream = cx.connect(&host, tcp).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
    } else {
        let mut tcp = tcp;
        tcp.write_all(request.as_bytes()).await?;
        let mut response = String::new();
        tcp.read_to_string(&mut response).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::Transaction;

    fn make_tx(program_id: &str, success: bool) -> Transaction {
        Transaction {
            signature: "sig".to_string(),
            program_id: program_id.to_string(),
            block_slot: 100,
            timestamp: Utc::now(),
            success,
            instruction_type: Some("swap".to_string()),
            accounts: vec![],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions: vec![],
            cu_profile: None,
            classification: None,
            classification_debug: None,
        }
    }

    #[tokio::test]
    async fn alert_fires_when_threshold_exceeded() {
        let rules = Arc::new(RwLock::new(vec![AlertRule {
            id: "rule1".to_string(),
            name: "High error rate".to_string(),
            program_id: "prog".to_string(),
            condition: AlertCondition {
                metric: AlertMetric::ErrorRate,
                operator: AlertOperator::Gt,
                threshold: 0.1,
                window_seconds: 60,
            },
            actions: vec![AlertAction::Websocket],
            webhook_url: None,
            enabled: true,
        }]));

        let windows = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut w = windows.write().await;
            let mut rw = RollingWindow::new(10);
            // 5 txs, 3 failed = 60% error rate > 10% threshold
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", false));
            rw.push(make_tx("prog", false));
            rw.push(make_tx("prog", false));
            w.insert("prog".to_string(), rw);
        }

        let (alert_tx, mut alert_rx) = broadcast::channel(10);
        let mut engine = AlertEngine::new(rules, windows, alert_tx, 30);

        engine.evaluate_all().await;

        let event = alert_rx.try_recv().unwrap();
        assert_eq!(event.rule_id, "rule1");
        assert_eq!(event.program_id, "prog");
        assert!(event.value > 0.1);
    }

    #[tokio::test]
    async fn alert_does_not_fire_below_threshold() {
        let rules = Arc::new(RwLock::new(vec![AlertRule {
            id: "rule1".to_string(),
            name: "High error rate".to_string(),
            program_id: "prog".to_string(),
            condition: AlertCondition {
                metric: AlertMetric::ErrorRate,
                operator: AlertOperator::Gt,
                threshold: 0.5,
                window_seconds: 60,
            },
            actions: vec![AlertAction::Websocket],
            webhook_url: None,
            enabled: true,
        }]));

        let windows = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut w = windows.write().await;
            let mut rw = RollingWindow::new(10);
            // 10% error rate < 50% threshold
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", true));
            rw.push(make_tx("prog", false));
            w.insert("prog".to_string(), rw);
        }

        let (alert_tx, mut alert_rx) = broadcast::channel(10);
        let mut engine = AlertEngine::new(rules, windows, alert_tx, 30);

        engine.evaluate_all().await;

        assert!(alert_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn alert_deduplication_cooldown() {
        let rules = Arc::new(RwLock::new(vec![AlertRule {
            id: "rule1".to_string(),
            name: "Always fires".to_string(),
            program_id: "prog".to_string(),
            condition: AlertCondition {
                metric: AlertMetric::TxCount,
                operator: AlertOperator::Gt,
                threshold: 0.0,
                window_seconds: 60,
            },
            actions: vec![AlertAction::Websocket],
            webhook_url: None,
            enabled: true,
        }]));

        let windows = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut w = windows.write().await;
            let mut rw = RollingWindow::new(10);
            rw.push(make_tx("prog", true));
            w.insert("prog".to_string(), rw);
        }

        let (alert_tx, mut alert_rx) = broadcast::channel(10);
        let mut engine = AlertEngine::new(rules, windows, alert_tx, 300);

        // First evaluation fires
        engine.evaluate_all().await;
        assert!(alert_rx.try_recv().is_ok());

        // Second evaluation within cooldown does NOT fire
        engine.evaluate_all().await;
        assert!(alert_rx.try_recv().is_err());
    }
}

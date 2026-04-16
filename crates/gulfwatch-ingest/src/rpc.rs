use serde_json::{json, Value};

pub async fn fetch_transaction(rpc_url: &str, signature: &str) -> reqwest::Result<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [
            signature,
            { "encoding": "json", "maxSupportedTransactionVersion": 0 }
        ]
    });
    reqwest::Client::new()
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json::<Value>()
        .await
}

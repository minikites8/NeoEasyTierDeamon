use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures_util::FutureExt;
use hmac::{Hmac, Mac};
use reqwest::Client as HttpClient;
use rust_socketio::{
    asynchronous::{Client, ClientBuilder, ReconnectSettings},
    Payload, TransportType,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::config::SocketConnectionConfig;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocketNodeData {
    pub id: i32,
    pub node_id: i32,
    pub protocol: String,
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub connect_url: String,
    #[serde(default)]
    pub latest_alter: String,
    pub is_relay: bool,
    pub network_name: String,
    pub network_secret: String,
    #[serde(default)]
    pub maximum_bandwidth: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingReport {
    pub node_id: i32,
    pub ping: i32,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendPeer {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sponsor: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub allow_relay: Option<bool>,
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub network_name: Option<String>,
    #[serde(default)]
    pub network_secret: Option<String>,
    pub status: String,
    #[serde(default)]
    pub latency_ms: Option<i32>,
    #[serde(default)]
    pub peer: Option<i32>,
    #[serde(default)]
    pub last_heartbeat: Option<String>,
}

impl From<&SocketNodeData> for BackendPeer {
    fn from(node: &SocketNodeData) -> Self {
        BackendPeer {
            id: node.id,
            name: format!("{}-{}", node.network_name, node.node_id),
            description: None,
            sponsor: None,
            location: None,
            allow_relay: Some(node.is_relay),
            public_ip: Some(format!("{}:{}", node.host, node.port)),
            protocol: Some(node.protocol.clone()),
            network_name: Some(node.network_name.clone()),
            network_secret: Some(node.network_secret.clone()),
            status: "unknown".to_string(),
            latency_ms: None,
            peer: None,
            last_heartbeat: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenExchangeRequest {
    cluster_id: String,
    signature: String,
    challenge: String,
    timestamp: i64,
    nonce: String,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Option<Instant>,
}

#[derive(Clone)]
pub struct TokenManager {
    http: HttpClient,
    config: SocketConnectionConfig,
    token: Arc<Mutex<Option<CachedToken>>>,
}

impl TokenManager {
    pub fn new(config: SocketConnectionConfig) -> Result<Self> {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("Failed to build HTTP client for token manager")?;

        Ok(Self {
            http,
            config,
            token: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn get_token(&self) -> Result<String> {
        let mut guard = self.token.lock().await;
        if let Some(cached) = guard.as_ref() {
            let valid = cached
                .expires_at
                .map(|exp| Instant::now() < exp)
                .unwrap_or(true);
            if valid {
                return Ok(cached.token.clone());
            }
        }

        let (token, expires_in) = self.fetch_token().await?;
        let expires_at = expires_in.map(|ttl| {
            // refresh 10s early
            let refresh_in = ttl.saturating_sub(10);
            Instant::now() + Duration::from_secs(refresh_in)
        });

        *guard = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    pub async fn invalidate_token(&self) {
        let mut guard = self.token.lock().await;
        *guard = None;
    }

    async fn fetch_token(&self) -> Result<(String, Option<u64>)> {
        let challenge_url = build_url(&self.config.url, &self.config.challenge_path);
        let token_url = build_url(&self.config.url, &self.config.token_path);

        let challenge_resp = self
            .http
            .get(&challenge_url)
            .query(&[("clusterId", self.config.cluster_id.as_str())])
            .header("User-Agent", self.config.user_agent.clone())
            .send()
            .await
            .context("Failed to request challenge")?;

        if !challenge_resp.status().is_success() {
            bail!(
                "Challenge request failed: status={} body={}",
                challenge_resp.status(),
                challenge_resp.text().await.unwrap_or_default()
            );
        }

        let challenge_text = challenge_resp
            .text()
            .await
            .context("Failed to read challenge response body")?;
        let challenge_value: Value = serde_json::from_str(&challenge_text).with_context(|| {
            format!(
                "Failed to parse challenge response as JSON, body={}",
                truncate_for_log(&challenge_text)
            )
        })?;
        let (challenge, challenge_timestamp, challenge_nonce) =
            parse_challenge_response(&challenge_value).with_context(|| {
                format!(
                    "Failed to parse challenge response fields, body={}",
                    truncate_for_log(&challenge_text)
                )
            })?;

        let timestamp = challenge_timestamp
            .unwrap_or_else(|| Utc::now().timestamp());
        let nonce = challenge_nonce
            .unwrap_or_else(|| Utc::now().timestamp_millis().to_string());

        let signature = sign_challenge(
            &self.config.cluster_secret,
            &self.config.cluster_id,
            &challenge,
            timestamp,
            &nonce,
        )?;

        let exchange_body = TokenExchangeRequest {
            cluster_id: self.config.cluster_id.clone(),
            signature,
            challenge,
            timestamp,
            nonce,
        };

        let token_resp = self
            .http
            .post(&token_url)
            .header("User-Agent", self.config.user_agent.clone())
            .json(&exchange_body)
            .send()
            .await
            .context("Failed to exchange challenge for token")?;

        if !token_resp.status().is_success() {
            bail!(
                "Token exchange failed: status={} body={}",
                token_resp.status(),
                token_resp.text().await.unwrap_or_default()
            );
        }

        let token_text = token_resp
            .text()
            .await
            .context("Failed to read token response body")?;
        let token_value: Value = serde_json::from_str(&token_text).with_context(|| {
            format!(
                "Failed to parse token response as JSON, body={}",
                truncate_for_log(&token_text)
            )
        })?;
        let (token, expires_in) = parse_token_response(&token_value).with_context(|| {
            format!(
                "Failed to parse token response fields, body={}",
                truncate_for_log(&token_text)
            )
        })?;

        if token.is_empty() {
            bail!("Empty token from token exchange endpoint");
        }

        let ttl = expires_in
            .or(Some(self.config.token_ttl_seconds))
            .filter(|v| *v > 0);

        Ok((token, ttl))
    }
}

fn truncate_for_log(s: &str) -> String {
    const LIMIT: usize = 512;
    if s.len() <= LIMIT {
        s.to_string()
    } else {
        format!("{}...(truncated)", &s[..LIMIT])
    }
}

fn as_object_candidates<'a>(value: &'a Value) -> Vec<&'a serde_json::Map<String, Value>> {
    let mut out = Vec::new();
    if let Some(obj) = value.as_object() {
        out.push(obj);
        for key in ["data", "result", "payload"] {
            if let Some(nested) = obj.get(key).and_then(|v| v.as_object()) {
                out.push(nested);
            }
        }
    }
    out
}

fn parse_challenge_response(value: &Value) -> Result<(String, Option<i64>, Option<String>)> {
    for obj in as_object_candidates(value) {
        let challenge = obj
            .get("challenge")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("challengeStr").and_then(|v| v.as_str()))
            .map(str::to_string);
        if let Some(challenge) = challenge {
            let timestamp = obj
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .or_else(|| obj.get("ts").and_then(|v| v.as_i64()))
                .or_else(|| obj.get("time").and_then(|v| v.as_i64()));
            let nonce = obj
                .get("nonce")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("random").and_then(|v| v.as_str()))
                .map(str::to_string);
            return Ok((challenge, timestamp, nonce));
        }
    }

    bail!("missing field `challenge`")
}

fn parse_token_response(value: &Value) -> Result<(String, Option<u64>)> {
    for obj in as_object_candidates(value) {
        let token = obj
            .get("token")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("accessToken").and_then(|v| v.as_str()))
            .map(str::to_string);
        if let Some(token) = token {
            let expires_in = obj
                .get("expiresIn")
                .and_then(|v| v.as_u64())
                .or_else(|| obj.get("expires_in").and_then(|v| v.as_u64()))
                .or_else(|| obj.get("ttl").and_then(|v| v.as_u64()));
            return Ok((token, expires_in));
        }
    }

    bail!("missing field `token`")
}

fn sign_challenge(
    cluster_secret: &str,
    cluster_id: &str,
    challenge: &str,
    timestamp: i64,
    nonce: &str,
) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(cluster_secret.as_bytes())
        .context("Failed to initialize HMAC with cluster secret")?;

    // Canonical payload: clusterId\nchallenge\ntimestamp\nnonce
    let payload = format!("{}\n{}\n{}\n{}", cluster_id, challenge, timestamp, nonce);
    mac.update(payload.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn build_url(base: &str, path: &str) -> String {
    if base.ends_with('/') {
        format!("{}{}", base.trim_end_matches('/'), path)
    } else {
        format!("{}{}", base, path)
    }
}

fn is_auth_error_payload(payload: &Payload) -> bool {
    fn string_has_auth_keyword(s: &str) -> bool {
        let v = s.to_lowercase();
        v.contains("auth") || v.contains("token") || v.contains("unauthorized")
    }

    match payload {
        Payload::Text(values) => values.iter().any(|value| {
            if let Some(obj) = value.as_object() {
                for key in ["error", "message", "reason", "code"] {
                    if let Some(field) = obj.get(key) {
                        if let Some(s) = field.as_str() {
                            if string_has_auth_keyword(s) {
                                return true;
                            }
                        } else if field.is_number() && field.to_string() == "401" {
                            return true;
                        }
                    }
                }
            }
            value
                .as_str()
                .map(string_has_auth_keyword)
                .unwrap_or(false)
        }),
        Payload::String(s) => string_has_auth_keyword(s),
        Payload::Binary(_) => false,
    }
}

pub struct SocketBackendClient {
    socket: Client,
    nodes: Arc<RwLock<Vec<SocketNodeData>>>,
}

impl SocketBackendClient {
    pub async fn new(config: &SocketConnectionConfig) -> Result<Self> {
        let nodes: Arc<RwLock<Vec<SocketNodeData>>> = Arc::new(RwLock::new(Vec::new()));
        let nodes_for_cb = nodes.clone();

        let token_manager = Arc::new(TokenManager::new(config.clone())?);
        let token = token_manager
            .get_token()
            .await
            .context("Failed to fetch initial auth token")?;

        let full_url = build_url(&config.url, &config.socket_path);

        let tm_for_reconnect = token_manager.clone();

        let socket = ClientBuilder::new(full_url)
            .transport_type(TransportType::Websocket)
            .opening_header("User-Agent", config.user_agent.clone())
            .auth(json!({ "token": token }))
            .on_reconnect(move || {
                let tm = tm_for_reconnect.clone();
                async move {
                    let mut settings = ReconnectSettings::new();
                    match tm.get_token().await {
                        Ok(token) => settings.auth(json!({ "token": token })),
                        Err(e) => error!("Failed to refresh auth token for reconnect: {}", e),
                    }
                    settings
                }
                .boxed()
            })
            .on("nodes", move |payload: Payload, _: Client| {
                let nodes = nodes_for_cb.clone();
                async move {
                    match payload {
                        Payload::Text(values) => {
                            if let Some(first) = values.first() {
                                match serde_json::from_value::<Vec<SocketNodeData>>(first.clone()) {
                                    Ok(node_list) => {
                                        let count = node_list.len();
                                        let mut guard = nodes.write().await;
                                        *guard = node_list;
                                        info!("Received {} nodes from Socket.IO server", count);
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse node list from 'nodes' event: {}", e);
                                        debug!("Raw payload: {:?}", values);
                                    }
                                }
                            }
                        }
                        other => warn!("Unexpected payload type in 'nodes' event: {:?}", other),
                    }
                }
                .boxed()
            })
            .on("error", {
                let tm = token_manager.clone();
                move |err: Payload, _: Client| {
                    let tm = tm.clone();
                    async move {
                        error!("Socket.IO server error: {:?}", err);
                        // If auth rejected, clear token so next reconnect fetches a new one
                        if is_auth_error_payload(&err) {
                            tm.invalidate_token().await;
                            warn!("Auth-related socket error detected, cached token invalidated");
                        }
                    }
                    .boxed()
                }
            })
            .on("disconnect", |_: Payload, _: Client| {
                async move {
                    warn!("Disconnected from Socket.IO server");
                }
                .boxed()
            })
            .connect()
            .await
            .context("Failed to connect to Socket.IO server")?;

        info!("Connected to Socket.IO server at {}", config.url);

        Ok(Self { socket, nodes })
    }

    pub async fn get_nodes(&self) -> Vec<SocketNodeData> {
        self.nodes.read().await.clone()
    }

    pub async fn fetch_peers(&self) -> Result<Vec<BackendPeer>> {
        let nodes = self.get_nodes().await;
        Ok(nodes.iter().map(BackendPeer::from).collect())
    }

    pub async fn submit_ping_reports(&self, reports: Vec<PingReport>) -> Result<()> {
        if reports.is_empty() {
            return Ok(());
        }

        let payload = serde_json::to_value(&reports)
            .context("Failed to serialize ping reports")?;
        self.socket
            .emit("report", payload)
            .await
            .context("Failed to emit 'report' event to Socket.IO server")?;

        debug!("Submitted {} ping report(s) to server", reports.len());
        Ok(())
    }

    pub async fn disconnect(self) -> Result<()> {
        self.socket
            .disconnect()
            .await
            .context("Failed to disconnect from Socket.IO server")?;
        Ok(())
    }
}

pub fn make_ping_report(node_id: i32, ping_ms: i32) -> PingReport {
    PingReport {
        node_id,
        ping: ping_ms,
        timestamp: Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_node_data_deserialization() {
        let json = r#"{
            "id": 1,
            "nodeId": 1,
            "protocol": "wss",
            "host": "frp.tianpao.top",
            "port": 11010,
            "connectUrl": "8141e9cdd271.enode.1tmc.top",
            "latestAlter": "2026-02-23T11:43:15.847Z",
            "isRelay": false,
            "networkName": "Tianpao",
            "networkSecret": "114514",
            "maximumBandwidth": 10000
        }"#;

        let node: SocketNodeData = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, 1);
        assert_eq!(node.node_id, 1);
        assert_eq!(node.protocol, "wss");
        assert_eq!(node.host, "frp.tianpao.top");
        assert_eq!(node.port, 11010);
        assert_eq!(node.connect_url, "8141e9cdd271.enode.1tmc.top");
        assert!(!node.is_relay);
        assert_eq!(node.network_name, "Tianpao");
        assert_eq!(node.network_secret, "114514");
        assert_eq!(node.maximum_bandwidth, 10000);
    }

    #[test]
    fn test_socket_node_data_relay_deserialization() {
        let json = r#"{
            "id": 2,
            "nodeId": 2,
            "protocol": "tcp",
            "host": "ald.tianpao.top",
            "port": 11010,
            "connectUrl": "",
            "latestAlter": "2026-02-28T18:11:19.765Z",
            "isRelay": true,
            "networkName": "Tianpao",
            "networkSecret": "114514",
            "maximumBandwidth": 1000
        }"#;

        let node: SocketNodeData = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, 2);
        assert!(node.is_relay);
    }

    #[test]
    fn test_backend_peer_from_socket_node() {
        let node = SocketNodeData {
            id: 3,
            node_id: 3,
            protocol: "tcp".to_string(),
            host: "www.gov.cn".to_string(),
            port: 11010,
            connect_url: "".to_string(),
            latest_alter: "2026-03-01T05:29:12.655Z".to_string(),
            is_relay: true,
            network_name: "Tianpao".to_string(),
            network_secret: "114514".to_string(),
            maximum_bandwidth: 100000,
        };

        let peer = BackendPeer::from(&node);
        assert_eq!(peer.id, 3);
        assert_eq!(peer.name, "Tianpao-3");
        assert_eq!(peer.public_ip, Some("www.gov.cn:11010".to_string()));
        assert_eq!(peer.protocol, Some("tcp".to_string()));
        assert_eq!(peer.allow_relay, Some(true));
        assert_eq!(peer.network_name, Some("Tianpao".to_string()));
        assert_eq!(peer.network_secret, Some("114514".to_string()));
    }

    #[test]
    fn test_ping_report_serialization() {
        let report = PingReport {
            node_id: 1,
            ping: 42,
            timestamp: "2026-03-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"nodeId\":1"));
        assert!(json.contains("\"ping\":42"));
        assert!(json.contains("\"timestamp\""));
    }

    #[test]
    fn test_make_ping_report() {
        let report = make_ping_report(5, 123);
        assert_eq!(report.node_id, 5);
        assert_eq!(report.ping, 123);
        assert!(!report.timestamp.is_empty());
    }

    #[test]
    fn test_sign_challenge_is_deterministic() {
        let sig1 = sign_challenge("secret", "cluster-1", "ch", 100, "n").unwrap();
        let sig2 = sign_challenge("secret", "cluster-1", "ch", 100, "n").unwrap();
        assert_eq!(sig1, sig2);
        assert!(!sig1.is_empty());
    }

    #[test]
    fn test_parse_challenge_response_top_level() {
        let v = serde_json::json!({
            "challenge": "abc",
            "timestamp": 100,
            "nonce": "xyz"
        });
        let (challenge, ts, nonce) = parse_challenge_response(&v).unwrap();
        assert_eq!(challenge, "abc");
        assert_eq!(ts, Some(100));
        assert_eq!(nonce.as_deref(), Some("xyz"));
    }

    #[test]
    fn test_parse_challenge_response_nested_data() {
        let v = serde_json::json!({
            "success": true,
            "data": {
                "challenge": "abc"
            }
        });
        let (challenge, ts, nonce) = parse_challenge_response(&v).unwrap();
        assert_eq!(challenge, "abc");
        assert!(ts.is_none());
        assert!(nonce.is_none());
    }

    #[test]
    fn test_parse_token_response_nested_result() {
        let v = serde_json::json!({
            "code": 0,
            "result": {
                "token": "tok",
                "expiresIn": 60
            }
        });
        let (token, expires_in) = parse_token_response(&v).unwrap();
        assert_eq!(token, "tok");
        assert_eq!(expires_in, Some(60));
    }

    #[tokio::test]
    async fn test_token_manager_invalidate() {
        let cfg = SocketConnectionConfig {
            url: "http://localhost:8081".to_string(),
            user_agent: "ua".to_string(),
            cluster_id: "cid".to_string(),
            cluster_secret: "sec".to_string(),
            socket_path: "/api/socket.io".to_string(),
            challenge_path: "/api/cluster/challenge".to_string(),
            token_path: "/api/cluster/token".to_string(),
            token_ttl_seconds: 60,
        };
        let tm = TokenManager::new(cfg).unwrap();
        {
            let mut g = tm.token.lock().await;
            *g = Some(CachedToken {
                token: "abc".to_string(),
                expires_at: None,
            });
        }
        tm.invalidate_token().await;
        let g = tm.token.lock().await;
        assert!(g.is_none());
    }
}

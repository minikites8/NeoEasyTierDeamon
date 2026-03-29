use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::FutureExt;
use rust_socketio::{
    asynchronous::{Client, ClientBuilder},
    Payload, TransportType,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::config::SocketConnectionConfig;

// ---------------------------------------------------------------------------
// Data structures received from the Socket.IO server
// ---------------------------------------------------------------------------

/// Node data pushed by the server over the "nodes" event.
/// Field names use camelCase as sent by the server.
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

// ---------------------------------------------------------------------------
// Data structure submitted to the server via the "report" event
// ---------------------------------------------------------------------------

/// Single-node ping entry sent in the "report" event payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingReport {
    /// nodeId as received in SocketNodeData
    pub node_id: i32,
    /// Round-trip latency in milliseconds; 0 when the node is unreachable
    pub ping: i32,
    /// ISO-8601 timestamp of when the measurement was taken
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Legacy BackendPeer kept for compatibility with sync_peers_to_db
// ---------------------------------------------------------------------------

/// Backend peer representation used internally by the sync logic.
/// Converted from SocketNodeData when nodes are received.
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
    /// "host:port" combined
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

// ---------------------------------------------------------------------------
// Socket.IO backend client
// ---------------------------------------------------------------------------

/// Persistent Socket.IO client that:
/// - Connects to the upstream server with cluster authentication
/// - Stores the latest node list received via the "nodes" event
/// - Emits ping reports via the "report" event
pub struct SocketBackendClient {
    socket: Client,
    /// Shared node list updated whenever a "nodes" event arrives
    nodes: Arc<RwLock<Vec<SocketNodeData>>>,
}

impl SocketBackendClient {
    /// Connect to the Socket.IO server and register event handlers.
    pub async fn new(config: &SocketConnectionConfig) -> Result<Self> {
        let nodes: Arc<RwLock<Vec<SocketNodeData>>> = Arc::new(RwLock::new(Vec::new()));
        let nodes_for_cb = nodes.clone();

        // Combine base URL with socket path (rust_socketio uses URL path for the socket endpoint)
        let full_url = if config.url.ends_with('/') {
            format!(
                "{}{}",
                config.url.trim_end_matches('/'),
                config.socket_path
            )
        } else {
            format!("{}{}", config.url, config.socket_path)
        };

        let socket = ClientBuilder::new(full_url)
            .transport_type(TransportType::Websocket)
            .opening_header("User-Agent", config.user_agent.clone())
            .auth(json!({
                "clusterId":     config.cluster_id,
                "clusterSecret": config.cluster_secret
            }))
            .on("nodes", move |payload: Payload, _: Client| {
                let nodes = nodes_for_cb.clone();
                async move {
                    match payload {
                        Payload::Text(values) => {
                            // The first argument of the event is the node array
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
                        other => {
                            warn!("Unexpected payload type in 'nodes' event: {:?}", other);
                        }
                    }
                }
                .boxed()
            })
            .on("error", |err: Payload, _: Client| {
                async move {
                    error!("Socket.IO server error: {:?}", err);
                }
                .boxed()
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

    /// Return a snapshot of the most recently received node list.
    pub async fn get_nodes(&self) -> Vec<SocketNodeData> {
        self.nodes.read().await.clone()
    }

    /// Return the most recently received nodes converted to BackendPeer for
    /// compatibility with the existing sync_peers_to_db helper.
    pub async fn fetch_peers(&self) -> Result<Vec<BackendPeer>> {
        let nodes = self.get_nodes().await;
        Ok(nodes.iter().map(BackendPeer::from).collect())
    }

    /// Emit a batch of ping reports to the server via the "report" event.
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

    /// Disconnect cleanly from the server.
    pub async fn disconnect(self) -> Result<()> {
        self.socket
            .disconnect()
            .await
            .context("Failed to disconnect from Socket.IO server")?;
        Ok(())
    }
}

/// Build a `PingReport` with the current UTC timestamp.
/// `ping_ms` is the round-trip time in milliseconds (≥ 0); pass 0 for unreachable nodes.
pub fn make_ping_report(node_id: i32, ping_ms: i32) -> PingReport {
    PingReport {
        node_id,
        ping: ping_ms,
        timestamp: Utc::now().to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
}

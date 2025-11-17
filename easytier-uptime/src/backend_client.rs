use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Backend API client for distributed probe mode
pub struct BackendClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

/// Peer node information from backend
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

/// Response from GET /node-status endpoint (for getting node IDs)
#[derive(Debug, Deserialize)]
pub struct NodeStatus {
    pub node_id: i32,
    pub status: String,
    #[serde(default)]
    pub latency_ms: Option<i32>,
    #[serde(default)]
    pub peer: Option<i32>,
    #[serde(default)]
    pub last_heartbeat: Option<String>,
}

/// Private node information from GET /nodes/{node_id}/private-info
#[derive(Debug, Deserialize)]
pub struct NodePrivateInfo {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sponsor: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub allow_relay: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub network_name: Option<String>,
    #[serde(default)]
    pub network_secret: Option<String>,
}

/// Response from GET /peers endpoint (deprecated, keeping for compatibility)
#[derive(Debug, Deserialize)]
pub struct PeersResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<PeersData>,
}

#[derive(Debug, Deserialize)]
pub struct PeersData {
    pub peers: Vec<BackendPeer>,
    pub total_available: i32,
    pub next_batch_available: bool,
}

/// Request body for POST /nodes/:node_id/heartbeat endpoint
#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    pub status: String,
    pub peer: i32,
    pub latency_ms: i32,
}

/// Response from POST /nodes/:node_id/heartbeat endpoint
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    pub success: bool,
    pub heartbeat: HeartbeatData,
    #[serde(rename = "nodeStatus")]
    pub node_status: NodeStatusData,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatData {
    pub id: i32,
    pub node_id: i32,
    pub status: String,
    pub peer: i32,
    pub latency_ms: i32,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct NodeStatusData {
    pub node_id: i32,
    pub status: String,
    pub latency_ms: i32,
    pub peer: i32,
    pub last_heartbeat: String,
}

impl BackendClient {
    /// Create a new backend client
    pub fn new(
        base_url: String,
        api_key: Option<String>,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url,
            api_key,
        })
    }

    /// Fetch peers from backend using two-step process:
    /// 1. GET /node-status to get all node IDs (no auth)
    /// 2. GET /nodes/{node_id}/private-info to get connection details (with auth)
    pub async fn fetch_peers(&self, region: Option<&str>) -> Result<Vec<BackendPeer>> {
        // Step 1: Get all node statuses (no authentication)
        let node_status_url = format!("{}/node-status", self.base_url);
        debug!("Fetching node statuses from backend: {}", node_status_url);

        let request = self.client.get(&node_status_url)
            .header("user-agent", "easytier-uptime");

        let response = request
            .send()
            .await
            .context("Failed to send request to backend for node statuses")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to fetch node statuses from backend: status={}, error={}",
                status,
                error_text
            );
        }

        let node_statuses: Vec<NodeStatus> = response
            .json()
            .await
            .context("Failed to parse node statuses response")?;

        info!("Fetched {} node statuses from backend", node_statuses.len());

        // Step 2: Fetch private info for each node (with authentication)
        let mut peers = Vec::new();
        
        for node_status in node_statuses {
            let private_info_url = format!("{}/nodes/{}/private-info", self.base_url, node_status.node_id);
            debug!("Fetching private info for node {}: {}", node_status.node_id, private_info_url);

            let mut request = self.client.get(&private_info_url)
                .header("user-agent", "easytier-uptime");

            // Add API key authentication using Bearer token
            if let Some(api_key) = &self.api_key {
                request = request.header("authorization", format!("Bearer {}", api_key));
            }

            match request.send().await {
                Ok(response) => {
                    let status_code = response.status();
                    if !status_code.is_success() {
                        let error_text = response.text().await.unwrap_or_default();
                        warn!(
                            "Failed to fetch private info for node {}: status={}, error={}",
                            node_status.node_id, status_code, error_text
                        );
                        continue;
                    }

                    match response.json::<NodePrivateInfo>().await {
                        Ok(private_info) => {
                            // Combine node status and private info into BackendPeer
                            let peer = BackendPeer {
                                id: private_info.id,
                                name: private_info.name,
                                description: private_info.description,
                                sponsor: private_info.sponsor,
                                location: private_info.location,
                                allow_relay: private_info.allow_relay,
                                public_ip: private_info.public_ip,
                                protocol: private_info.protocol,
                                network_name: private_info.network_name,
                                network_secret: private_info.network_secret,
                                status: node_status.status.clone(),
                                latency_ms: node_status.latency_ms,
                                peer: node_status.peer,
                                last_heartbeat: node_status.last_heartbeat.clone(),
                            };
                            peers.push(peer);
                        }
                        Err(e) => {
                            warn!("Failed to parse private info for node {}: {}", node_status.node_id, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to fetch private info for node {}: {}", node_status.node_id, e);
                }
            }
        }

        info!("Successfully fetched detailed info for {} peers from backend", peers.len());
        Ok(peers)
    }

    /// Report node status to backend via heartbeat endpoint
    pub async fn report_status(
        &self,
        node_id: i32,
        status: &str,
        latency_ms: i32,
        peer: i32,
    ) -> Result<()> {
        let url = format!("{}/nodes/{}/heartbeat", self.base_url, node_id);

        debug!("Reporting heartbeat to backend: {} for node id={}", url, node_id);

        let request_body = HeartbeatRequest {
            status: status.to_string(),
            peer,
            latency_ms,
        };

        let mut request = self.client.post(&url).json(&request_body);
        
        request = request.header("user-agent", "easytier-uptime");

        // Add API key authentication using Bearer token
        if let Some(api_key) = &self.api_key {
            request = request.header("authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .context("Failed to send heartbeat to backend")?;

        let status_code = response.status();
        if !status_code.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to report heartbeat to backend: status={}, error={}",
                status_code,
                error_text
            );
        }

        let heartbeat_response: HeartbeatResponse = response
            .json()
            .await
            .context("Failed to parse heartbeat response")?;

        if !heartbeat_response.success {
            anyhow::bail!("Backend returned success=false for heartbeat");
        }

        debug!("Successfully reported heartbeat to backend");
        Ok(())
    }

    /// Test backend connection
    pub async fn test_connection(&self) -> Result<()> {
        let url = format!("{}/node-status", self.base_url);
        debug!("Testing backend connection: {}", url);

        let request = self.client.get(&url)
            .header("user-agent", "easytier-uptime");

        let response = request
            .send()
            .await
            .context("Failed to connect to backend")?;

        if response.status().is_success() {
            info!("Backend connection test successful");
            Ok(())
        } else {
            anyhow::bail!("Backend connection test failed: status={}", response.status())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_client_creation() {
        let client = BackendClient::new(
            "http://localhost:8080".to_string(),
            Some("test-api-key".to_string()),
        );
        assert!(client.is_ok());
    }

    #[test]
    fn test_backend_peer_deserialization_with_network_secret() {
        // Test that network_secret is properly deserialized
        let json = r#"{
            "id": 1,
            "name": "test-peer",
            "network_name": "test-network",
            "network_secret": "secret123",
            "status": "online",
            "latency_ms": 50,
            "peer": 2,
            "public_ip": "192.168.1.1",
            "protocol": "tcp"
        }"#;
        
        let peer: Result<BackendPeer, _> = serde_json::from_str(json);
        assert!(peer.is_ok());
        let peer = peer.unwrap();
        assert_eq!(peer.network_secret, Some("secret123".to_string()));
    }

    #[test]
    fn test_backend_peer_deserialization_without_network_secret() {
        // Test that network_secret defaults to None when not provided
        let json = r#"{
            "id": 1,
            "name": "test-peer",
            "status": "online",
            "latency_ms": 50,
            "peer": 2
        }"#;
        
        let peer: Result<BackendPeer, _> = serde_json::from_str(json);
        assert!(peer.is_ok());
        let peer = peer.unwrap();
        assert_eq!(peer.network_secret, None);
        assert_eq!(peer.network_name, None);
    }

    #[test]
    fn test_node_status_deserialization() {
        let json = r#"{
            "node_id": 0,
            "status": "online",
            "latency_ms": 50,
            "peer": 2,
            "last_heartbeat": "2025-11-17T13:01:33.407Z"
        }"#;
        
        let node_status: Result<NodeStatus, _> = serde_json::from_str(json);
        assert!(node_status.is_ok());
        let node_status = node_status.unwrap();
        assert_eq!(node_status.node_id, 0);
        assert_eq!(node_status.status, "online");
    }

    #[test]
    fn test_node_private_info_deserialization() {
        let json = r#"{
            "id": 0,
            "name": "string",
            "protocol": "string",
            "description": "string",
            "sponsor": "string",
            "location": "string",
            "allow_relay": true,
            "created_at": "2025-11-17T13:05:31.321Z",
            "updated_at": "2025-11-17T13:05:31.321Z",
            "public_ip": "string",
            "network_name": "string",
            "network_secret": "string"
        }"#;
        
        let private_info: Result<NodePrivateInfo, _> = serde_json::from_str(json);
        assert!(private_info.is_ok());
        let private_info = private_info.unwrap();
        assert_eq!(private_info.id, 0);
        assert_eq!(private_info.network_secret, Some("string".to_string()));
    }

    #[test]
    fn test_heartbeat_response_deserialization() {
        let json = r#"{
            "success": true,
            "heartbeat": {
                "id": 0,
                "node_id": 0,
                "status": "online",
                "peer": 2,
                "latency_ms": 50,
                "timestamp": "2025-11-17T12:59:28.437Z"
            },
            "nodeStatus": {
                "node_id": 0,
                "status": "online",
                "latency_ms": 50,
                "peer": 2,
                "last_heartbeat": "2025-11-17T12:59:28.437Z"
            }
        }"#;
        
        let response: Result<HeartbeatResponse, _> = serde_json::from_str(json);
        assert!(response.is_ok());
        let response = response.unwrap();
        assert!(response.success);
        assert_eq!(response.heartbeat.node_id, 0);
    }
}

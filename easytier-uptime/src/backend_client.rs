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
    node_token: Option<String>,
    api_key: Option<String>,
}

/// Peer node information from backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendPeer {
    pub id: i32,
    pub name: String,
    pub host: String,
    pub port: i32,
    pub protocol: String,
    pub network_name: String,
    pub status: String,
    pub response_time: Option<i32>,
}

/// Response from GET /peers endpoint
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

/// Request body for PUT /nodes/status endpoint
#[derive(Debug, Serialize)]
pub struct NodeStatusRequest {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Response from PUT /nodes/status endpoint
#[derive(Debug, Deserialize)]
pub struct NodeStatusResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl BackendClient {
    /// Create a new backend client
    pub fn new(
        base_url: String,
        node_token: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url,
            node_token,
            api_key,
        })
    }

    /// Fetch peers from backend
    pub async fn fetch_peers(&self, region: Option<&str>) -> Result<Vec<BackendPeer>> {
        let mut url = format!("{}/peers", self.base_url);
        if let Some(r) = region {
            url.push_str(&format!("?region={}", r));
        }

        debug!("Fetching peers from backend: {}", url);

        let mut request = self.client.get(&url);

        // Add API key if available
        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .context("Failed to send request to backend")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to fetch peers from backend: status={}, error={}",
                status,
                error_text
            );
        }

        let peers_response: PeersResponse = response
            .json()
            .await
            .context("Failed to parse peers response")?;

        if peers_response.code != 200 {
            anyhow::bail!(
                "Backend returned error: code={}, message={}",
                peers_response.code,
                peers_response.message
            );
        }

        let peers = peers_response
            .data
            .map(|d| d.peers)
            .unwrap_or_default();

        info!("Successfully fetched {} peers from backend", peers.len());
        Ok(peers)
    }

    /// Report node status to backend
    pub async fn report_status(
        &self,
        status: &str,
        response_time: Option<i32>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<()> {
        let url = format!("{}/nodes/status", self.base_url);

        debug!("Reporting status to backend: {}", url);

        let request_body = NodeStatusRequest {
            status: status.to_string(),
            response_time,
            metadata,
        };

        let mut request = self.client.put(&url).json(&request_body);

        // Add node token if available
        if let Some(token) = &self.node_token {
            request = request.header("x-node-token", token);
        }

        let response = request
            .send()
            .await
            .context("Failed to send status report to backend")?;

        let status_code = response.status();
        if !status_code.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to report status to backend: status={}, error={}",
                status_code,
                error_text
            );
        }

        let status_response: NodeStatusResponse = response
            .json()
            .await
            .context("Failed to parse status response")?;

        if status_response.code != 200 {
            anyhow::bail!(
                "Backend returned error: code={}, message={}",
                status_response.code,
                status_response.message
            );
        }

        debug!("Successfully reported status to backend");
        Ok(())
    }

    /// Test backend connection
    pub async fn test_connection(&self) -> Result<()> {
        let url = format!("{}/peers", self.base_url);
        debug!("Testing backend connection: {}", url);

        let mut request = self.client.get(&url);

        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

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
            Some("test-token".to_string()),
            Some("test-api-key".to_string()),
        );
        assert!(client.is_ok());
    }
}

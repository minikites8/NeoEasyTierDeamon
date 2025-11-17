use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::backend_client::{BackendClient, BackendPeer};
use crate::config::DistributedConfig;
use crate::db::entity::shared_nodes;
use crate::db::{operations::NodeOperations, Db};
use crate::health_checker::HealthChecker;

/// Distributed probe that fetches peers from backend and reports status
pub struct DistributedProbe {
    config: DistributedConfig,
    backend_client: Arc<BackendClient>,
    health_checker: Arc<HealthChecker>,
    db: Db,
    version: String,
}

impl DistributedProbe {
    /// Create a new distributed probe
    pub fn new(
        config: DistributedConfig,
        health_checker: Arc<HealthChecker>,
        db: Db,
    ) -> Result<Self> {
        let backend_url = config
            .backend_base_url
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Backend base URL is required for distributed mode"))?
            .clone();

        let backend_client = Arc::new(BackendClient::new(
            backend_url,
            config.api_key.clone(),
        )?);

        Ok(Self {
            config,
            backend_client,
            health_checker,
            db,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    /// Start the distributed probe
    pub async fn start(&self) -> Result<()> {
        info!("Starting distributed probe mode");

        // Test backend connection first
        self.backend_client
            .test_connection()
            .await
            .context("Failed to connect to backend")?;

        info!("Backend connection successful");

        // Start peer fetching task
        let peer_fetch_handle = self.start_peer_fetch_task();

        // Start status reporting task
        let status_report_handle = self.start_status_report_task();

        // Wait for tasks (they run indefinitely)
        tokio::select! {
            _ = peer_fetch_handle => {
                error!("Peer fetch task completed unexpectedly");
            }
            _ = status_report_handle => {
                error!("Status report task completed unexpectedly");
            }
        }

        Ok(())
    }

    /// Start periodic peer fetching from backend
    fn start_peer_fetch_task(&self) -> tokio::task::JoinHandle<()> {
        let backend_client = Arc::clone(&self.backend_client);
        let db = self.db.clone();
        let region = self.config.region.clone();
        let interval_secs = self.config.peer_fetch_interval;
        let health_checker = Arc::clone(&self.health_checker);

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            loop {
                ticker.tick().await;

                debug!("Fetching peers from backend...");

                match backend_client.fetch_peers(region.as_deref()).await {
                    Ok(peers) => {
                        info!("Fetched {} peers from backend", peers.len());

                        // Sync peers with local database
                        if let Err(e) = Self::sync_peers_to_db(&db, &health_checker, peers).await {
                            error!("Failed to sync peers to database: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to fetch peers from backend: {}", e);
                    }
                }
            }
        })
    }

    /// Start periodic status reporting to backend
    fn start_status_report_task(&self) -> tokio::task::JoinHandle<()> {
        let backend_client = Arc::clone(&self.backend_client);
        let health_checker = Arc::clone(&self.health_checker);
        let interval_secs = self.config.status_report_interval;
        let version = self.version.clone();
        let region = self.config.region.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            loop {
                ticker.tick().await;

                debug!("Reporting status to backend...");

                // Collect status information
                let all_statuses = health_checker.get_all_nodes_health_status();
                let total_nodes = all_statuses.len();
                let healthy_nodes = all_statuses
                    .iter()
                    .filter(|(_, status, _)| matches!(status, crate::db::HealthStatus::Healthy))
                    .count();

                // Calculate average response time (convert from microseconds to milliseconds)
                let avg_response_time = if !all_statuses.is_empty() {
                    let rtt_values: Vec<i32> = all_statuses
                        .iter()
                        .filter(|(_, status, _)| matches!(status, crate::db::HealthStatus::Healthy))
                        .filter_map(|(node_id, _, _)| {
                            health_checker
                                .get_node_memory_record(*node_id)
                                .and_then(|r| r.get_last_response_time())
                        })
                        .collect();

                    if !rtt_values.is_empty() {
                        // Convert from microseconds to milliseconds
                        let sum_ms: i32 = rtt_values.iter().map(|&rtt_us| rtt_us / 1000).sum();
                        Some(sum_ms / rtt_values.len() as i32)
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Calculate max RTT for metadata
                let max_response_time = if !all_statuses.is_empty() {
                    all_statuses
                        .iter()
                        .filter(|(_, status, _)| matches!(status, crate::db::HealthStatus::Healthy))
                        .filter_map(|(node_id, _, _)| {
                            health_checker
                                .get_node_memory_record(*node_id)
                                .and_then(|r| r.get_last_response_time())
                        })
                        .max()
                        .map(|rtt_us| rtt_us / 1000) // Convert to milliseconds
                } else {
                    None
                };

                let mut metadata = HashMap::new();
                metadata.insert(
                    "version".to_string(),
                    serde_json::Value::String(version.clone()),
                );
                metadata.insert(
                    "peers_count".to_string(),
                    serde_json::Value::Number(total_nodes.into()),
                );
                metadata.insert(
                    "reachable_peers".to_string(),
                    serde_json::Value::Number(healthy_nodes.into()),
                );
                if let Some(r) = &region {
                    metadata.insert("region".to_string(), serde_json::Value::String(r.clone()));
                }
                if let Some(avg) = avg_response_time {
                    metadata.insert(
                        "avg_peer_rtt".to_string(),
                        serde_json::Value::Number(avg.into()),
                    );
                }
                if let Some(max) = max_response_time {
                    metadata.insert(
                        "max_peer_rtt".to_string(),
                        serde_json::Value::Number(max.into()),
                    );
                }

                // Determine overall status
                let status = if total_nodes == 0 {
                    "online" // No nodes to monitor yet, but probe is running
                } else {
                    "online" // Probe is working regardless of peer health
                };

                // Calculate peer count (total healthy peers)
                let peer_count = healthy_nodes as i32;

                // Calculate latency in milliseconds (default to 0 if no data)
                let latency_ms = avg_response_time.unwrap_or(0);

                // Report with ID 0 to represent the probe node itself (not a specific peer)
                // TODO: This may need to be updated based on backend API requirements
                match backend_client
                    .report_status(0, status, latency_ms, peer_count)
                    .await
                {
                    Ok(_) => {
                        debug!("Successfully reported status to backend");
                    }
                    Err(e) => {
                        error!("Failed to report status to backend: {}", e);
                    }
                }
            }
        })
    }

    /// Sync fetched peers to local database and health checker
    async fn sync_peers_to_db(
        db: &Db,
        health_checker: &Arc<HealthChecker>,
        backend_peers: Vec<BackendPeer>,
    ) -> Result<()> {
        // Get current nodes from database
        let current_nodes = NodeOperations::get_all_nodes(db)
            .await
            .context("Failed to get current nodes")?;

        // Create a map keyed by backend node ID for easier lookup
        let mut current_node_map: HashMap<i32, shared_nodes::Model> = HashMap::new();
        for node in current_nodes {
            // Extract backend peer ID from description
            if let Some(id_str) = node.description
                .strip_prefix("Auto-added from backend (ID: ")
                .and_then(|s| s.strip_suffix(")")) {
                if let Ok(backend_id) = id_str.parse::<i32>() {
                    current_node_map.insert(backend_id, node);
                }
            }
        }

        // Add or update peers from backend
        for backend_peer in backend_peers {
            if let Some(existing_node) = current_node_map.get(&backend_peer.id) {
                // Node already exists
                // Check if network_secret or other fields need to be updated
                let backend_secret = backend_peer.network_secret.clone().unwrap_or_else(|| String::new());
                let backend_network_name = backend_peer.network_name.clone().unwrap_or_else(|| String::from("default"));
                
                let needs_update = existing_node.name != backend_peer.name
                    || existing_node.network_name != backend_network_name
                    || existing_node.network_secret != backend_secret;
                
                if needs_update {
                    debug!("Updating existing node: {}", backend_peer.name);
                    if let Ok(Some(node)) = NodeOperations::get_node_by_id(db, existing_node.id).await {
                        let mut active_model = node.into_active_model();
                        active_model.name = Set(backend_peer.name.clone());
                        active_model.network_name = Set(backend_network_name);
                        active_model.network_secret = Set(backend_secret);
                        
                        if let Err(e) = active_model.update(db.orm_db()).await {
                            warn!("Failed to update node: {}", e);
                        } else {
                            // Trigger health checker to reload configuration for this node
                            info!("Node configuration updated for {}, triggering reload", backend_peer.name);
                            if let Err(e) = health_checker.try_update_node(existing_node.id).await {
                                error!("Failed to reload health checker for node {}: {}", existing_node.id, e);
                            }
                        }
                    }
                }
            } else {
                // New node, add to database
                info!("Adding new peer from backend: {}", backend_peer.name);

                // Parse host and port from public_ip
                // Format could be "IP:PORT" or just "IP"
                let (host, port) = if let Some(public_ip) = &backend_peer.public_ip {
                    if let Some((ip, port_str)) = public_ip.split_once(':') {
                        (ip.to_string(), port_str.parse::<i32>().unwrap_or(11010))
                    } else {
                        // No port specified, use default
                        (public_ip.clone(), 11010)
                    }
                } else {
                    // No public_ip provided, skip this peer
                    warn!("Peer {} has no public_ip, skipping", backend_peer.name);
                    continue;
                };

                // Create node request
                let create_req = crate::api::models::CreateNodeRequest {
                    name: backend_peer.name.clone(),
                    host,
                    port,
                    protocol: backend_peer.protocol.clone().unwrap_or_else(|| String::from("tcp")),
                    description: Some(format!("Auto-added from backend (ID: {})", backend_peer.id)),
                    max_connections: 100,
                    allow_relay: backend_peer.allow_relay.unwrap_or(true),
                    network_name: backend_peer.network_name.clone().unwrap_or_else(|| String::from("default")),
                    network_secret: backend_peer.network_secret.clone(),
                    qq_number: None,
                    wechat: None,
                    mail: None,
                };

                match NodeOperations::create_node(db, create_req).await {
                    Ok(node) => {
                        // Auto-approve nodes from backend
                        let mut active_model = node.into_active_model();
                        active_model.is_approved = Set(true);

                        if let Err(e) = active_model.update(db.orm_db()).await {
                            warn!("Failed to approve new node: {}", e);
                        } else {
                            info!("Successfully added and approved node from backend");
                        }
                    }
                    Err(e) => {
                        error!("Failed to create node {}: {}", backend_peer.name, e);
                    }
                }
            }
        }

        // Remove nodes that are no longer in backend (optional)
        // For safety, we don't automatically remove nodes in this implementation
        // You can add this logic if needed based on your requirements

        Ok(())
    }
}

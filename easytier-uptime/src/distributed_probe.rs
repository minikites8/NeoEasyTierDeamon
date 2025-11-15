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
            config.node_token.clone(),
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

                let mut metadata = HashMap::new();
                metadata.insert(
                    "version".to_string(),
                    serde_json::Value::String(version.clone()),
                );
                metadata.insert(
                    "total_monitored_nodes".to_string(),
                    serde_json::Value::Number(total_nodes.into()),
                );
                metadata.insert(
                    "healthy_nodes".to_string(),
                    serde_json::Value::Number(healthy_nodes.into()),
                );
                if let Some(r) = &region {
                    metadata.insert("region".to_string(), serde_json::Value::String(r.clone()));
                }

                // Determine overall status
                let status = if total_nodes == 0 {
                    "Online" // No nodes to monitor yet, but probe is running
                } else {
                    "Online" // Probe is working regardless of peer health
                };

                // Calculate average response time (simple heuristic)
                let avg_response_time = if !all_statuses.is_empty() {
                    let sum: i32 = all_statuses
                        .iter()
                        .filter_map(|(node_id, _, _)| {
                            health_checker
                                .get_node_memory_record(*node_id)
                                .and_then(|r| r.get_last_response_time())
                        })
                        .sum();
                    if sum > 0 {
                        Some(sum / all_statuses.len() as i32)
                    } else {
                        None
                    }
                } else {
                    None
                };

                match backend_client
                    .report_status(status, avg_response_time, Some(metadata))
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

        let current_node_map: HashMap<String, shared_nodes::Model> = current_nodes
            .into_iter()
            .map(|n| (format!("{}:{}", n.host, n.port), n))
            .collect();

        let mut synced_keys = std::collections::HashSet::new();

        // Add or update peers from backend
        for backend_peer in backend_peers {
            let key = format!("{}:{}", backend_peer.host, backend_peer.port);
            synced_keys.insert(key.clone());

            if let Some(existing_node) = current_node_map.get(&key) {
                // Node already exists, check if update needed
                if existing_node.name != backend_peer.name
                    || existing_node.protocol != backend_peer.protocol
                    || existing_node.network_name != backend_peer.network_name
                {
                    debug!("Updating existing node: {}", backend_peer.name);
                    // Update node if needed
                    // Note: For simplicity, we keep the existing node as-is in this implementation
                    // In a production system, you might want to update specific fields
                }
            } else {
                // New node, add to database
                info!("Adding new peer from backend: {}", backend_peer.name);

                // Create node request
                let create_req = crate::api::models::CreateNodeRequest {
                    name: backend_peer.name.clone(),
                    host: backend_peer.host.clone(),
                    port: backend_peer.port,
                    protocol: backend_peer.protocol.clone(),
                    description: Some(format!("Auto-added from backend (ID: {})", backend_peer.id)),
                    max_connections: 100,
                    allow_relay: true,
                    network_name: backend_peer.network_name.clone(),
                    network_secret: Some(String::new()), // Empty for distributed mode
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

//! neo-uptime-node: Independent distributed probe node binary
//!
//! This binary is a standalone probe that:
//! - Fetches peer lists from a backend API
//! - Performs health checks on peers using EasyTier detection logic
//! - Reports probe status and peer latency statistics to backend
//! - Communicates only via HTTP API (no local database dependency)

mod api;
mod backend_client;
mod config;
mod db;
mod health_checker;
mod health_checker_manager;
mod migrator;

use anyhow::{Context, Result};
use clap::Parser;
use config::{AppConfig, DistributedConfig};
use dashmap::DashMap;
use db::Db;
use easytier::utils::init_logger;
use health_checker::HealthChecker;
use health_checker_manager::HealthCheckerManager;
use mimalloc::MiMalloc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use backend_client::{BackendClient, BackendPeer};
use db::entity::shared_nodes;
use db::operations::NodeOperations;
use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};

/// Global mapping of local node ID to backend peer metadata
type PeerMetadataMap = Arc<DashMap<i32, BackendPeer>>;

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[derive(Parser, Debug)]
#[command(
    name = "neo-uptime-node",
    author,
    version,
    about = "Distributed uptime monitoring probe node",
    long_about = "A standalone probe that monitors EasyTier peers and reports status to a central backend"
)]
struct Args {
    /// Backend base URL (e.g., https://backend.example.com)
    #[arg(long, env = "BACKEND_BASE_URL", required = true)]
    backend_base_url: String,

    /// API key for authentication with backend
    #[arg(long, env = "API_KEY", required = true)]
    api_key: String,

    /// Region identifier (optional)
    #[arg(long, env = "REGION")]
    region: Option<String>,

    /// Peer fetch interval in seconds
    #[arg(long, env = "PEER_FETCH_INTERVAL", default_value = "60")]
    peer_fetch_interval: u64,

    /// Status report interval in seconds
    #[arg(long, env = "STATUS_REPORT_INTERVAL", default_value = "30")]
    status_report_interval: u64,

    /// Health check interval in seconds (per peer)
    #[arg(long, env = "HEALTH_CHECK_INTERVAL", default_value = "5")]
    health_check_interval: u64,

    /// Database path for local caching (optional)
    #[arg(long, env = "DATABASE_PATH", default_value = "neo-uptime-node.db")]
    database_path: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logger
    let mut config = AppConfig::default();
    let _ = init_logger(&config.logging, false);

    info!("Starting neo-uptime-node v{}", env!("CARGO_PKG_VERSION"));
    info!("Backend URL: {}", args.backend_base_url);
    info!("Region: {:?}", args.region);
    info!("Peer fetch interval: {}s", args.peer_fetch_interval);
    info!("Status report interval: {}s", args.status_report_interval);

    // Create database connection for local caching
    let db = Db::new(&args.database_path).await?;
    info!("Database initialized at: {}", args.database_path);

    // Create health checker
    let health_checker = Arc::new(HealthChecker::new(db.clone()));

    // Load existing health records from database
    health_checker
        .load_health_records_from_db()
        .await
        .context("Failed to load health records")?;

    // Start health checker manager
    let health_checker_manager = HealthCheckerManager::new(health_checker.clone(), db.clone())
        .with_monitor_interval(Duration::from_secs(args.health_check_interval));

    health_checker_manager
        .start_monitoring()
        .await
        .context("Failed to start health checker manager")?;
    info!("Health checker manager started");

    // Create backend client
    let backend_client = Arc::new(
        BackendClient::new(
            args.backend_base_url.clone(),
            Some(args.api_key.clone()),
        )
        .context("Failed to create backend client")?,
    );

    // Test backend connection
    backend_client
        .test_connection()
        .await
        .context("Failed to connect to backend")?;
    info!("Backend connection successful");

    // Create peer metadata map for tracking backend peer information
    let peer_metadata: PeerMetadataMap = Arc::new(DashMap::new());

    // Build distributed config
    let distributed_config = DistributedConfig {
        enabled: true,
        backend_base_url: Some(args.backend_base_url),
        api_key: Some(args.api_key),
        region: args.region.clone(),
        peer_fetch_interval: args.peer_fetch_interval,
        status_report_interval: args.status_report_interval,
    };

    // Start peer fetch task
    let peer_fetch_handle = start_peer_fetch_task(
        backend_client.clone(),
        db.clone(),
        health_checker.clone(),
        peer_metadata.clone(),
        distributed_config.clone(),
    );

    // Start status report task
    let status_report_handle = start_status_report_task(
        backend_client.clone(),
        db.clone(),
        health_checker.clone(),
        peer_metadata.clone(),
        distributed_config,
    );

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
        _ = peer_fetch_handle => {
            error!("Peer fetch task completed unexpectedly");
        }
        _ = status_report_handle => {
            error!("Status report task completed unexpectedly");
        }
    }

    info!("Shutting down gracefully...");
    Ok(())
}

/// Start periodic peer fetching from backend
fn start_peer_fetch_task(
    backend_client: Arc<BackendClient>,
    db: Db,
    health_checker: Arc<HealthChecker>,
    peer_metadata: PeerMetadataMap,
    config: DistributedConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(config.peer_fetch_interval));
        let mut consecutive_failures = 0;
        let max_failures = 5;

        loop {
            ticker.tick().await;

            debug!("Fetching peers from backend...");

            match backend_client.fetch_peers(config.region.as_deref()).await {
                Ok(peers) => {
                    info!("Fetched {} peers from backend", peers.len());
                    consecutive_failures = 0;

                    // Sync peers with local database
                    if let Err(e) = sync_peers_to_db(&db, &health_checker, &peer_metadata, peers).await {
                        error!("Failed to sync peers to database: {}", e);
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    error!(
                        "Failed to fetch peers from backend (attempt {}/{}): {}",
                        consecutive_failures, max_failures, e
                    );

                    if consecutive_failures >= max_failures {
                        warn!(
                            "Failed to fetch peers {} times consecutively, but continuing...",
                            max_failures
                        );
                        // Reset counter to avoid spam
                        consecutive_failures = 0;
                    }
                }
            }
        }
    })
}

/// Start periodic status reporting to backend (per-peer reporting)
fn start_status_report_task(
    backend_client: Arc<BackendClient>,
    db: Db,
    health_checker: Arc<HealthChecker>,
    peer_metadata: PeerMetadataMap,
    config: DistributedConfig,
) -> tokio::task::JoinHandle<()> {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let region = config.region.clone();

    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(config.status_report_interval));

        loop {
            ticker.tick().await;

            debug!("Collecting and reporting peer statuses...");

            // Get all node statuses
            let all_statuses = health_checker.get_all_nodes_health_status();
            
            debug!("Found {} peers to report", all_statuses.len());

            // Report each peer individually (Mode A)
            for (node_id, health_status, error_info) in all_statuses {
                // Get RTT for this peer (in microseconds from health checker)
                let rtt_us = health_checker
                    .get_node_memory_record(node_id)
                    .and_then(|r| r.get_last_response_time());
                
                // Convert RTT from microseconds to milliseconds
                let response_time_ms = rtt_us.map(|us| us / 1000);

                // Determine status
                let status = match health_status {
                    db::HealthStatus::Healthy => "Online",
                    _ => "Offline",
                };

                // Get node details from database to retrieve backend peer ID
                let node_details = match NodeOperations::get_node_by_id(&db, node_id).await {
                    Ok(Some(node)) => node,
                    Ok(None) => {
                        warn!("Node {} not found in database, skipping report", node_id);
                        continue;
                    }
                    Err(e) => {
                        error!("Failed to get node {} from database: {}", node_id, e);
                        continue;
                    }
                };

                // Extract backend peer ID from description
                // Format: "Auto-added from backend (ID: 123)"
                let backend_peer_id = if let Some(id_str) = node_details.description
                    .strip_prefix("Auto-added from backend (ID: ")
                    .and_then(|s| s.strip_suffix(")")) {
                    match id_str.parse::<i32>() {
                        Ok(id) => id,
                        Err(_) => {
                            warn!("Failed to parse backend peer ID from description: {}", node_details.description);
                            continue;
                        }
                    }
                } else {
                    warn!("Node {} does not have a valid backend peer ID in description: {}", 
                          node_id, node_details.description);
                    continue;
                };

                // Get peer metadata if available
                let peer_meta = peer_metadata.get(&node_id);

                // Build metadata
                let mut metadata = HashMap::new();
                metadata.insert(
                    "peer_name".to_string(),
                    serde_json::Value::String(node_details.name.clone()),
                );
                metadata.insert(
                    "host".to_string(),
                    serde_json::Value::String(node_details.host.clone()),
                );
                metadata.insert(
                    "port".to_string(),
                    serde_json::Value::Number(node_details.port.into()),
                );
                metadata.insert(
                    "protocol".to_string(),
                    serde_json::Value::String(node_details.protocol.clone()),
                );
                
                if let Some(ref peer) = peer_meta {
                    if let Some(ref network_name) = peer.network_name {
                        metadata.insert(
                            "network_name".to_string(),
                            serde_json::Value::String(network_name.clone()),
                        );
                    }
                    if let Some(ref region_val) = peer.region {
                        metadata.insert(
                            "region".to_string(),
                            serde_json::Value::String(region_val.clone()),
                        );
                    }
                    if let Some(ref isp) = peer.isp {
                        metadata.insert(
                            "ISP".to_string(),
                            serde_json::Value::String(isp.clone()),
                        );
                    }
                }

                // Add probe information
                if let Some(ref probe_region) = region {
                    metadata.insert(
                        "probe_region".to_string(),
                        serde_json::Value::String(probe_region.clone()),
                    );
                }
                metadata.insert(
                    "probe_version".to_string(),
                    serde_json::Value::String(version.clone()),
                );

                if let Some(ref err) = error_info {
                    metadata.insert(
                        "error_message".to_string(),
                        serde_json::Value::String(err.clone()),
                    );
                }

                debug!(
                    "Reporting peer {} (backend ID {}): status={}, rtt={:?}ms",
                    node_details.name, backend_peer_id, status, response_time_ms
                );

                // Report to backend with retry logic
                let mut retry_count = 0;
                let max_retries = 3;
                
                loop {
                    match backend_client
                        .report_status(backend_peer_id, status, response_time_ms, Some(metadata.clone()))
                        .await
                    {
                        Ok(_) => {
                            debug!("Successfully reported status for peer {} (backend ID {})", 
                                   node_details.name, backend_peer_id);
                            break;
                        }
                        Err(e) => {
                            retry_count += 1;
                            if e.to_string().contains("401") || e.to_string().contains("Unauthorized") {
                                error!("Authentication failed for peer {}: {}. Skipping retry.", 
                                       node_details.name, e);
                                break;
                            }
                            
                            if retry_count >= max_retries {
                                error!(
                                    "Failed to report status for peer {} after {} attempts: {}",
                                    node_details.name, max_retries, e
                                );
                                break;
                            }
                            
                            warn!(
                                "Failed to report status for peer {} (attempt {}/{}): {}. Retrying...",
                                node_details.name, retry_count, max_retries, e
                            );
                            
                            // Simple backoff
                            tokio::time::sleep(Duration::from_secs(2_u64.pow(retry_count as u32))).await;
                        }
                    }
                }
            }
        }
    })
}

/// Sync fetched peers to local database and health checker
async fn sync_peers_to_db(
    db: &Db,
    health_checker: &Arc<HealthChecker>,
    peer_metadata: &PeerMetadataMap,
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
            // Node already exists - store/update peer metadata
            peer_metadata.insert(existing_node.id, backend_peer.clone());
            
            // Check if network_secret needs to be updated
            let backend_secret = backend_peer.network_secret.clone().unwrap_or_else(|| String::new());
            let needs_update = existing_node.network_secret != backend_secret;
            
            // Update description to include backend ID if needed
            let expected_desc = format!("Auto-added from backend (ID: {})", backend_peer.id);
            let desc_needs_update = existing_node.description != expected_desc;
            
            if needs_update || desc_needs_update {
                debug!("Updating peer {}: network_secret={}, description={}", 
                       backend_peer.name, needs_update, desc_needs_update);
                if let Ok(Some(node)) = NodeOperations::get_node_by_id(db, existing_node.id).await {
                    let mut active_model = node.into_active_model();
                    if needs_update {
                        active_model.network_secret = Set(backend_secret);
                    }
                    if desc_needs_update {
                        active_model.description = Set(expected_desc);
                    }
                    if let Err(e) = active_model.update(db.orm_db()).await {
                        warn!("Failed to update node: {}", e);
                    } else if needs_update {
                        // Trigger health checker to reload configuration for this node
                        info!("Network secret updated for node {}, triggering reload", backend_peer.name);
                        if let Err(e) = health_checker.try_update_node(existing_node.id).await {
                            error!("Failed to reload health checker for node {}: {}", existing_node.id, e);
                        }
                    }
                }
            }
        } else {
            // New node, add to database
            info!("Adding new peer from backend: {}", backend_peer.name);

            // Import the API models module
            use crate::api::models::CreateNodeRequest;

            let create_req = CreateNodeRequest {
                name: backend_peer.name.clone(),
                host: backend_peer.host.clone(),
                port: backend_peer.port,
                protocol: backend_peer.protocol.clone(),
                description: Some(format!(
                    "Auto-added from backend (ID: {})",
                    backend_peer.id
                )),
                max_connections: 100,
                allow_relay: true,
                network_name: backend_peer.network_name.clone().unwrap_or_else(|| String::from("default")),
                network_secret: backend_peer.network_secret.clone(),
                qq_number: None,
                wechat: None,
                mail: None,
            };

            match NodeOperations::create_node(db, create_req).await {
                Ok(node) => {
                    // Auto-approve nodes from backend
                    let mut active_model = node.clone().into_active_model();
                    active_model.is_approved = Set(true);

                    if let Err(e) = active_model.update(db.orm_db()).await {
                        warn!("Failed to approve new node: {}", e);
                    } else {
                        info!("Successfully added and approved peer: {}", backend_peer.name);
                        // Store peer metadata with the new node ID
                        peer_metadata.insert(node.id, backend_peer.clone());
                    }
                }
                Err(e) => {
                    error!("Failed to create node {}: {}", backend_peer.name, e);
                }
            }
        }
    }

    // Note: We don't remove nodes that are no longer in backend for safety
    // This preserves historical data and prevents accidental data loss

    Ok(())
}

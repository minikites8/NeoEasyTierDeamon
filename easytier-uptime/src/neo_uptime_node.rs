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
            None,
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
        distributed_config.clone(),
    );

    // Start status report task
    let status_report_handle = start_status_report_task(
        backend_client.clone(),
        health_checker.clone(),
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
                    if let Err(e) = sync_peers_to_db(&db, &health_checker, peers).await {
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

/// Start periodic status reporting to backend
fn start_status_report_task(
    backend_client: Arc<BackendClient>,
    health_checker: Arc<HealthChecker>,
    config: DistributedConfig,
) -> tokio::task::JoinHandle<()> {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let region = config.region.clone();

    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(config.status_report_interval));
        let mut consecutive_failures = 0;
        let max_failures = 5;

        loop {
            ticker.tick().await;

            debug!("Collecting status information...");

            // Collect status information
            let all_statuses = health_checker.get_all_nodes_health_status();
            let total_peers = all_statuses.len();
            let healthy_peers = all_statuses
                .iter()
                .filter(|(_, status, _)| matches!(status, db::HealthStatus::Healthy))
                .count();

            // Calculate average RTT (in milliseconds)
            let (avg_rtt_ms, max_rtt_ms, reachable_peers) = calculate_rtt_statistics(&health_checker, &all_statuses);

            // Build metadata
            let mut metadata = HashMap::new();
            metadata.insert(
                "version".to_string(),
                serde_json::Value::String(version.clone()),
            );
            metadata.insert(
                "peers_count".to_string(),
                serde_json::Value::Number(total_peers.into()),
            );
            metadata.insert(
                "reachable_peers".to_string(),
                serde_json::Value::Number(reachable_peers.into()),
            );
            if let Some(r) = &region {
                metadata.insert("region".to_string(), serde_json::Value::String(r.clone()));
            }
            if let Some(avg) = avg_rtt_ms {
                metadata.insert(
                    "avg_peer_rtt".to_string(),
                    serde_json::Value::Number(avg.into()),
                );
            }
            if let Some(max) = max_rtt_ms {
                metadata.insert(
                    "max_peer_rtt".to_string(),
                    serde_json::Value::Number(max.into()),
                );
            }

            // Determine overall status
            let status = "Online"; // Probe is always online if running

            debug!(
                "Reporting status: peers={}, reachable={}, avg_rtt={:?}ms",
                total_peers, reachable_peers, avg_rtt_ms
            );

            // Report to backend
            match backend_client
                .report_status(status, avg_rtt_ms, Some(metadata))
                .await
            {
                Ok(_) => {
                    debug!("Successfully reported status to backend");
                    consecutive_failures = 0;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    error!(
                        "Failed to report status to backend (attempt {}/{}): {}",
                        consecutive_failures, max_failures, e
                    );

                    if consecutive_failures >= max_failures {
                        warn!(
                            "Failed to report status {} times consecutively, but continuing...",
                            max_failures
                        );
                        consecutive_failures = 0;
                    }
                }
            }
        }
    })
}

/// Calculate RTT statistics from health checker data
/// Returns (avg_rtt_ms, max_rtt_ms, reachable_peers_count)
fn calculate_rtt_statistics(
    health_checker: &Arc<HealthChecker>,
    all_statuses: &[(i32, db::HealthStatus, Option<String>)],
) -> (Option<i32>, Option<i32>, usize) {
    let rtt_values: Vec<i32> = all_statuses
        .iter()
        .filter(|(_, status, _)| matches!(status, db::HealthStatus::Healthy))
        .filter_map(|(node_id, _, _)| {
            health_checker
                .get_node_memory_record(*node_id)
                .and_then(|r| r.get_last_response_time())
        })
        .collect();

    let reachable_peers = rtt_values.len();

    if rtt_values.is_empty() {
        return (None, None, 0);
    }

    // Note: The RTT values from health_checker are already in microseconds
    // We need to convert them to milliseconds by dividing by 1000
    let rtt_ms_values: Vec<i32> = rtt_values.iter().map(|&rtt_us| rtt_us / 1000).collect();

    let sum: i32 = rtt_ms_values.iter().sum();
    let avg_rtt_ms = sum / rtt_ms_values.len() as i32;
    let max_rtt_ms = *rtt_ms_values.iter().max().unwrap_or(&0);

    (Some(avg_rtt_ms), Some(max_rtt_ms), reachable_peers)
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
            // Node already exists - we keep it as-is
            debug!("Peer already exists: {}", backend_peer.name);
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
                        info!("Successfully added and approved peer: {}", backend_peer.name);
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

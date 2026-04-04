//! neo-uptime-node: Independent distributed probe node binary
//!
//! This binary is a standalone probe that:
//! - Connects to a Socket.IO server to receive the peer list
//! - Performs health checks on peers using EasyTier detection logic
//! - Reports per-peer ping statistics back to the server via Socket.IO

mod backend_client;
mod config;
mod db;
mod health_checker;
mod health_checker_manager;
mod migrator;
mod models;

use anyhow::{Context, Result};
use clap::Parser;
use config::{AppConfig, SocketConnectionConfig};
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

use backend_client::{make_ping_report, BackendPeer, SocketBackendClient};
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
    long_about = "A standalone probe that monitors EasyTier peers via Socket.IO and reports latency to a central server"
)]
struct Args {
    /// Socket.IO server URL (e.g. http://uptime.1tmc.top:8081)
    #[arg(long, env = "SOCKET_URL", required = true)]
    socket_url: String,

    /// Cluster ID for Socket.IO authentication
    #[arg(long, env = "CLUSTER_ID", required = true)]
    cluster_id: String,

    /// Cluster secret for Socket.IO authentication
    #[arg(long, env = "CLUSTER_SECRET", required = true)]
    cluster_secret: String,

    /// Socket.IO endpoint path
    #[arg(long, env = "SOCKET_PATH", default_value = "/api/socket.io")]
    socket_path: String,

    /// Challenge endpoint path for token issuance
    #[arg(long, env = "CHALLENGE_PATH", default_value = "/api/cluster/challenge")]
    challenge_path: String,

    /// Token exchange endpoint path for token issuance
    #[arg(long, env = "TOKEN_PATH", default_value = "/api/cluster/token")]
    token_path: String,

    /// Token TTL hint in seconds (used when response does not include expiresIn)
    #[arg(long, env = "TOKEN_TTL_SECONDS", default_value = "300")]
    token_ttl_seconds: u64,

    /// User-Agent header sent during Socket.IO connection
    #[arg(
        long,
        env = "USER_AGENT",
        default_value = "EasyTierMC.Uptime.Cluster/Node/1.0.0"
    )]
    user_agent: String,

    /// Peer sync interval in seconds (how often to sync the received node list to local DB)
    #[arg(long, env = "PEER_FETCH_INTERVAL", default_value = "60")]
    peer_fetch_interval: u64,

    /// Ping report interval in seconds
    #[arg(long, env = "STATUS_REPORT_INTERVAL", default_value = "30")]
    status_report_interval: u64,

    /// Health check interval in seconds (per peer)
    #[arg(long, env = "HEALTH_CHECK_INTERVAL", default_value = "5")]
    health_check_interval: u64,

    /// Database path for local caching
    #[arg(long, env = "DATABASE_PATH", default_value = "neo-uptime-node.db")]
    database_path: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();

    let config = AppConfig::default();
    let _ = init_logger(&config.logging, false);

    info!("Starting neo-uptime-node v{}", env!("CARGO_PKG_VERSION"));
    info!("Socket.IO server: {}", args.socket_url);
    info!("Cluster ID: {}", args.cluster_id);
    info!("Peer sync interval: {}s", args.peer_fetch_interval);
    info!("Ping report interval: {}s", args.status_report_interval);

    // Local SQLite cache
    let db = Db::new(&args.database_path).await?;
    info!("Database initialized at: {}", args.database_path);

    // Health checker
    let health_checker = Arc::new(HealthChecker::new(db.clone()));
    health_checker
        .load_health_records_from_db()
        .await
        .context("Failed to load health records")?;

    let health_checker_manager = HealthCheckerManager::new(health_checker.clone(), db.clone())
        .with_monitor_interval(Duration::from_secs(args.health_check_interval));
    health_checker_manager
        .start_monitoring()
        .await
        .context("Failed to start health checker manager")?;
    info!("Health checker manager started");

    // Socket.IO client
    let socket_config = SocketConnectionConfig {
        url: args.socket_url.clone(),
        user_agent: args.user_agent.clone(),
        cluster_id: args.cluster_id.clone(),
        cluster_secret: args.cluster_secret.clone(),
        socket_path: args.socket_path.clone(),
        challenge_path: args.challenge_path.clone(),
        token_path: args.token_path.clone(),
        token_ttl_seconds: args.token_ttl_seconds,
    };

    let socket_client = Arc::new(
        SocketBackendClient::new(&socket_config)
            .await
            .context("Failed to connect to Socket.IO server")?,
    );
    info!("Socket.IO connection established");

    // Peer metadata map (local node ID → backend peer)
    let peer_metadata: PeerMetadataMap = Arc::new(DashMap::new());

    // Start peer sync task
    let peer_sync_handle = start_peer_sync_task(
        socket_client.clone(),
        db.clone(),
        health_checker.clone(),
        peer_metadata.clone(),
        args.peer_fetch_interval,
    );

    // Start ping report task
    let ping_report_handle = start_ping_report_task(
        socket_client.clone(),
        db.clone(),
        health_checker.clone(),
        peer_metadata.clone(),
        args.status_report_interval,
    );

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
        _ = peer_sync_handle => {
            error!("Peer sync task completed unexpectedly");
        }
        _ = ping_report_handle => {
            error!("Ping report task completed unexpectedly");
        }
    }

    info!("Shutting down gracefully...");
    Ok(())
}

/// Periodically sync the latest node list (received from Socket.IO) into the local DB.
fn start_peer_sync_task(
    socket_client: Arc<SocketBackendClient>,
    db: Db,
    health_checker: Arc<HealthChecker>,
    peer_metadata: PeerMetadataMap,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        let mut consecutive_failures = 0u32;
        const MAX_FAILURES: u32 = 5;

        loop {
            ticker.tick().await;
            debug!("Syncing peer list from Socket.IO node cache...");

            match socket_client.fetch_peers().await {
                Ok(peers) => {
                    info!("Syncing {} peer(s) to local database", peers.len());
                    consecutive_failures = 0;

                    if let Err(e) =
                        sync_peers_to_db(&db, &health_checker, &peer_metadata, peers).await
                    {
                        error!("Failed to sync peers to database: {}", e);
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    error!(
                        "Failed to read peer list (attempt {}/{}): {}",
                        consecutive_failures, MAX_FAILURES, e
                    );
                    if consecutive_failures >= MAX_FAILURES {
                        warn!(
                            "Failed to read peers {} times consecutively, continuing...",
                            MAX_FAILURES
                        );
                        consecutive_failures = 0;
                    }
                }
            }
        }
    })
}

/// Periodically collect ping measurements and emit them via Socket.IO.
fn start_ping_report_task(
    socket_client: Arc<SocketBackendClient>,
    db: Db,
    health_checker: Arc<HealthChecker>,
    peer_metadata: PeerMetadataMap,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));

        loop {
            ticker.tick().await;
            debug!("Collecting ping measurements for report...");

            let all_statuses = health_checker.get_all_nodes_health_status();
            debug!("Found {} peer(s) to report", all_statuses.len());

            let mut reports = Vec::new();

            for (node_id, health_status, _error_info) in all_statuses {
                // Retrieve the backend nodeId stored in peer_metadata
                let backend_node_id = match peer_metadata.get(&node_id) {
                    Some(peer) => peer.id,
                    None => {
                        // Fall back to description lookup
                        match NodeOperations::get_node_by_id(&db, node_id).await {
                            Ok(Some(node)) => {
                                if let Some(id_str) = node
                                    .description
                                    .strip_prefix("Auto-added from backend (ID: ")
                                    .and_then(|s| s.strip_suffix(")"))
                                {
                                    match id_str.parse::<i32>() {
                                        Ok(id) => id,
                                        Err(_) => {
                                            warn!(
                                                "Cannot parse backend ID from description: {}",
                                                node.description
                                            );
                                            continue;
                                        }
                                    }
                                } else {
                                    warn!(
                                        "Node {} has no valid backend ID in description",
                                        node_id
                                    );
                                    continue;
                                }
                            }
                            Ok(None) => {
                                warn!("Node {} not found in database, skipping", node_id);
                                continue;
                            }
                            Err(e) => {
                                error!("Failed to get node {}: {}", node_id, e);
                                continue;
                            }
                        }
                    }
                };

                // Compute ping in milliseconds (0 when offline)
                let ping_ms = match health_status {
                    db::HealthStatus::Healthy => {
                        health_checker
                            .get_node_memory_record(node_id)
                            .and_then(|r| r.get_last_response_time())
                            .map(|us| us / 1000) // microseconds → milliseconds
                            .unwrap_or(0)
                    }
                    _ => 0,
                };

                reports.push(make_ping_report(backend_node_id, ping_ms));
            }

            if reports.is_empty() {
                debug!("No ping reports to submit");
                continue;
            }

            debug!("Submitting {} ping report(s)", reports.len());
            match socket_client.submit_ping_reports(reports).await {
                Ok(_) => info!("Ping reports submitted successfully"),
                Err(e) => error!("Failed to submit ping reports: {}", e),
            }
        }
    })
}

/// Sync fetched peers to local database and health checker.
async fn sync_peers_to_db(
    db: &Db,
    health_checker: &Arc<HealthChecker>,
    peer_metadata: &PeerMetadataMap,
    backend_peers: Vec<BackendPeer>,
) -> Result<()> {
    let current_nodes = NodeOperations::get_all_nodes(db)
        .await
        .context("Failed to get current nodes")?;

    let mut current_node_map: HashMap<i32, shared_nodes::Model> = HashMap::new();
    for node in current_nodes {
        if let Some(id_str) = node
            .description
            .strip_prefix("Auto-added from backend (ID: ")
            .and_then(|s| s.strip_suffix(")"))
        {
            if let Ok(backend_id) = id_str.parse::<i32>() {
                current_node_map.insert(backend_id, node);
            }
        }
    }

    for backend_peer in backend_peers {
        if let Some(existing_node) = current_node_map.get(&backend_peer.id) {
            peer_metadata.insert(existing_node.id, backend_peer.clone());

            let backend_secret = backend_peer
                .network_secret
                .clone()
                .unwrap_or_else(String::new);
            let needs_update = existing_node.network_secret != backend_secret;

            let expected_desc = format!("Auto-added from backend (ID: {})", backend_peer.id);
            let desc_needs_update = existing_node.description != expected_desc;

            if needs_update || desc_needs_update {
                debug!(
                    "Updating peer {}: network_secret={}, description={}",
                    backend_peer.name, needs_update, desc_needs_update
                );
                if let Ok(Some(node)) =
                    NodeOperations::get_node_by_id(db, existing_node.id).await
                {
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
                        info!(
                            "Network secret updated for node {}, triggering reload",
                            backend_peer.name
                        );
                        if let Err(e) =
                            health_checker.try_update_node(existing_node.id).await
                        {
                            error!(
                                "Failed to reload health checker for node {}: {}",
                                existing_node.id, e
                            );
                        }
                    }
                }
            }
        } else {
            info!("Adding new peer from backend: {}", backend_peer.name);

            let (host, port) = if let Some(public_ip) = &backend_peer.public_ip {
                if let Some((ip, port_str)) = public_ip.split_once(':') {
                    (ip.to_string(), port_str.parse::<i32>().unwrap_or(11010))
                } else {
                    (public_ip.clone(), 11010)
                }
            } else {
                warn!("Peer {} has no public_ip, skipping", backend_peer.name);
                continue;
            };

            use crate::models::CreateNodeRequest;

            let create_req = CreateNodeRequest {
                name: backend_peer.name.clone(),
                host,
                port,
                protocol: backend_peer
                    .protocol
                    .clone()
                    .unwrap_or_else(|| "tcp".to_string()),
                description: Some(format!(
                    "Auto-added from backend (ID: {})",
                    backend_peer.id
                )),
                max_connections: 100,
                allow_relay: backend_peer.allow_relay.unwrap_or(true),
                network_name: backend_peer
                    .network_name
                    .clone()
                    .unwrap_or_else(|| "default".to_string()),
                network_secret: backend_peer.network_secret.clone(),
                qq_number: None,
                wechat: None,
                mail: None,
            };

            match NodeOperations::create_node(db, create_req).await {
                Ok(node) => {
                    let mut active_model = node.clone().into_active_model();
                    active_model.is_approved = Set(true);

                    if let Err(e) = active_model.update(db.orm_db()).await {
                        warn!("Failed to approve new node: {}", e);
                    } else {
                        info!("Successfully added and approved peer: {}", backend_peer.name);
                        peer_metadata.insert(node.id, backend_peer.clone());
                    }
                }
                Err(e) => {
                    error!("Failed to create node {}: {}", backend_peer.name, e);
                }
            }
        }
    }

    Ok(())
}

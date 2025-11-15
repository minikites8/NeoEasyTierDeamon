#![allow(unused)]

mod api;
mod backend_client;
mod config;
mod db;
mod distributed_probe;
mod health_checker;
mod health_checker_manager;
mod migrator;

use api::routes::create_routes;
use clap::Parser;
use config::AppConfig;
use db::{operations::NodeOperations, Db};
use easytier::utils::init_logger;
use health_checker::HealthChecker;
use health_checker_manager::HealthCheckerManager;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

use crate::db::cleanup::{CleanupConfig, CleanupManager};

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Admin password for management access
    #[arg(long, env = "ADMIN_PASSWORD")]
    admin_password: Option<String>,

    /// Enable distributed probe mode
    #[arg(long, env = "DISTRIBUTED_MODE_ENABLED")]
    distributed_mode: bool,

    /// Backend base URL for distributed mode
    #[arg(long, env = "BACKEND_BASE_URL")]
    backend_base_url: Option<String>,

    /// Node token for authentication with backend
    #[arg(long, env = "NODE_TOKEN")]
    node_token: Option<String>,

    /// API key for peer discovery (optional)
    #[arg(long, env = "API_KEY")]
    api_key: Option<String>,

    /// Region identifier (optional)
    #[arg(long, env = "REGION")]
    region: Option<String>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    // 解析命令行参数
    let args = Args::parse();

    // 加载配置
    let mut config = AppConfig::default();

    // Override config with command line arguments
    if args.distributed_mode {
        config.distributed.enabled = true;
    }
    if let Some(url) = args.backend_base_url {
        config.distributed.backend_base_url = Some(url);
    }
    if let Some(token) = args.node_token {
        config.distributed.node_token = Some(token);
    }
    if let Some(key) = args.api_key {
        config.distributed.api_key = Some(key);
    }
    if let Some(region) = args.region {
        config.distributed.region = Some(region);
    }

    // 初始化日志
    let _ = init_logger(&config.logging, false);

    // 如果提供了管理员密码，设置环境变量
    if let Some(password) = args.admin_password {
        env::set_var("ADMIN_PASSWORD", password);
    }

    tracing::info!(
        "Admin password configured: {}",
        !config.security.admin_password.is_empty()
    );

    // Check if running in distributed mode
    if config.distributed.enabled {
        tracing::info!("Starting in DISTRIBUTED PROBE mode");
        return run_distributed_mode(config).await;
    }

    // Standard standalone mode
    tracing::info!("Starting in STANDALONE mode");
    run_standalone_mode(config).await
}

/// Run in distributed probe mode
async fn run_distributed_mode(config: AppConfig) -> anyhow::Result<()> {
    use distributed_probe::DistributedProbe;

    tracing::info!("Distributed mode configuration:");
    tracing::info!("  Backend URL: {:?}", config.distributed.backend_base_url);
    tracing::info!("  Region: {:?}", config.distributed.region);
    tracing::info!(
        "  Peer fetch interval: {}s",
        config.distributed.peer_fetch_interval
    );
    tracing::info!(
        "  Status report interval: {}s",
        config.distributed.status_report_interval
    );

    // Create database connection (still needed for local caching)
    let db = Db::new(&config.database.path.to_string_lossy()).await?;
    tracing::info!("Database initialized successfully!");

    // Create health checker
    let health_checker = Arc::new(HealthChecker::new(db.clone()));

    // Load existing health records
    health_checker.load_health_records_from_db().await?;

    // Create and start health checker manager
    let health_checker_manager = HealthCheckerManager::new(health_checker.clone(), db.clone())
        .with_monitor_interval(Duration::from_secs(1));

    let cleanup_manager = CleanupManager::new(db.clone(), CleanupConfig::default());
    cleanup_manager.start_auto_cleanup().await?;

    health_checker_manager.start_monitoring().await?;
    tracing::info!("Health checker manager started successfully!");

    // Create distributed probe
    let distributed_probe = DistributedProbe::new(
        config.distributed.clone(),
        health_checker.clone(),
        db.clone(),
    )?;

    // Start distributed probe in background
    let probe_handle = tokio::spawn(async move {
        if let Err(e) = distributed_probe.start().await {
            tracing::error!("Distributed probe failed: {}", e);
        }
    });

    // Optionally start API server for monitoring
    let app_state = crate::api::handlers::AppState {
        db: db.clone(),
        health_checker_manager: Arc::new(health_checker_manager),
    };

    let app = create_routes().with_state(app_state);
    let addr = config.server.addr;

    tracing::info!("Starting monitoring API on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let shutdown_signal = Arc::new(tokio::sync::Notify::new());
    let server_shutdown_signal = shutdown_signal.clone();

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown_signal.notified().await;
            })
            .await
            .unwrap();
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received shutdown signal");
        }
        _ = probe_handle => {
            tracing::info!("Probe task completed");
        }
        _ = server_handle => {
            tracing::info!("Server task completed");
        }
    }

    tracing::info!("Shutting down gracefully...");
    shutdown_signal.notify_waiters();

    tracing::info!("Shutdown complete");
    Ok(())
}

/// Run in standalone mode (original behavior)
async fn run_standalone_mode(config: AppConfig) -> anyhow::Result<()> {
    tracing::info!(
        "Admin password configured: {}",
        !config.security.admin_password.is_empty()
    );

    // 创建数据库连接
    let db = Db::new(&config.database.path.to_string_lossy()).await?;

    // 获取数据库统计信息
    let stats = db.get_database_stats().await?;
    tracing::info!("Database initialized successfully!");
    tracing::info!("Database stats: {:?}", stats);

    // 创建配置目录
    let config_dir = PathBuf::from("./configs");
    tokio::fs::create_dir_all(&config_dir).await?;

    // 创建健康检查器和管理器
    let health_checker = Arc::new(HealthChecker::new(db.clone()));
    let health_checker_manager = HealthCheckerManager::new(health_checker, db.clone())
        .with_monitor_interval(Duration::from_secs(1)); // 每30秒检查一次节点变化

    let cleanup_manager = CleanupManager::new(db.clone(), CleanupConfig::default());
    cleanup_manager.start_auto_cleanup().await?;

    // 启动节点监控
    health_checker_manager.start_monitoring().await?;
    tracing::info!("Health checker manager started successfully!");

    let monitored_count = health_checker_manager.get_monitored_node_count().await;
    tracing::info!("Currently monitoring {} nodes", monitored_count);

    // 创建应用状态
    let app_state = crate::api::handlers::AppState {
        db: db.clone(),
        health_checker_manager: Arc::new(health_checker_manager),
    };

    // 创建 API 路由
    let app = create_routes().with_state(app_state);

    // 配置服务器地址
    let addr = config.server.addr;

    tracing::info!("Starting server on http://{}", addr);
    tracing::info!("Available endpoints:");
    tracing::info!("  GET  /health - Health check");
    tracing::info!("  GET  /api/nodes - Get nodes (paginated, approved only)");
    tracing::info!("  POST /api/nodes - Create node (pending approval)");
    tracing::info!("  GET  /api/nodes/:id - Get node by ID");
    tracing::info!("  PUT  /api/nodes/:id - Update node");
    tracing::info!("  DELETE /api/nodes/:id - Delete node");
    tracing::info!("  GET  /api/nodes/:id/health - Get node health history");
    tracing::info!("  GET  /api/nodes/:id/health/stats - Get node health stats");
    tracing::info!("Admin endpoints:");
    tracing::info!("  POST /api/admin/login - Admin login");
    tracing::info!("  GET  /api/admin/nodes - Get all nodes (including pending)");
    tracing::info!("  PUT  /api/admin/nodes/:id/approve - Approve/reject node");
    tracing::info!("  DELETE /api/admin/nodes/:id - Delete node (admin only)");

    // 启动服务器
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // 设置优雅关闭
    let shutdown_signal = Arc::new(tokio::sync::Notify::new());
    let server_shutdown_signal = shutdown_signal.clone();

    // 启动服务器任务
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown_signal.notified().await;
            })
            .await
            .unwrap();
    });

    // 等待 Ctrl+C 信号
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received shutdown signal");
        }
        _ = server_handle => {
            tracing::info!("Server task completed");
        }
    }

    // 优雅关闭
    tracing::info!("Shutting down gracefully...");
    shutdown_signal.notify_waiters();

    tracing::info!("Shutdown complete");
    Ok(())
}

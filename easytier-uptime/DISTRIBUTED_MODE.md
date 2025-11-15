# Distributed Probe Mode

The easytier-uptime system can now be deployed as a distributed probe node that integrates with a backend API for centralized peer discovery and status reporting.

## Two Deployment Options

### Option 1: neo-uptime-node (Recommended)

A standalone, independently compiled binary specifically designed for distributed probing. This is the **recommended** approach for production deployments.

**Advantages:**
- Completely decoupled from the main easytier-uptime service
- Smaller binary size, focused only on probing functionality  
- Easier to deploy and scale independently
- No database or frontend dependencies
- Better isolation and fault tolerance

**Usage:** See the [neo-uptime-node usage guide](./README.md#neo-uptime-node-使用指南) in README.md

### Option 2: easytier-uptime with Distributed Mode

The original easytier-uptime binary can also run in distributed probe mode.

**Usage:** Continue reading this document for configuration details

## Overview

In distributed mode, the probe:
- Fetches peer lists from a central backend API
- Performs health checks using the existing detection logic
- Reports its own status back to the backend
- Can be deployed across multiple regions/locations

## Configuration (easytier-uptime distributed mode)

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DISTRIBUTED_MODE_ENABLED` | No | `false` | Enable distributed probe mode |
| `BACKEND_BASE_URL` | Yes* | - | Backend API base URL (e.g., `http://backend.example.com`) |
| `NODE_TOKEN` | Yes* | - | Authentication token for status reporting |
| `API_KEY` | No | - | Optional API key for peer discovery |
| `REGION` | No | - | Region identifier for filtering peers |
| `PEER_FETCH_INTERVAL` | No | `60` | Interval for fetching peers from backend (seconds) |
| `STATUS_REPORT_INTERVAL` | No | `30` | Interval for reporting status to backend (seconds) |

\* Required when `DISTRIBUTED_MODE_ENABLED=true`

### Command Line Arguments

```bash
easytier-uptime \
  --distributed-mode \
  --backend-base-url "http://backend.example.com" \
  --node-token "your-node-token" \
  --api-key "optional-api-key" \
  --region "us-west"
```

## Backend API Requirements

The backend must implement these endpoints:

### 1. GET /peers - Peer Discovery

Fetch the list of peers to monitor.

**Request:**
```
GET /peers?region=us-west
Authorization: Bearer {apiKey}
```

**Response:**
```json
{
  "code": 200,
  "message": "Peer 节点列表获取成功",
  "data": {
    "peers": [
      {
        "id": 1,
        "name": "节点1",
        "host": "192.168.1.100",
        "port": 25565,
        "protocol": "http",
        "network_name": "main",
        "status": "Online",
        "response_time": 50
      }
    ],
    "total_available": 100,
    "next_batch_available": true
  }
}
```

### 2. PUT /nodes/status - Status Reporting

Report the probe node's own status.

**Request:**
```
PUT /nodes/status
x-node-token: {nodeToken}
Content-Type: application/json

{
  "status": "Online",
  "response_time": 50,
  "metadata": {
    "version": "0.1.0",
    "region": "us-west",
    "peers_count": 10,
    "reachable_peers": 8,
    "avg_peer_rtt": 45,
    "max_peer_rtt": 120
  }
}
```

**Field Descriptions:**
- `status` (required): Probe node status, supports `Online` / `Offline`
- `response_time` (optional): **Average latency from probe to all peers (milliseconds)**
  - Calculation: Average RTT of all successfully probed peers
  - Type: Integer (milliseconds)
  - **Note:** Fixed issue where microseconds were incorrectly reported - now correctly converted to milliseconds
- `metadata` (optional): Additional information
  - `version`: Probe node version
  - `region`: Node region identifier
  - `peers_count`: Number of peers fetched in this round
  - `reachable_peers`: Number of successfully probed peers
  - `avg_peer_rtt`: Average RTT (milliseconds)
  - `max_peer_rtt`: Maximum RTT (milliseconds)

**Response:**
```json
{
  "code": 200,
  "message": "节点状态更新成功",
  "data": null
}
```

## Latency Reporting Fix

**Important:** In previous versions, there was an issue where latency values were reported in microseconds instead of milliseconds. This has been fixed:

- EasyTier internally uses **microseconds (μs)** for latency measurements
- Both distributed modes now automatically convert to **milliseconds (ms)** before reporting
- The `response_time` field is guaranteed to be in integer milliseconds
- Conversion formula: `RTT_ms = RTT_us / 1000`

## Running Examples

### Using Environment Variables

```bash
# Create .env file
cat > .env << EOF
DISTRIBUTED_MODE_ENABLED=true
BACKEND_BASE_URL=http://backend.example.com
NODE_TOKEN=your-secret-token
API_KEY=optional-api-key
REGION=us-west
PEER_FETCH_INTERVAL=60
STATUS_REPORT_INTERVAL=30
EOF

# Run the probe
cargo run --release
```

### Using Command Line

```bash
cargo run --release -- \
  --distributed-mode \
  --backend-base-url "http://backend.example.com" \
  --node-token "your-secret-token" \
  --region "us-west"
```

### Docker Deployment

```bash
docker run -d \
  --name easytier-probe \
  -e DISTRIBUTED_MODE_ENABLED=true \
  -e BACKEND_BASE_URL=http://backend.example.com \
  -e NODE_TOKEN=your-secret-token \
  -e REGION=us-west \
  easytier-uptime:latest
```

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Backend API Server                    │
│  ┌──────────────┐              ┌──────────────────────┐ │
│  │ GET /peers   │              │ PUT /nodes/status    │ │
│  └──────────────┘              └──────────────────────┘ │
└─────────────────────────────────────────────────────────┘
           │                                ▲
           │ Peer List                      │ Status Reports
           ▼                                │
┌─────────────────────────────────────────────────────────┐
│              Distributed Probe (Region A)                │
│  ┌──────────────────────────────────────────────────┐   │
│  │  Existing HealthChecker Logic                    │   │
│  │  - EasyTier connection tests                     │   │
│  │  - Response time measurement                     │   │
│  │  - Status tracking                               │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│              Distributed Probe (Region B)                │
│                     (Same logic)                          │
└─────────────────────────────────────────────────────────┘
```

## Features

- **Backward Compatible**: Can still run in standalone mode without any changes
- **Zero Code Change**: Detection logic remains identical to standalone mode
- **Auto-Discovery**: Automatically discovers and monitors new peers from backend
- **Self-Reporting**: Reports probe health and metrics to backend
- **Multi-Region**: Supports region-based filtering for geo-distributed deployments
- **Resilient**: Continues checking existing peers even if backend is temporarily unavailable

## Monitoring API

Even in distributed mode, the probe exposes a local monitoring API:

```
http://localhost:8080/health          - Health check
http://localhost:8080/api/nodes       - List monitored nodes
http://localhost:8080/api/nodes/{id}/health - Node health history
```

## Troubleshooting

### Backend Connection Issues

Check logs for connection errors:
```bash
tail -f easytier-uptime.log | grep backend
```

Test backend connectivity manually:
```bash
curl -H "Authorization: Bearer YOUR_API_KEY" \
  http://backend.example.com/peers
```

### Peer Sync Issues

Monitor peer synchronization:
```bash
# Check number of nodes being monitored
curl http://localhost:8080/api/nodes | jq '.data.total'
```

### Authentication Errors

Verify your credentials:
- `NODE_TOKEN`: Used for PUT /nodes/status (x-node-token header)
- `API_KEY`: Used for GET /peers (Authorization: Bearer header)

## Migration from Standalone

To migrate from standalone to distributed mode:

1. Keep existing database (maintains historical data)
2. Add distributed configuration
3. Backend will start managing peer list
4. Local database used for caching and historical records

## Security Considerations

- **Token Security**: Store `NODE_TOKEN` and `API_KEY` securely
- **HTTPS**: Use HTTPS for production backend URLs
- **Network Security**: Restrict probe access to backend API only
- **Monitoring**: Regularly check probe status reports in backend

## Performance

- **Peer Fetch**: Default 60s interval (configurable)
- **Status Report**: Default 30s interval (configurable)
- **Health Checks**: Unchanged from standalone mode (5s interval per peer)
- **Database**: SQLite for local caching and history

## Development

Build distributed mode:
```bash
cargo build --release
```

Run tests:
```bash
cargo test
```

Check code:
```bash
cargo check
```

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

### 1. GET /node-status - Get Node IDs

Fetch the list of all node IDs (no authentication required).

**Request:**
```
GET /node-status
```

**Response:**
```json
[
  {
    "node_id": 0,
    "status": "online",
    "latency_ms": 0,
    "peer": 0,
    "last_heartbeat": "2025-11-17T13:01:33.407Z"
  }
]
```

### 2. GET /nodes/{node_id}/private-info - Get Node Connection Details

Fetch private connection information for a specific node (requires authentication).

**Request:**
```
GET /nodes/{node_id}/private-info
Authorization: Bearer {apiKey}
```

**Response:**
```json
{
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
}
```

### 3. POST /nodes/:node_id/heartbeat - Submit Node Status

Report node status information (requires authentication).

**Request:**
```
POST /nodes/{node_id}/heartbeat
Authorization: Bearer {apiKey}
Content-Type: application/json

{
  "status": "online",
  "peer": 0,
  "latency_ms": 0
}
```

**Field Descriptions:**
- `status` (required): Node status, supports `online` / `offline`
- `peer` (required): Number of connected peers (integer)
- `latency_ms` (required): Latency in milliseconds (integer)

**Response:**
```json
{
  "success": true,
  "heartbeat": {
    "id": 0,
    "node_id": 0,
    "status": "online",
    "peer": 0,
    "latency_ms": 0,
    "timestamp": "2025-11-17T12:59:28.437Z"
  },
  "nodeStatus": {
    "node_id": 0,
    "status": "online",
    "latency_ms": 0,
    "peer": 0,
    "last_heartbeat": "2025-11-17T12:59:28.437Z"
  }
}
```

## Authentication Changes

**Important:** The API authentication method has been updated:

- **Old Method:** Used `x-api-key` header for authentication
- **New Method:** Uses `Authorization: Bearer {apiKey}` header
- All authenticated endpoints now use the Bearer token authentication scheme
- The `GET /node-status` endpoint does not require authentication
- The `GET /nodes/{node_id}/private-info` and `POST /nodes/{node_id}/heartbeat` endpoints require Bearer token authentication

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

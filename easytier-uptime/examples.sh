#!/bin/bash

# Example 1: Run in standalone mode (default)
# This is the traditional mode where nodes are managed locally
echo "Example 1: Standalone Mode"
echo "cargo run --release"
echo ""

# Example 2: Run in distributed probe mode with environment variables
echo "Example 2: Distributed Mode with Environment Variables"
cat << 'EOF'
export DISTRIBUTED_MODE_ENABLED=true
export BACKEND_BASE_URL="http://backend.example.com"
export NODE_TOKEN="your-secret-node-token"
export API_KEY="optional-api-key"
export REGION="us-west"
export PEER_FETCH_INTERVAL=60
export STATUS_REPORT_INTERVAL=30

cargo run --release
EOF
echo ""

# Example 3: Run in distributed probe mode with CLI arguments
echo "Example 3: Distributed Mode with CLI Arguments"
cat << 'EOF'
cargo run --release -- \
  --distributed-mode \
  --backend-base-url "http://backend.example.com" \
  --node-token "your-secret-node-token" \
  --api-key "optional-api-key" \
  --region "us-west"
EOF
echo ""

# Example 4: Docker deployment in distributed mode
echo "Example 4: Docker Deployment (Distributed Mode)"
cat << 'EOF'
docker run -d \
  --name easytier-probe-us-west \
  -e DISTRIBUTED_MODE_ENABLED=true \
  -e BACKEND_BASE_URL=http://backend.example.com \
  -e NODE_TOKEN=your-secret-token \
  -e REGION=us-west \
  -p 8080:8080 \
  easytier-uptime:latest
EOF
echo ""

# Example 5: Multiple probes in different regions
echo "Example 5: Multiple Regional Probes"
cat << 'EOF'
# US West probe
docker run -d --name probe-us-west \
  -e DISTRIBUTED_MODE_ENABLED=true \
  -e BACKEND_BASE_URL=http://backend.example.com \
  -e NODE_TOKEN=token-us-west \
  -e REGION=us-west \
  easytier-uptime:latest

# US East probe
docker run -d --name probe-us-east \
  -e DISTRIBUTED_MODE_ENABLED=true \
  -e BACKEND_BASE_URL=http://backend.example.com \
  -e NODE_TOKEN=token-us-east \
  -e REGION=us-east \
  easytier-uptime:latest

# EU probe
docker run -d --name probe-eu \
  -e DISTRIBUTED_MODE_ENABLED=true \
  -e BACKEND_BASE_URL=http://backend.example.com \
  -e NODE_TOKEN=token-eu \
  -e REGION=eu-central \
  easytier-uptime:latest
EOF
echo ""

# Example 6: Systemd service configuration
echo "Example 6: Systemd Service (Distributed Mode)"
cat << 'EOF'
# /etc/systemd/system/easytier-probe.service
[Unit]
Description=EasyTier Uptime Probe
After=network.target

[Service]
Type=simple
User=easytier
WorkingDirectory=/opt/easytier-uptime
Environment="DISTRIBUTED_MODE_ENABLED=true"
Environment="BACKEND_BASE_URL=http://backend.example.com"
Environment="NODE_TOKEN=your-secret-token"
Environment="REGION=us-west"
ExecStart=/opt/easytier-uptime/easytier-uptime
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target

# Enable and start:
# sudo systemctl enable easytier-probe
# sudo systemctl start easytier-probe
# sudo systemctl status easytier-probe
EOF

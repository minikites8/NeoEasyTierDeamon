#!/bin/bash
# Example script for running neo-uptime-node

# This is a template - replace these values with your actual configuration
BACKEND_BASE_URL="https://backend.example.com"
API_KEY="your-api-key"        # Required for authentication
REGION="cn-hz"                 # Optional

# Run neo-uptime-node
BACKEND_BASE_URL="$BACKEND_BASE_URL" \
API_KEY="$API_KEY" \
REGION="$REGION" \
PEER_FETCH_INTERVAL=60 \
STATUS_REPORT_INTERVAL=30 \
HEALTH_CHECK_INTERVAL=5 \
./target/release/neo-uptime-node

# neo-uptime-node

独立的分布式 EasyTier 探测节点程序，用于监控 EasyTier 节点并向后端报告状态。

## 特性

- ✨ **完全独立**：作为单独的 crate，与 easytier-uptime 完全解耦
- 🚀 **轻量级**：只包含探测功能，资源占用更少
- 📦 **易于部署**：通过环境变量配置，适合容器化和云原生部署
- 🔒 **良好隔离**：探测节点故障不会影响后端主服务
- 📊 **精确测量**：自动计算和上报延迟统计（RTT，单位毫秒）
- 🌍 **分布式友好**：支持多地域部署，按需扩展

## 快速开始

### 构建

```bash
# 从工作区根目录构建
cargo build -p neo-uptime-node --release

# 编译后的二进制位于
./target/release/neo-uptime-node
```

### 运行

```bash
# 使用命令行参数
./target/release/neo-uptime-node \
  --backend-base-url "https://backend.example.com" \
  --api-key "your-api-key" \
  --region "cn-hz"

# 或使用环境变量
export BACKEND_BASE_URL="https://backend.example.com"
export API_KEY="your-api-key"
export REGION="cn-hz"
./target/release/neo-uptime-node
```

## 配置

### 必需配置

| 环境变量 | 命令行参数 | 说明 |
|---------|-----------|------|
| `BACKEND_BASE_URL` | `--backend-base-url` | 后端 API 基础地址 |
| `API_KEY` | `--api-key` | API Key（用于请求认证） |

### 可选配置

| 环境变量 | 命令行参数 | 默认值 | 说明 |
|---------|-----------|--------|------|
| `REGION` | `--region` | 无 | 区域标识符 |
| `PEER_FETCH_INTERVAL` | `--peer-fetch-interval` | `60` | 获取 peer 列表的间隔（秒） |
| `STATUS_REPORT_INTERVAL` | `--status-report-interval` | `30` | 上报 peer 状态的间隔（秒） |
| `HEALTH_CHECK_INTERVAL` | `--health-check-interval` | `5` | 健康检查间隔（秒） |
| `DATABASE_PATH` | `--database-path` | `neo-uptime-node.db` | 本地缓存数据库路径 |

## Docker 部署

### Dockerfile 示例

```dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build -p neo-uptime-node --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/neo-uptime-node /usr/local/bin/
ENTRYPOINT ["neo-uptime-node"]
```

### 运行容器

```bash
docker build -t neo-uptime-node:latest .

docker run -d \
  --name neo-uptime-node \
  --restart unless-stopped \
  -e BACKEND_BASE_URL="https://backend.example.com" \
  -e API_KEY="your-api-key" \
  -e REGION="cn-hz" \
  neo-uptime-node:latest
```

## 工作原理

1. **启动阶段**
   - 初始化本地 SQLite 数据库（用于缓存）
   - 测试与后端的连接
   - 启动健康检查管理器

2. **运行循环**
   - **Peer 获取**（默认每 60 秒）：从后端获取需要监控的节点列表
   - **健康检查**（每个 peer 默认每 5 秒）：使用 EasyTier 原生探测逻辑测量 RTT
   - **状态上报**（默认每 30 秒）：逐个上报每个 peer 的健康状态和延迟

3. **延迟计算**
   - 自动将 EasyTier 内部的微秒（μs）延迟转换为毫秒（ms）
   - 每个 peer 独立计算和上报 RTT

## 后端 API 要求

neo-uptime-node 需要后端实现以下 API 端点：

### GET /peers - 获取节点列表

请求：
```
GET /peers?region=cn-hz
x-api-key: {API_KEY}
```

响应：
```json
{
  "code": 200,
  "message": "Success",
  "data": {
    "peers": [
      {
        "id": 1,
        "name": "Node 1",
        "host": "192.168.1.1",
        "port": 11010,
        "protocol": "tcp",
        "network_name": "default",
        "network_secret": null,
        "public_ip": "192.168.1.1:11010"
      }
    ]
  }
}
```

### PUT /nodes/status - 上报节点状态

请求：
```
PUT /nodes/status
x-api-key: {API_KEY}
Content-Type: application/json

{
  "id": 1,
  "status": "online",
  "response_time": 25,
  "peer_count": 3
}
```

响应：
```json
{
  "code": 200,
  "message": "Success"
}
```

## 日志和调试

使用 `RUST_LOG` 环境变量控制日志级别：

```bash
# 详细日志
RUST_LOG=debug ./target/release/neo-uptime-node ...

# 只显示错误
RUST_LOG=error ./target/release/neo-uptime-node ...

# 针对特定模块
RUST_LOG=neo_uptime_node=debug,backend_client=trace ./target/release/neo-uptime-node ...
```

## 架构说明

neo-uptime-node 与 easytier-uptime 的分离：

```
之前的架构：
easytier-uptime
├── src/
│   ├── main.rs                    (standalone mode)
│   ├── distributed_probe.rs       (distributed mode)
│   └── neo_uptime_node.rs         (standalone distributed binary)

现在的架构：
easytier-uptime/                   (只有 standalone mode)
└── src/
    └── main.rs

neo-uptime-node/                   (完全独立的 crate)
└── src/
    ├── main.rs
    ├── backend_client.rs
    ├── health_checker.rs
    └── ...
```

## 许可证

MIT License

## 支持

如有问题或建议，请提交 Issue 或联系开发团队。

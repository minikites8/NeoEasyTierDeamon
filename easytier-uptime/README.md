# EasyTier Uptime Monitor

一个用于监控 EasyTier 实例健康状态和运行时间的系统。支持独立部署和分布式探测节点两种模式。

## 功能特性

- 🏥 **健康监控**: 实时监控 EasyTier 节点的健康状态
- 📊 **数据统计**: 提供详细的运行时间和响应时间统计
- 🔧 **实例管理**: 管理多个 EasyTier 实例
- 🌐 **Web界面**: 直观的 Web 管理界面
- 🚨 **告警系统**: 支持健康状态异常告警
- 📈 **图表展示**: 可视化展示监控数据
- 🌍 **分布式探测**: 支持分布式部署多个探测节点
- 🚀 **独立探测节点**: `neo-uptime-node` - 独立编译的分布式探测二进制（新功能）

## 部署模式

### 独立模式（Standalone Mode）
传统的独立部署模式，适用于单个监控点场景。

### 分布式探测模式（Distributed Probe Mode）
将 easytier-uptime 作为探测节点分布式部署，通过后端 API 统一管理：
- 从后端 API 获取需要检测的节点列表
- 将检测结果上报给后端
- 支持多地域部署
- 详细文档请参考：[DISTRIBUTED_MODE.md](./DISTRIBUTED_MODE.md)

### neo-uptime-node 独立探测节点（推荐）
`neo-uptime-node` 是一个独立编译的二进制程序，专门用于分布式探测：
- 完全独立于主程序，可单独部署
- 只通过 HTTP API 与后端通信
- 不依赖本地数据库或主守护进程
- 自动计算和上报延迟统计（RTT）
- 支持环境变量配置，易于容器化部署
- 详细使用说明请参考下方 [neo-uptime-node 使用指南](#neo-uptime-node-使用指南) 章节

## 系统架构

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Frontend      │    │   Backend       │    │   Database      │
│   (Vue.js)      │◄──►│   (Rust/Axum)   │◄──►│   (SQLite)      │
│                 │    │                 │    │                 │
│ ┌─────────────┐ │    │ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │ Dashboard   │ │    │ │ API Routes  │ │    │ │ Nodes       │ │
│ │ Health View │ │    │ │ Health      │ │    │ │ Health      │ │
│ │ Node Mgmt   │ │    │ │ Instances   │ │    │ │ Instances   │ │
│ │ Charts      │ │    │ │ Scheduler   │ │    │ │ Stats       │ │
│ └─────────────┘ │    │ └─────────────┘ │    │ └─────────────┘ │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## 快速开始

### 环境要求

- **Rust**: 1.70+
- **Node.js**: 16+
- **npm**: 8+

### 开发环境

1. **克隆项目**
   ```bash
   git clone <repository-url>
   cd easytier-uptime
   ```

2. **启动开发环境**
   ```bash
   ./start-dev.sh
   ```

3. **访问应用**
   - 前端界面: http://localhost:3000
   - 后端API: http://localhost:8080
   - 健康检查: http://localhost:8080/health

### 生产环境

1. **启动生产环境**
   ```bash
   ./start-prod.sh
   ```

2. **停止生产环境**
   ```bash
   ./stop-prod.sh
   ```

## 配置说明

### 环境变量

#### 后端配置 (.env)

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `SERVER_HOST` | `127.0.0.1` | 服务器监听地址 |
| `SERVER_PORT` | `8080` | 服务器端口 |
| `DATABASE_PATH` | `uptime.db` | 数据库文件路径 |
| `DATABASE_MAX_CONNECTIONS` | `10` | 数据库最大连接数 |
| `HEALTH_CHECK_INTERVAL` | `30` | 健康检查间隔(秒) |
| `HEALTH_CHECK_TIMEOUT` | `10` | 健康检查超时(秒) |
| `HEALTH_CHECK_RETRIES` | `3` | 健康检查重试次数 |
| `RUST_LOG` | `info` | 日志级别 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:3000` | 允许的跨域来源 |
| `ENABLE_CORS` | `true` | 是否启用CORS |
| `ENABLE_COMPRESSION` | `true` | 是否启用压缩 |

#### 前端配置 (frontend/.env)

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `VITE_APP_TITLE` | `EasyTier Uptime Monitor` | 应用标题 |
| `VITE_API_BASE_URL` | `/api` | API基础URL |
| `VITE_APP_ENV` | `development` | 应用环境 |
| `VITE_ENABLE_DEV_TOOLS` | `true` | 是否启用开发工具 |
| `VITE_API_TIMEOUT` | `10000` | API超时时间(毫秒) |

## API 文档

### 健康检查

```http
GET /health
```

### 节点管理

```http
# 获取节点列表
GET /api/nodes

# 创建节点
POST /api/nodes

# 获取节点详情
GET /api/nodes/{id}

# 更新节点
PUT /api/nodes/{id}

# 删除节点
DELETE /api/nodes/{id}
```

### 健康记录

```http
# 获取节点健康历史
GET /api/nodes/{id}/health

# 获取节点健康统计
GET /api/nodes/{id}/health/stats
```

### 实例管理

```http
# 获取实例列表
GET /api/instances

# 创建实例
POST /api/instances

# 停止实例
DELETE /api/instances/{id}
```

## 测试

### 运行集成测试

```bash
./test-integration.sh
```

### 运行单元测试

```bash
cargo test
```

### 测试覆盖率

```bash
cargo tarpaulin
```

## 部署

### Docker 部署

```bash
# 构建镜像
docker build -t easytier-uptime .

# 运行容器
docker run -d -p 8080:8080 easytier-uptime
```

### 手动部署

1. **构建后端**
   ```bash
   cargo build --release
   ```

2. **构建前端**
   ```bash
   cd frontend
   npm install
   npm run build
   cd ..
   ```

3. **配置环境**
   ```bash
   cp .env.production .env
   # 编辑 .env 文件
   ```

4. **启动服务**
   ```bash
   ./start-prod.sh
   ```

## 监控和日志

### 日志文件

- **后端日志**: `logs/backend.log`
- **前端日志**: `logs/frontend.log`
- **测试日志**: `test-results/`

### 健康检查

系统提供以下健康检查端点：

- `/health` - 基本健康检查
- `/api/health/stats` - 健康统计信息
- `/api/health/scheduler/status` - 调度器状态

## 故障排除

### 常见问题

1. **后端启动失败**
   - 检查端口是否被占用
   - 确认数据库文件权限
   - 查看日志文件 `logs/backend.log`

2. **前端连接失败**
   - 检查后端服务是否运行
   - 确认API地址配置
   - 检查CORS配置

3. **健康检查失败**
   - 确认目标节点可访问
   - 检查防火墙设置
   - 验证健康检查配置

### 性能优化

1. **数据库优化**
   - 定期清理过期数据
   - 配置适当的连接池大小
   - 使用索引优化查询

2. **前端优化**
   - 启用代码分割
   - 配置缓存策略
   - 优化图片和资源

3. **网络优化**
   - 启用压缩
   - 配置CDN
   - 优化API响应时间

## neo-uptime-node 使用指南

### 简介

`neo-uptime-node` 是一个独立的分布式探测节点程序，专门用于监控 EasyTier 节点并向后端报告状态。相比集成在 `easytier-uptime` 中的分布式模式，它具有以下优势：

- **完全解耦**：独立编译的二进制，可以单独部署和更新
- **轻量级**：只包含探测功能，资源占用更少
- **易于部署**：通过环境变量配置，适合容器化和云原生部署
- **更好的隔离**：探测节点故障不会影响后端主服务

### 构建

```bash
# 从源码构建
cd easytier-uptime
cargo build --bin neo-uptime-node --release

# 编译后的二进制位于
./target/release/neo-uptime-node
```

### 配置

`neo-uptime-node` 支持通过环境变量或命令行参数进行配置：

#### 必需配置

| 环境变量 | 命令行参数 | 说明 | 示例 |
|---------|-----------|------|------|
| `BACKEND_BASE_URL` | `--backend-base-url` | 后端 API 基础地址 | `https://backend.example.com` |
| `API_KEY` | `--api-key` | API Key（用于所有请求的 `x-api-key` 请求头） | `your-api-key` |

#### 可选配置

| 环境变量 | 命令行参数 | 默认值 | 说明 |
|---------|-----------|--------|------|
| `REGION` | `--region` | 无 | 区域标识符，用于标识探测节点所在区域 |
| `PEER_FETCH_INTERVAL` | `--peer-fetch-interval` | `60` | 获取 peer 列表的间隔（秒） |
| `STATUS_REPORT_INTERVAL` | `--status-report-interval` | `30` | 上报每个 peer 状态的间隔（秒） |
| `HEALTH_CHECK_INTERVAL` | `--health-check-interval` | `5` | 每个 peer 的健康检查间隔（秒） |
| `DATABASE_PATH` | `--database-path` | `neo-uptime-node.db` | 本地缓存数据库路径 |

### 运行示例

#### 使用环境变量

```bash
# 设置环境变量
export BACKEND_BASE_URL="https://backend.example.com"
export API_KEY="your-api-key"
export REGION="cn-hz"

# 运行
./target/release/neo-uptime-node
```

#### 使用命令行参数

```bash
./target/release/neo-uptime-node \
  --backend-base-url "https://backend.example.com" \
  --api-key "your-api-key" \
  --region "cn-hz" \
  --peer-fetch-interval 60 \
  --status-report-interval 30
```

#### 使用 .env 文件

```bash
# 创建 .env 文件
cat > .env << EOF
BACKEND_BASE_URL=https://backend.example.com
API_KEY=your-api-key
REGION=cn-hz
PEER_FETCH_INTERVAL=60
STATUS_REPORT_INTERVAL=30
EOF

# 使用 env 文件运行
source .env && ./target/release/neo-uptime-node
```

#### Docker 部署

```dockerfile
# Dockerfile 示例
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build --bin neo-uptime-node --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/neo-uptime-node /usr/local/bin/
ENTRYPOINT ["neo-uptime-node"]
```

```bash
# 构建镜像
docker build -t neo-uptime-node:latest .

# 运行容器
docker run -d \
  --name neo-uptime-node \
  --restart unless-stopped \
  -e BACKEND_BASE_URL="https://backend.example.com" \
  -e API_KEY="your-api-key" \
  -e REGION="cn-hz" \
  neo-uptime-node:latest
```

### 后端 API 要求

`neo-uptime-node` 需要后端实现以下 API 端点：

#### 1. GET /peers - 获取节点列表

用于获取需要监控的 peer 节点列表。

**请求示例：**
```
GET /peers?region=cn-hz
x-api-key: {API_KEY}
```

**响应示例：**
```json
{
  "code": 200,
  "message": "Peer 节点列表获取成功",
  "data": {
    "peers": [
      {
        "id": 2,
        "name": "节点1",
        "host": "221.7.223.136",
        "port": 11010,
        "protocol": "tcp",
        "network_name": null,
        "status": "Online",
        "response_time": 100,
        "region": "China",
        "ISP": "CHINA UNICOM China169 Backbone"
      }
    ],
    "total_available": 1,
    "next_batch_available": false
  }
}
```

#### 2. PUT /nodes/status - 上报远程节点状态

用于探测节点向后端报告**每个远程 easytier 节点**的探测结果（按 peer 维度逐个上报）。

**请求示例：**
```
PUT /nodes/status
x-api-key: {API_KEY}
Content-Type: application/json

{
  "id": 2,
  "status": "Online",
  "response_time": 37,
  "metadata": {
    "peer_name": "节点1",
    "host": "221.7.223.136",
    "port": 11010,
    "protocol": "tcp",
    "network_name": null,
    "region": "China",
    "ISP": "CHINA UNICOM China169 Backbone",
    "probe_region": "cn-hz",
    "probe_version": "0.1.0"
  }
}
```

**字段说明：**
- `id`（必填）：远程 easytier 节点的 ID（整数，来自 `/peers` 返回的 `peers[*].id`）
- `status`（必填）：该远程节点的探测结果，支持 `Online` / `Offline`
- `response_time`（可选）：**本探测节点到该远程节点的 RTT 延迟（毫秒）**
  - 类型：整数（毫秒）
  - 仅当探测成功时提供
- `metadata`（可选）：额外信息
  - `peer_name`: 远程节点名称
  - `host`: 远程节点主机地址
  - `port`: 远程节点端口
  - `protocol`: 远程节点协议
  - `network_name`: 远程节点网络名（可选）
  - `region`: 远程节点所在区域（可选）
  - `ISP`: 远程节点 ISP 信息（可选）
  - `probe_region`: 探测节点所在区域
  - `probe_version`: 探测节点版本号
  - `error_message`: 探测失败时的错误信息（可选）

**响应示例：**
```json
{
  "code": 200,
  "message": "节点状态更新成功",
  "data": null
}
```

### 工作原理

1. **启动阶段**
   - 连接本地 SQLite 数据库用于缓存
   - 测试与后端的连接
   - 启动健康检查管理器

2. **运行循环**
   - **Peer 获取任务**（默认每 60 秒）：
     - 调用 `GET /peers` 获取 peer 列表
     - 将新的 peers 同步到本地数据库
     - 存储 backend peer ID 到 description 字段以便后续查找
     - 自动批准并开始监控新 peers
   
   - **状态上报任务**（默认每 30 秒）：
     - 遍历所有被监控的 peers
     - 对每个 peer，获取其最新探测结果和 RTT
     - 逐个调用 `PUT /nodes/status` 上报每个 peer 的状态（Mode A）
     - 包含 peer 的完整信息和探测结果

3. **健康检查**（每个 peer 默认每 5 秒）：
   - 使用 EasyTier 原生探测逻辑
   - 测量实际的网络延迟（RTT）
   - 记录到本地数据库和内存

### 延迟计算说明

**重要：已修复延迟单位问题**

在之前的版本中，延迟值可能以微秒为单位被错误上报。在新版本中：

- EasyTier 内部使用 **微秒（μs）** 作为延迟单位
- `neo-uptime-node` 自动将其转换为 **毫秒（ms）** 再上报
- `response_time` 字段保证为整数毫秒值
- 转换公式：`RTT_ms = RTT_us / 1000`
- 每个 peer 的 RTT 独立计算和上报

### 多节点部署

`neo-uptime-node` 设计用于多节点分布式部署：

```
┌──────────────────────────────────────┐
│         Backend API Server           │
│   (统一管理所有探测节点)              │
└──────────────────────────────────────┘
         ▲         ▲         ▲
         │         │         │
    ┌────┴───┐ ┌──┴────┐ ┌──┴────┐
    │ Node 1 │ │ Node 2│ │ Node 3│
    │ (北京) │ │ (上海)│ │ (广州)│
    └────────┘ └───────┘ └───────┘
```

每个节点：
- 独立运行，互不影响
- 使用相同的 `BACKEND_BASE_URL` 和 `NODE_TOKEN`
- 可设置不同的 `REGION` 标识符
- 各自上报探测结果

### 日志和调试

`neo-uptime-node` 使用 `tracing` 框架进行日志记录。可以通过 `RUST_LOG` 环境变量控制日志级别：

```bash
# 显示详细日志
RUST_LOG=debug ./target/release/neo-uptime-node ...

# 只显示错误
RUST_LOG=error ./target/release/neo-uptime-node ...

# 针对特定模块
RUST_LOG=neo_uptime_node=debug,backend_client=trace ./target/release/neo-uptime-node ...
```

### 故障排除

#### 1. 无法连接到后端

```
Error: Failed to connect to backend
```

**解决方法：**
- 检查 `BACKEND_BASE_URL` 是否正确
- 确认后端服务正在运行
- 检查网络连接和防火墙设置
- 验证 SSL 证书（如使用 HTTPS）

#### 2. 认证失败

```
Error: Failed to report status to backend: status=401
```

**解决方法：**
- 检查 `NODE_TOKEN` 是否正确
- 确认 token 未过期
- 检查后端是否正确配置了 token 验证

#### 3. 无法获取 peer 列表

```
Error: Failed to fetch peers from backend
```

**解决方法：**
- 检查 `API_KEY` 是否正确配置（如果后端要求）
- 确认后端 `/peers` 端点正常工作
- 检查 `REGION` 参数是否有效

#### 4. 延迟数据不准确

- 确保使用最新版本（已修复微秒/毫秒转换问题）
- 检查网络状况，高延迟可能是实际网络问题
- 查看日志中的 `avg_peer_rtt` 和 `max_peer_rtt` 值

### 性能优化

- **减少探测间隔**：降低 `HEALTH_CHECK_INTERVAL` 可以更频繁地检测，但会增加网络负载
- **增加上报间隔**：提高 `STATUS_REPORT_INTERVAL` 可以减少后端压力
- **并发探测**：程序自动并发探测多个 peers，无需手动配置
- **本地缓存**：使用 SQLite 缓存数据，减少对后端的依赖

## 贡献指南

1. Fork 项目
2. 创建特性分支
3. 提交更改
4. 推送到分支
5. 创建 Pull Request

## 许可证

MIT License

## 支持

如有问题或建议，请提交 Issue 或联系开发团队。
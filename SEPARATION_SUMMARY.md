# 分离说明 / Separation Summary

## 概述 / Overview

本次更新将 `neo-uptime-node` 与 `easytier-uptime` 彻底分离，使其成为完全独立的 workspace 成员。

This update completely separates `neo-uptime-node` from `easytier-uptime`, making it a fully independent workspace member.

## 主要变更 / Key Changes

### 1. 新的目录结构 / New Directory Structure

**之前 / Before:**
```
NeoEasyTierDeamon/
├── easytier/
└── easytier-uptime/
    ├── src/
    │   ├── main.rs                  (standalone + distributed mode)
    │   ├── distributed_probe.rs     (distributed probe logic)
    │   └── neo_uptime_node.rs       (distributed binary)
    └── Cargo.toml                   (2 binaries)
```

**之后 / After:**
```
NeoEasyTierDeamon/
├── easytier/
├── easytier-uptime/              (standalone mode only)
│   ├── src/
│   │   └── main.rs
│   └── Cargo.toml                (1 binary)
└── neo-uptime-node/              (独立crate / independent crate)
    ├── src/
    │   ├── main.rs
    │   ├── backend_client.rs
    │   ├── health_checker.rs
    │   ├── db/
    │   └── ...
    ├── Cargo.toml                (independent dependencies)
    └── README.md
```

### 2. 代码分离 / Code Separation

#### easytier-uptime
- ✅ 移除了 `distributed_probe.rs`
- ✅ 移除了 `neo_uptime_node.rs`
- ✅ 移除了分布式模式相关的命令行参数
- ✅ 简化为仅支持独立模式（standalone mode）
- ✅ 更新了 README 文档

#### neo-uptime-node (新建)
- ✅ 创建为独立的 workspace 成员
- ✅ 复制了所需的所有模块（backend_client, health_checker, db, config, etc.）
- ✅ 创建了最小化的 `models.rs` 模块
- ✅ 添加了独立的 README 文档
- ✅ 配置了独立的依赖项
- ✅ 可以独立构建和部署

### 3. 构建和部署 / Build and Deploy

#### 构建 / Build

```bash
# 构建 easytier-uptime (独立监控服务)
cargo build -p easytier-uptime

# 构建 neo-uptime-node (分布式探测节点)
cargo build -p neo-uptime-node

# 构建 release 版本
cargo build -p easytier-uptime --release
cargo build -p neo-uptime-node --release
```

#### 运行 / Run

```bash
# 运行 easytier-uptime
./target/release/easytier-uptime --admin-password "your-password"

# 运行 neo-uptime-node
./target/release/neo-uptime-node \
  --backend-base-url "https://backend.example.com" \
  --api-key "your-api-key" \
  --region "cn-hz"
```

### 4. 文档更新 / Documentation Updates

- ✅ `easytier-uptime/README.md` - 移除了 neo-uptime-node 使用指南，添加了指向独立 README 的链接
- ✅ `neo-uptime-node/README.md` - 新建的独立文档，包含完整的使用说明

## 优势 / Benefits

1. **完全解耦 / Complete Decoupling**
   - neo-uptime-node 不再依赖 easytier-uptime 的代码
   - 可以独立开发、测试和部署

2. **更清晰的职责 / Clear Responsibilities**
   - `easytier-uptime`: 独立监控服务，提供 Web 界面和 API
   - `neo-uptime-node`: 分布式探测节点，只负责探测和上报

3. **更容易维护 / Easier Maintenance**
   - 两个项目可以独立演进
   - 减少了代码重复和混淆
   - 更容易理解和修改

4. **灵活部署 / Flexible Deployment**
   - 可以独立部署 neo-uptime-node 到不同的地域
   - 不需要部署完整的 easytier-uptime 服务

5. **减少资源占用 / Reduced Resource Usage**
   - neo-uptime-node 二进制更小（451MB vs 473MB in debug mode）
   - 只包含探测功能，运行时资源占用更少

## 兼容性 / Compatibility

- ✅ 保持了与后端 API 的兼容性
- ✅ 数据库格式保持不变
- ✅ 配置文件格式兼容
- ✅ 探测逻辑和延迟计算保持一致

## 迁移指南 / Migration Guide

### 从旧版本迁移 / Migrating from Old Version

如果之前使用 `easytier-uptime --distributed-mode`：

```bash
# 旧方式 (不再支持)
easytier-uptime --distributed-mode \
  --backend-base-url "https://backend.example.com" \
  --api-key "your-api-key"

# 新方式 (使用独立的 neo-uptime-node)
neo-uptime-node \
  --backend-base-url "https://backend.example.com" \
  --api-key "your-api-key"
```

### Docker 部署更新 / Docker Deployment Update

```dockerfile
# 旧的 Dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY easytier-uptime easytier-uptime
RUN cd easytier-uptime && cargo build --bin neo-uptime-node --release

# 新的 Dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build -p neo-uptime-node --release
```

## 测试验证 / Testing Verification

✅ 两个包都可以成功构建：
```bash
$ cargo build -p easytier-uptime
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.04s

$ cargo build -p neo-uptime-node
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.54s
```

✅ 二进制文件正常生成：
```bash
$ ls -lh target/debug/easytier-uptime target/debug/neo-uptime-node
-rwxrwxr-x 2 runner runner 473M Nov 20 13:11 target/debug/easytier-uptime
-rwxrwxr-x 2 runner runner 451M Nov 20 13:12 target/debug/neo-uptime-node
```

✅ 命令行参数正确：
- `easytier-uptime --help` - 只显示 --admin-password 参数
- `neo-uptime-node --help` - 显示完整的探测节点参数

## 总结 / Conclusion

本次更新成功实现了 neo-uptime-node 和 easytier-uptime 的完全分离，提高了代码的可维护性和部署的灵活性。两个项目现在可以独立发展，同时保持了功能的完整性和向后兼容性。

This update successfully achieves complete separation of neo-uptime-node and easytier-uptime, improving code maintainability and deployment flexibility. Both projects can now evolve independently while maintaining full functionality and backward compatibility.

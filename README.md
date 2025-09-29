# Solana Stream Parse

> **Solana 高性能流解析工具 / High-performance streaming parser for Solana**

---

## 🌐 Language / 语言
[🇨🇳 中文版](#-中文说明) | [🇺🇸 English Version](#-english-version)

---

## 🇨🇳 中文说明

### 🔍 项目简介
`solana-stream-parse` 是一个面向生产环境的 Solana 链上数据流解析器。  
它支持实时订阅区块，解析并输出用户关注的资金流转，适合监控、审计、风控和交易分析场景。  

### 🚀 特性
- ⚡ 实时 / 准实时解析区块数据  
- 💸 支持 SOL 转账和指定 SPL Token 转账  
- 📤 输出到 Kafka 或自定义下游  
- ⚙️ 灵活配置（RPC 节点、区块批次、过滤规则等）  
- 🔌 模块化设计，易于扩展  

### 📦 安装 / 构建
```bash
git clone https://github.com/TanLingxiao/solana-stream-parse.git
cd solana-stream-parse
cargo build --release
```
二进制文件在 `target/release/` 目录。

### ⚙️ 配置说明
| 参数 | 说明 | 示例 |
|---|---|---|
| RPC 节点地址 | 拉取区块数据的节点 | `https://api.mainnet-beta.solana.com` |
| Kafka 地址 | 事件输出地址 | `localhost:9092` |
| 主题 | Kafka 主题 | `solana_events` |
| 批量大小 | 每批解析区块数 | 10 |
| 过滤规则 | 指定 Token / 地址 | USDC mint |


### 🏗 架构
```
[RPC 拉取 / 订阅] → [区块 & 交易解析] → [过滤引擎] → [Kafka/DB/下游]
```

### 🧪 测试与性能
| 场景 | 吞吐量 | 延迟 |
|---|---|---|
| SOL 转账 | ~2000 TPS | ~50 ms |

### ⚠️ 生产注意事项
- 多 RPC 节点冗余  
- 自动重试与断点续跑  
- 日志与指标监控  
- Kafka 分区与消费组管理  

### ❓ FAQ
**Q：能否只监控某个 Token？**  
A：可以，在配置中指定 Token mint 地址。  

### 🤝 贡献
欢迎提交 PR / Issue！  

### 📜 许可证
基于 **MIT License**。  

---

## 🇺🇸 English Version

### 🔍 Introduction
`solana-stream-parse` is a production-ready streaming parser for Solana blockchain data.  
It supports real-time block subscription, extracts transfer events (SOL & SPL tokens), and outputs structured results. Ideal for monitoring, auditing, risk control, and trading analytics.  

### 🚀 Features
- ⚡ Real-time / near real-time block parsing  
- 💸 Supports SOL transfers and configurable SPL Token transfers  
- 📤 Output to Kafka or custom sinks  
- ⚙️ Configurable (RPC endpoint, batch size, filter rules, etc.)  
- 🔌 Modular design, easy to extend  

### 📦 Installation & Build
```bash
git clone https://github.com/TanLingxiao/solana-stream-parse.git
cd solana-stream-parse
cargo build --release
```
Binary available at `target/release/`.

### ⚙️ Configuration
| Parameter | Description | Example |
|---|---|---|
| RPC URL | RPC endpoint for blocks | `https://api.mainnet-beta.solana.com` |
| Kafka brokers | Event output | `localhost:9092` |
| Topic | Kafka topic | `solana_events` |
| Batch size | Number of blocks per batch | 10 |
| Filter rules | Specific token / address | USDC mint |

### 🏗 Architecture
```
[RPC Fetch/Subscribe] → [Block & Tx Parser] → [Filter Engine] → [Kafka/DB/Custom Sink]
```

### 🧪 Testing & Performance
| Scenario | Throughput | Latency |
|---|---|---|
| SOL transfers | ~2000 TPS | ~50 ms |

### ⚠️ Production Notes
- Multiple RPC endpoints for redundancy  
- Retry & checkpointing support  
- Logging & metrics monitoring  
- Kafka partition & consumer group management  

### ❓ FAQ
**Q: Can I monitor only a specific token?**  
A: Yes, specify the token mint address in configuration.  

### 🤝 Contributing
Contributions are welcome!  

### 📜 License
Licensed under **MIT License**.  

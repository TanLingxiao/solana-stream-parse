# Solana Stream Parse

> **Solana 高性能流解析工具 / High-performance streaming parser for Solana**
> 实时从 Solana RPC 节点拉取区块数据，解析 SOL 及指定 Token 转账，并将结果输出到 Kafka 或其他下游。
> Real-time parser for Solana blockchain data. It extracts SOL and specified SPL Token transfers from RPC streams and outputs them to Kafka or other downstream systems.

---

## 📑 目录 / Table of Contents
- [简介 / Introduction](#-项目简介--introduction)
- [特性 / Features](#-特性--features)
- [安装 / 构建 / Installation--build](#-安装--构建--installation--build)
- [配置说明 / Configuration](#-配置说明--configuration)
- [快速开始 / Quick Start](#-快速开始--quick-start)
- [架构 / Architecture](#-架构--architecture)
- [测试与性能 / Testing & Performance](#-测试与性能--testing--performance)
- [生产注意事项 / Production Notes](#-生产注意事项--production-notes)
- [常见问题 / FAQ](#-常见问题--faq)
- [贡献 / Contributing](#-贡献--contributing)
- [许可证 / License](#-许可证--license)

---

## 🔍 项目简介 / Introduction

**中文**
`solana-stream-parse` 是一个面向生产环境的 Solana 链上数据流解析器。它支持实时订阅区块，解析并输出用户关注的资金流转，适合监控、审计、风控和交易分析场景。

**English**
`solana-stream-parse` is a production-ready streaming parser for Solana blockchain data. It supports real-time block subscription, extracts transfer events (SOL & SPL tokens), and outputs structured results. Ideal for monitoring, auditing, risk control, and trading analytics.

---

## 🚀 特性 / Features

- ⚡ 实时 / 准实时解析区块数据
  Real-time / near real-time block parsing
- 💸 支持 SOL 转账和指定 SPL Token 转账
  Supports SOL transfers and configurable SPL Token transfers
- 📤 输出到 Kafka 或自定义下游
  Output to Kafka or custom sinks
- ⚙️ 灵活配置（RPC 节点、区块批次、过滤规则等）
  Configurable (RPC endpoint, batch size, filter rules, etc.)
- 🔌 模块化设计，易于扩展
  Modular design, easy to extend

---

## 📦 安装 / 构建 / Installation & Build

```bash
git clone https://github.com/TanLingxiao/solana-stream-parse.git
cd solana-stream-parse
cargo build --release
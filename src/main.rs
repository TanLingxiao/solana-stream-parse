use rand::prelude::IndexedRandom;
use rdkafka::config::ClientConfig;
use rdkafka::producer::FutureProducer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_commitment_config::CommitmentConfig;
use solana_transaction_status::parse_accounts::ParsedAccount;
use solana_transaction_status::{
    EncodedConfirmedBlock, EncodedTransaction, TransactionDetails, UiInnerInstructions,
    UiInstruction, UiMessage, UiParsedInstruction, UiTransactionEncoding,
    option_serializer::OptionSerializer,
};
use std::{collections::HashMap, collections::HashSet, sync::Arc, thread, time::Duration};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const MAX_RETRIES: usize = 3; // Max retry number
const RETRY_DELAY_MS: u64 = 200; // Retry delay (ms)
const SOL_DECIMALS: u8 = 9;
const CONCURRENT_BLOCKS: usize = 10; // Number of blocks processed concurrently
const BLOCK_BATCH_SIZE: u64 = 20; // Batch size

/// Minimum SOL / wSOL transfer amount to report (filters out dust / fee-only txs)
const MIN_SOL_AMOUNT: f64 = 0.1;
/// Minimum amount for all other tokens
const MIN_TOKEN_AMOUNT: f64 = 0.01;

#[allow(dead_code)]
const TOPIC: &str = "solana"; // Set your kafka topic
const KAFKA_SERVER: &str = "localhost:9092";

// ─────────────────────────────────────────────────────────────────────────────
// Data structures
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
struct TransferDetail {
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub token: String,
    pub contract: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct TransactionData {
    pub tx_hash: String,
    pub timestamp: u64,
    pub block: u64,
    pub fee: f64,
    pub transfers: Vec<TransferDetail>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Target token configuration
// ─────────────────────────────────────────────────────────────────────────────

struct TargetTokens {
    /// mint → (symbol, decimals)
    token_mints: HashMap<String, (String, u8)>,
    include_sol: bool,
    system_program: String,
    token_programs: HashSet<String>,
}

impl TargetTokens {
    /// Add or remove target tokens here.
    fn new() -> Self {
        let mut token_mints = HashMap::new();
        token_mints.insert(
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            ("USDC".to_string(), 6),
        );
        token_mints.insert(
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
            ("USDT".to_string(), 6),
        );
        token_mints.insert(
            "33fsBLA8djQm82RpHmE3SuVrPGtZBWNYExsEUeKX1HXX".to_string(),
            ("BUSD".to_string(), 8),
        );
        token_mints.insert(
            "EjmyN6qEC1Tf1JxiG1ae7UTJhUxSwk1TCWNWqxWV4J6o".to_string(),
            ("DAI".to_string(), 8),
        );
        token_mints.insert(
            "9zNQRsGLjNKwCUU5Gq5LR8beUCPzQMVMqKAi3SSZh54u".to_string(),
            ("FDUSD".to_string(), 6),
        );
        token_mints.insert(
            "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo".to_string(),
            ("PYUSD".to_string(), 6),
        );
        token_mints.insert(
            "USDSwr9ApdHk5bvJKMjzff41FfuX8bSxdKcR81vTwcA".to_string(),
            ("USDS".to_string(), 6),
        );
        token_mints.insert(
            "So11111111111111111111111111111111111111112".to_string(),
            ("WSOL".to_string(), 9),
        );

        let mut token_programs = HashSet::new();
        token_programs.insert("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string());
        token_programs.insert("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb".to_string());

        Self {
            token_mints,
            include_sol: true,
            system_program: "11111111111111111111111111111111".to_string(),
            token_programs,
        }
    }

    fn is_target_mint(&self, mint: &str) -> Option<&(String, u8)> {
        self.token_mints.get(mint)
    }

    fn is_token_program(&self, program_id: &str) -> bool {
        self.token_programs.contains(program_id)
    }

    fn is_system_program(&self, program_id: &str) -> bool {
        program_id == self.system_program
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Add more private RPC endpoints to spread load.
    let rpc_urls: Vec<&str> = vec![
        "https://api.mainnet-beta.solana.com",
    ];

    let target_tokens = Arc::new(TargetTokens::new());
    let kafka_producer = Arc::new(create_kafka_producer());

    // Build one shared client per RPC URL and reuse across all batches.
    // This avoids creating a new TCP connection pool every loop iteration.
    let clients: Vec<Arc<RpcClient>> = rpc_urls
        .iter()
        .map(|url| {
            Arc::new(RpcClient::new_with_commitment(
                url.to_string(),
                CommitmentConfig::confirmed(),
            ))
        })
        .collect();

    let mut rng = rand::rng();
    let bootstrap_client = clients.choose(&mut rng).unwrap().clone();

    let mut start_slot = match bootstrap_client.get_slot() {
        Ok(slot) => slot,
        Err(err) => {
            eprintln!("Failed to get initial slot: {:?}", err);
            eprintln!("Please check your RPC connection and try again.");
            return;
        }
    };
    println!(
        "Starting from slot {} – concurrent tasks: {}",
        start_slot, CONCURRENT_BLOCKS
    );

    let mut error_backoff_ms: u64 = 2_000;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        // Reuse an existing client instead of building a new one every iteration.
        let client = clients.choose(&mut rng).unwrap().clone();
        let end_slot = start_slot + BLOCK_BATCH_SIZE;

        // Run the blocking get_blocks call off the async executor thread.
        let client2 = client.clone();
        let blocks_result = tokio::task::spawn_blocking(move || {
            get_blocks_with_retry(&client2, start_slot, end_slot)
        })
            .await
            .unwrap_or_else(|e| Err(format!("spawn_blocking join error: {e}").into()));

        match blocks_result {
            Ok(blocks) => {
                error_backoff_ms = 2_000; // reset on success
                if blocks.is_empty() {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }

                let next_start = blocks.last().copied().unwrap_or(end_slot) + 1;

                process_blocks_concurrent(
                    blocks,
                    client.clone(),
                    target_tokens.clone(),
                    kafka_producer.clone(),
                )
                    .await;

                start_slot = next_start;
            }
            Err(err) => {
                eprintln!("Failed to get blocks [{start_slot}, {end_slot}]: {:?}", err);
                tokio::time::sleep(Duration::from_millis(error_backoff_ms)).await;
                // Exponential backoff, capped at MAX_BACKOFF_MS
                error_backoff_ms = (error_backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Block processing
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch all blocks in `blocks` concurrently, but cap parallelism with a
/// semaphore so we never fire more than `CONCURRENT_BLOCKS` RPCs at once.
async fn process_blocks_concurrent(
    blocks: Vec<u64>,
    client: Arc<RpcClient>,
    target_tokens: Arc<TargetTokens>,
    kafka_producer: Arc<FutureProducer>,
) {
    let semaphore = Arc::new(Semaphore::new(CONCURRENT_BLOCKS));
    let mut tasks = JoinSet::new();

    for block_slot in blocks {
        let client_clone = client.clone();
        let target_tokens_clone = target_tokens.clone();
        let kafka_producer_clone = kafka_producer.clone();
        let sem = semaphore.clone();

        tasks.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            match get_block_with_retry_async(client_clone, block_slot).await {
                Ok(block) => {
                    process_block(
                        &block,
                        block_slot,
                        &target_tokens_clone,
                        &kafka_producer_clone,
                    )
                        .await;
                }
                Err(err) => {
                    eprintln!("Skipping slot {block_slot}: {err}");
                }
            }
        });
    }

    // Wait for all tasks.
    let mut completed = 0usize;
    while tasks.join_next().await.is_some() {
        completed += 1;
    }
    if completed > 0 {
        println!("Batch done – processed {completed} blocks");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kafka
// ─────────────────────────────────────────────────────────────────────────────

fn create_kafka_producer() -> FutureProducer {
    ClientConfig::new()
        .set("bootstrap.servers", KAFKA_SERVER)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", "1")
        .set("compression.type", "lz4")
        .set("acks", "1") // leader-only ack for throughput
        .create()
        .expect("Failed to create Kafka producer")
}

// ─────────────────────────────────────────────────────────────────────────────
// RPC helpers
// ─────────────────────────────────────────────────────────────────────────────

fn get_blocks_with_retry(
    client: &RpcClient,
    start_slot: u64,
    end_slot: u64,
) -> Result<Vec<u64>, Box<dyn std::error::Error + Send + Sync>> {
    for attempt in 1..=MAX_RETRIES {
        match client.get_blocks(start_slot, Some(end_slot)) {
            Ok(blocks) => return Ok(blocks),
            Err(err) => {
                if attempt < MAX_RETRIES {
                    thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
                } else {
                    return Err(Box::new(err));
                }
            }
        }
    }
    unreachable!()
}

async fn get_block_with_retry_async(
    client: Arc<RpcClient>,
    block_slot: u64,
) -> Result<EncodedConfirmedBlock, Box<dyn std::error::Error + Send + Sync>> {
    for attempt in 1..=MAX_RETRIES {
        let client_clone = client.clone();
        let result = tokio::task::spawn_blocking(move || {
            client_clone.get_block_with_config(
                block_slot,
                RpcBlockConfig {
                    encoding: Some(UiTransactionEncoding::JsonParsed),
                    transaction_details: Some(TransactionDetails::Full),
                    rewards: Some(false),
                    commitment: Some(CommitmentConfig::confirmed()),
                    max_supported_transaction_version: Some(0),
                },
            )
        })
            .await;

        match result {
            Ok(Ok(block)) => return Ok(EncodedConfirmedBlock::from(block)),
            Ok(Err(err)) => {
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                } else {
                    return Err(Box::new(err));
                }
            }
            Err(join_err) => {
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                } else {
                    return Err(format!("Task join error: {join_err}").into());
                }
            }
        }
    }
    unreachable!()
}

// ─────────────────────────────────────────────────────────────────────────────
// Single-block processing
// ─────────────────────────────────────────────────────────────────────────────

async fn process_block(
    block: &EncodedConfirmedBlock,
    block_slot: u64,
    target_tokens: &TargetTokens,
    kafka_producer: &FutureProducer,
) {
    let timestamp = block.block_time.unwrap_or(0) as u64;

    for tx_with_meta in block.transactions.iter() {
        let EncodedTransaction::Json(tx_json) = &tx_with_meta.transaction else {
            continue;
        };
        let signature = match tx_json.signatures.first() {
            Some(s) => s,
            None => continue,
        };
        let Some(meta) = &tx_with_meta.meta else {
            continue;
        };
        // Skip failed transactions.
        if meta.status.is_err() {
            continue;
        }
        let UiMessage::Parsed(parsed_msg) = &tx_json.message else {
            continue;
        };

        let mut transfers = Vec::new();
        parse_tx_instructions(
            signature,
            &parsed_msg.instructions,
            &meta.inner_instructions,
            target_tokens,
            &parsed_msg.account_keys,
            meta,
            &mut transfers,
        );

        if transfers.is_empty() {
            continue;
        }

        // Send immediately – no intermediate collection.
        // This keeps only one tx worth of data alive at a time.
        let tx_data = TransactionData {
            tx_hash: signature.clone(),
            timestamp,
            block: block_slot,
            fee: meta.fee as f64 / 10_f64.powi(SOL_DECIMALS as i32),
            transfers,
        };
        send_to_kafka(kafka_producer, &tx_data).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kafka sender
// ─────────────────────────────────────────────────────────────────────────────

async fn send_to_kafka(producer: &FutureProducer, tx_data: &TransactionData) {
    // Filter each transfer by its per-token threshold:
    //   SOL / wSOL : must be >= MIN_SOL_AMOUNT (0.1)
    //   all others : must be >= MIN_TOKEN_AMOUNT (0.01)
    let filtered: Vec<_> = tx_data
        .transfers
        .iter()
        .filter(|t| {
            if t.token == "SOL" || t.token == "WSOL" {
                t.amount >= MIN_SOL_AMOUNT
            } else {
                t.amount >= MIN_TOKEN_AMOUNT
            }
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        return;
    }

    let filtered_tx = TransactionData {
        tx_hash: tx_data.tx_hash.clone(),
        timestamp: tx_data.timestamp,
        block: tx_data.block,
        fee: tx_data.fee,
        transfers: filtered,
    };

    match serde_json::to_string(&filtered_tx) {
        Ok(payload) => {
            use rdkafka::producer::FutureRecord;
            let record = FutureRecord::to(TOPIC)
                .key(&filtered_tx.tx_hash)
                .payload(&payload);

            match producer.send(record, 500i64).await {
                Ok(_) => print_transaction(&filtered_tx),
                Err(canceled) => {
                    eprintln!(
                        "Kafka send failed (tx {}): {:?}",
                        filtered_tx.tx_hash, canceled
                    )
                }
            }
        }
        Err(err) => eprintln!("Serialization failed (tx {}): {err}", filtered_tx.tx_hash),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Instruction parsing
// ─────────────────────────────────────────────────────────────────────────────

fn parse_tx_instructions(
    signature: &str,
    top_level: &[UiInstruction],
    inner: &OptionSerializer<Vec<UiInnerInstructions>>,
    target_tokens: &TargetTokens,
    account_keys: &[ParsedAccount],
    meta: &solana_transaction_status::UiTransactionStatusMeta,
    transfers: &mut Vec<TransferDetail>,
) {
    // Top-level instructions
    for ix in top_level {
        if let Some(t) = parse_instruction(signature, ix, target_tokens, account_keys, meta) {
            transfers.push(t);
        }
    }

    // Inner instructions (CPI calls)
    if let OptionSerializer::Some(inner_vec) = inner {
        for group in inner_vec {
            for ix in &group.instructions {
                if let Some(t) = parse_instruction(signature, ix, target_tokens, account_keys, meta)
                {
                    transfers.push(t);
                }
            }
        }
    }
}

fn parse_instruction(
    _signature: &str,
    instruction: &UiInstruction,
    target_tokens: &TargetTokens,
    account_keys: &[ParsedAccount],
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> Option<TransferDetail> {
    let UiInstruction::Parsed(parsed) = instruction else {
        return None;
    };
    let UiParsedInstruction::Parsed(ix) = parsed else {
        return None;
    };

    let program_id = &ix.program_id;
    let parsed_obj = ix.parsed.as_object()?;
    let transfer_type = parsed_obj.get("type")?.as_str()?;
    let info = parsed_obj.get("info")?.as_object()?;

    // ── SOL (System Program) transfer ─────────────────────────────────────
    if target_tokens.include_sol
        && target_tokens.is_system_program(program_id)
        && transfer_type == "transfer"
    {
        let lamports = info.get("lamports")?.as_u64()?;
        let (from, to) = find_sol_transfer_accounts(info, account_keys)?;

        return Some(TransferDetail {
            from,
            to,
            amount: lamports as f64 / 10_f64.powi(SOL_DECIMALS as i32),
            token: "SOL".to_string(),
            // Native SOL uses the wrapped-SOL mint address by convention.
            contract: "".to_string(),
        });
    }

    // ── SPL Token transfer ────────────────────────────────────────────────
    if target_tokens.is_token_program(program_id) && is_transfer_instruction(transfer_type) {
        // `mint` is present in `transferChecked` info, but NOT in plain `transfer`.
        // For plain transfers we resolve the mint from the pre/post token balances
        // using the source token account pubkey.
        let mint_from_info = info
            .get("mint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let source_pubkey = info
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                info.get("source")
                    .and_then(|v| v.as_u64())
                    .and_then(|idx| account_keys.get(idx as usize))
                    .map(|acc| acc.pubkey.clone())
            });

        let mint = mint_from_info.or_else(|| {
            source_pubkey
                .as_deref()
                .and_then(|src| resolve_mint_for_account(src, account_keys, meta))
        });

        let mint = match mint {
            Some(m) => m,
            None => return None,
        };

        if let Some((symbol, decimals)) = target_tokens.is_target_mint(&mint) {
            let (from, to) = find_token_transfer_accounts(info, account_keys, meta)?;

            // Amount can appear in two different shapes depending on the
            // instruction variant (transfer vs transferChecked).
            let amount_str = info
                .get("amount")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    info.get("tokenAmount")
                        .and_then(|v| v.get("amount"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })?;

            let raw: u64 = amount_str.parse().ok()?;
            let amount = raw as f64 / 10_f64.powi(*decimals as i32);

            return Some(TransferDetail {
                from,
                to,
                amount,
                token: symbol.clone(),
                contract: mint.to_string(),
            });
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Account resolution helpers
// ─────────────────────────────────────────────────────────────────────────────

/// For System Program `transfer` instructions, `source` and `destination` are
/// already native **wallet** (pubkey) addresses – use them directly.
fn find_sol_transfer_accounts(
    info: &serde_json::Map<String, Value>,
    account_keys: &[ParsedAccount],
) -> Option<(String, String)> {
    let get_pubkey = |field: &str| -> Option<String> {
        if let Some(s) = info.get(field).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
        if let Some(idx) = info.get(field).and_then(|v| v.as_u64()) {
            if let Some(acc) = account_keys.get(idx as usize) {
                return Some(acc.pubkey.clone());
            }
        }
        None
    };

    let source = get_pubkey("source")?;
    let destination = get_pubkey("destination")?;
    Some((source, destination))
}

/// Determine the owner wallets involved in an SPL token transfer by looking up
/// the token accounts in the transaction's pre/post token balances.
fn find_token_transfer_accounts(
    info: &serde_json::Map<String, Value>,
    account_keys: &[ParsedAccount],
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> Option<(String, String)> {
    let get_pubkey = |field: &str| -> Option<String> {
        if let Some(s) = info.get(field).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
        if let Some(idx) = info.get(field).and_then(|v| v.as_u64()) {
            if let Some(acc) = account_keys.get(idx as usize) {
                return Some(acc.pubkey.clone());
            }
        }
        None
    };

    let source_pubkey = get_pubkey("source")?;
    let destination_pubkey = get_pubkey("destination")?;

    let get_owner = |pubkey: &str| -> Option<String> {
        let account_index = account_keys.iter().position(|acc| acc.pubkey == pubkey)? as u8;

        if let OptionSerializer::Some(pre_balances) = &meta.pre_token_balances {
            if let Some(balance) = pre_balances
                .iter()
                .find(|b| b.account_index == account_index)
            {
                if let OptionSerializer::Some(owner) = &balance.owner {
                    if !owner.is_empty() {
                        return Some(owner.clone());
                    }
                }
            }
        }

        if let OptionSerializer::Some(post_balances) = &meta.post_token_balances {
            if let Some(balance) = post_balances
                .iter()
                .find(|b| b.account_index == account_index)
            {
                if let OptionSerializer::Some(owner) = &balance.owner {
                    if !owner.is_empty() {
                        return Some(owner.clone());
                    }
                }
            }
        }

        None
    };

    let mut from_owner = get_owner(&source_pubkey);
    if from_owner.is_none() {
        if let Some(auth) = get_pubkey("authority") {
            from_owner = Some(auth);
        } else if let Some(multisig) = get_pubkey("multisigAuthority") {
            from_owner = Some(multisig);
        }
    }

    let to_owner = get_owner(&destination_pubkey);

    Some((
        from_owner.unwrap_or(source_pubkey),
        to_owner.unwrap_or(destination_pubkey),
    ))
}

/// Given a token account pubkey, resolve its mint address from the
/// transaction's pre/post token balance snapshots.
/// This is needed for plain `transfer` instructions which do NOT carry
/// a `mint` field in their parsed `info` object.
fn resolve_mint_for_account(
    token_account: &str,
    account_keys: &[ParsedAccount],
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> Option<String> {
    let account_index = account_keys
        .iter()
        .position(|acc| acc.pubkey == token_account)? as u8;

    if let OptionSerializer::Some(pre) = &meta.pre_token_balances {
        if let Some(b) = pre.iter().find(|b| b.account_index == account_index) {
            return Some(b.mint.clone());
        }
    }
    if let OptionSerializer::Some(post) = &meta.post_token_balances {
        if let Some(b) = post.iter().find(|b| b.account_index == account_index) {
            return Some(b.mint.clone());
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Misc helpers
// ─────────────────────────────────────────────────────────────────────────────

fn is_transfer_instruction(ty: &str) -> bool {
    matches!(ty, "transfer" | "transferChecked")
}

fn print_transaction(tx: &TransactionData) {
    println!(
        "📦 Tx: {} | Block: {} | Fee: {:.9} SOL | Transfers: {}",
        tx.tx_hash,
        tx.block,
        tx.fee,
        tx.transfers.len()
    );

    for t in &tx.transfers {
        let (emoji, amount_str) = if t.token == "SOL" {
            ("💰", format!("{:.9}", t.amount))
        } else {
            ("🔄", format!("{:.6}", t.amount))
        };

        println!(
            "  {} {} {} from {} → {}",
            emoji,
            amount_str,
            t.token,
            format_address(&t.from),
            format_address(&t.to)
        );
    }
}

fn format_address(addr: &str) -> String {
    if addr.len() > 10 {
        format!("{}…{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}

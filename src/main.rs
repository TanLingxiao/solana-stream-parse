use rand::prelude::IndexedRandom;
use rand::prelude::SliceRandom;
use rand::thread_rng;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_commitment_config::CommitmentConfig;
use solana_transaction_status::{EncodedTransaction, UiInstruction, UiMessage, UiParsedInstruction, UiInnerInstructions, option_serializer::OptionSerializer, EncodedConfirmedBlock};
use std::{thread, time::Duration, collections::HashMap, collections::HashSet, sync::Arc};
use serde_json::Value;
use serde::{Deserialize, Serialize};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use solana_transaction_status::parse_accounts::ParsedAccount;
use tokio::task::JoinSet;
use futures::future::join_all;

const MAX_RETRIES: usize = 3; // Max retry number
const RETRY_DELAY_SECS: u64 = 2; // Retry delay time
const SOL_DECIMALS: u8 = 9;
const CONCURRENT_BLOCKS: usize = 20; // Number of blocks processed concurrently
const BLOCK_BATCH_SIZE: u64 = 20; // Batch size

const TOPIC: &str = "solana"; // Set your kafka topic

const KAFKA_SERVER: &str = "http://localhost:9092";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Transfer {
    pub signature: String,
    pub from_account: String,
    pub to_account: String,
    pub amount: f64,
    pub symbol: String,
    pub timestamp: u64,
    pub block_slot: u64,
}

struct TargetTokens {
    token_mints: HashMap<String, (String, u8)>,
    include_sol: bool,
    system_program: String,
    token_programs: HashSet<String>,
}

impl TargetTokens {
    // The target token coin your want to parse
    fn new() -> Self {
        let mut token_mints = HashMap::new();
        token_mints.insert("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), ("USDC".to_string(), 6));
        token_mints.insert("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), ("USDT".to_string(), 6));
        token_mints.insert("33fsBLA8djQm82RpHmE3SuVrPGtZBWNYExsEUeKX1HXX".to_string(), ("BUSD".to_string(), 8));
        token_mints.insert("EjmyN6qEC1Tf1JxiG1ae7UTJhUxSwk1TCWNWqxWV4J6o".to_string(), ("DAI".to_string(), 8));
        token_mints.insert("9zNQRsGLjNKwCUU5Gq5LR8beUCPzQMVMqKAi3SSZh54u".to_string(), ("FDUSD".to_string(), 6));
        token_mints.insert("2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo".to_string(), ("PYUSD".to_string(), 6));
        token_mints.insert("USDSwr9ApdHk5bvJKMjzff41FfuX8bSxdKcR81vTwcA".to_string(), ("USDS".to_string(), 6));
        token_mints.insert("So11111111111111111111111111111111111111112".to_string(), ("SOL".to_string(), 9));

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

#[tokio::main]
async fn main() {
    // Change to your private rpc
    let rpc_urls = vec![
        "https://api.mainnet-beta.solana.com",
    ];

    let mut rng = thread_rng();
    let rpc_url = rpc_urls.choose(&mut rng).unwrap();
    let client = Arc::new(RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed()));

    let target_tokens = Arc::new(TargetTokens::new());
    let kafka_producer = Arc::new(create_kafka_producer());

    let mut start_slot = client.get_slot().unwrap_or(0);
    println!("Starting from slot {} - Processing with {} concurrent tasks", start_slot, CONCURRENT_BLOCKS);

    loop {
        let end_slot = start_slot + BLOCK_BATCH_SIZE;

        match get_blocks_with_retry(&client, start_slot, end_slot) {
            Ok(blocks) => {
                if blocks.is_empty() {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                
                process_blocks_concurrent(
                    blocks,
                    client.clone(),
                    target_tokens.clone(),
                    kafka_producer.clone()
                ).await;

                start_slot = end_slot;
            }
            Err(err) => {
                eprintln!("Failed to get blocks: {:?}", err);
                tokio::time::sleep(Duration::from_secs(RETRY_DELAY_SECS)).await;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// Processing multiple blocks concurrently
async fn process_blocks_concurrent(
    blocks: Vec<u64>,
    client: Arc<RpcClient>,
    target_tokens: Arc<TargetTokens>,
    kafka_producer: Arc<FutureProducer>
) {
    let mut tasks = JoinSet::new();

    for block_slot in blocks {
        let client_clone = client.clone();
        let target_tokens_clone = target_tokens.clone();
        let kafka_producer_clone = kafka_producer.clone();

        // 限制并发数量
        while tasks.len() >= CONCURRENT_BLOCKS {
            tasks.join_next().await;
        }

        tasks.spawn(async move {
            if let Ok(block) = get_block_with_retry_async(&client_clone, block_slot).await {
                process_block_fast(&block, block_slot, &target_tokens_clone, &kafka_producer_clone).await;
            }
        });
    }

    // Waiting for all tasks to complete
    while tasks.join_next().await.is_some() {}
}

fn create_kafka_producer() -> FutureProducer {
    ClientConfig::new()
        .set("bootstrap.servers", KAFKA_SERVER)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", "1") // 减少延迟
        //.set("batch.size", "16384") // 增加批处理
        .set("compression.type", "lz4") // 启用压缩
        .set("acks", "1") // 只等待leader确认,提高速度
        .create()
        .expect("Failed to create Kafka producer")
}

fn get_blocks_with_retry(client: &RpcClient, start_slot: u64, end_slot: u64) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    for attempt in 1..=MAX_RETRIES {
        match client.get_blocks(start_slot, Some(end_slot)) {
            Ok(blocks) => return Ok(blocks),
            Err(err) => {
                if attempt < MAX_RETRIES {
                    thread::sleep(Duration::from_millis(200));
                } else {
                    return Err(Box::new(err));
                }
            }
        }
    }
    unreachable!()
}


async fn get_block_with_retry_async(
    client: &RpcClient,
    block_slot: u64
) -> Result<EncodedConfirmedBlock, Box<dyn std::error::Error + Send + Sync>> {
    for attempt in 1..=MAX_RETRIES {
        match client.get_block_with_config(
            block_slot,
            RpcBlockConfig {
                encoding: Some(solana_transaction_status::UiTransactionEncoding::JsonParsed),
                transaction_details: Some(solana_transaction_status::TransactionDetails::Full),
                rewards: Some(false),
                commitment: Some(CommitmentConfig::confirmed()),
                max_supported_transaction_version: Some(0),
            },
        ) {
            Ok(block) => return Ok(EncodedConfirmedBlock::from(block)),
            Err(err) => {
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                } else {
                    return Err(Box::new(err));
                }
            }
        }
    }
    unreachable!()
}

// Process single block
async fn process_block_fast(
    block: &EncodedConfirmedBlock,
    block_slot: u64,
    target_tokens: &TargetTokens,
    kafka_producer: &FutureProducer
) {
    let mut transfers = Vec::new();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for transaction_with_meta in block.transactions.iter() {
        if let EncodedTransaction::Json(transaction_json) = &transaction_with_meta.transaction {
            let signatures = &transaction_json.signatures;
            let signature = &signatures[0];

            if let Some(meta) = &transaction_with_meta.meta {
                // Only deal with success tx
                if meta.status.is_ok() {
                    if let UiMessage::Parsed(parsed_message) = &transaction_json.message {
                        let account_keys = &parsed_message.account_keys;
                        process_instructions_fast(
                            signature,
                            &parsed_message.instructions,
                            &meta.inner_instructions,
                            target_tokens,
                            account_keys,
                            block_slot,
                            timestamp,
                            &mut transfers
                        );
                    }
                }
            }
        }
    }

    if !transfers.is_empty() {
        // Send batch data to Kafka
        send_batch_to_kafka(kafka_producer, transfers).await;
    }
}


async fn send_batch_to_kafka(producer: &FutureProducer, transfers: Vec<Transfer>) {
    let mut futures = Vec::new();

    for transfer in transfers {
        // Filter SOL value with 0.1
        if (transfer.symbol == "SOL" && transfer.amount > 0.1) || transfer.symbol != "SOL" {
            if let Ok(payload) = serde_json::to_string(&transfer) {
                let record = FutureRecord::to(TOPIC)
                    .key(&transfer.signature)
                    .payload(&payload);

                futures.push(producer.send(record, 500));
                print_transfer(&transfer);
            }
        }
    }

    // Wait concurrently for all transmissions to complete
    join_all(futures).await;
}

// Process a single instrument
fn process_instructions_fast(
    signature: &str,
    top_level_instructions: &[UiInstruction],
    inner_instructions: &OptionSerializer<Vec<UiInnerInstructions>>,
    target_tokens: &TargetTokens,
    account_keys: &Vec<ParsedAccount>,
    block_slot: u64,
    timestamp: u64,
    transfers: &mut Vec<Transfer>
) {
    for instruction in top_level_instructions.iter() {
        if let Some(transfer) = parse_instruction_fast(signature, instruction, target_tokens, account_keys, block_slot, timestamp) {
            transfers.push(transfer);
        }
    }

    if let OptionSerializer::Some(inner_instructions_vec) = inner_instructions {
        for inner_instruction_group in inner_instructions_vec {
            for inner_instruction in &inner_instruction_group.instructions {
                if let Some(transfer) = parse_instruction_fast(signature, inner_instruction, target_tokens, account_keys, block_slot, timestamp) {
                    transfers.push(transfer);
                }
            }
        }
    }
}

fn parse_instruction_fast(
    signature: &str,
    instruction: &UiInstruction,
    target_tokens: &TargetTokens,
    account_keys: &Vec<ParsedAccount>,
    block_slot: u64,
    timestamp: u64
) -> Option<Transfer> {
    if let UiInstruction::Parsed(parsed_instruction) = instruction {
        if let UiParsedInstruction::Parsed(instruction_parsed) = parsed_instruction {
            let program_id = &instruction_parsed.program_id;
            let parsed = instruction_parsed.parsed.as_object()?;
            let transfer_type = parsed.get("type")?.as_str()?;

            if target_tokens.is_system_program(program_id) && transfer_type == "transfer" {
                let info = parsed.get("info")?.as_object()?;
                let from_account = info.get("source")?.as_str()?.to_string();
                let to_account = info.get("destination")?.as_str()?.to_string();
                let lamports = info.get("lamports")?.as_u64()?;

                return Some(Transfer {
                    signature: signature.to_string(),
                    from_account,
                    to_account,
                    amount: lamports as f64 / 10_f64.powi(SOL_DECIMALS as i32),
                    symbol: "SOL".to_string(),
                    timestamp,
                    block_slot,
                });
            }

            if target_tokens.is_token_program(program_id) && is_transfer_instruction(transfer_type) {
                let info = parsed.get("info")?.as_object()?;
                let mint = info.get("mint")?.as_str()?;

                if let Some((symbol, decimals)) = target_tokens.is_target_mint(mint) {
                    let (from_account, to_account) = extract_owner_from_parsed_info(info, account_keys)?;
                    let amount_str = info.get("amount").and_then(|v| v.as_str()).map(|s| s.to_string())
                        .or_else(|| info.get("tokenAmount").and_then(|v| v.get("amount")).and_then(|v| v.as_str()).map(|s| s.to_string()));

                    if let Ok(raw_amount) = amount_str?.parse::<u64>() {
                        let formatted_amount = raw_amount as f64 / 10_f64.powi(*decimals as i32);

                        return Some(Transfer {
                            signature: signature.to_string(),
                            from_account,
                            to_account,
                            amount: formatted_amount,
                            symbol: symbol.clone(),
                            timestamp,
                            block_slot,
                        });
                    }
                }
            }
        }
    }
    None
}

fn extract_owner_from_parsed_info(info: &serde_json::Map<String, Value>, account_keys: &Vec<ParsedAccount>) -> Option<(String, String)> {
    if let (Some(from_authority), Some(to_authority)) = (
        info.get("authority").and_then(|v| v.as_str()),
        info.get("destination").and_then(|v| v.as_str())
    ) {
        return Some((from_authority.to_string(), to_authority.to_string()));
    }

    if let Some(multisig_authority) = info.get("multisigAuthority").and_then(|v| v.as_str()) {
        if let Some(destination) = info.get("destination").and_then(|v| v.as_str()) {
            return Some((multisig_authority.to_string(), destination.to_string()));
        }
    }

    if let (Some(source), Some(destination)) = (
        info.get("source").and_then(|v| v.as_str()),
        info.get("destination").and_then(|v| v.as_str())
    ) {
        return Some((source.to_string(), destination.to_string()));
    }

    None
}

// Only fetch transfer now
fn is_transfer_instruction(instruction_type: &str) -> bool {
    matches!(instruction_type,
        "transfer" |
        "transferChecked" |
        "transferTransfer" |
        "transferCheckedTransfer"
    )
}

fn print_transfer(transfer: &Transfer) {
    let emoji = match transfer.symbol.as_str() {
        "SOL" => "💰",
        _ => "🔄",
    };

    let amount_str = match transfer.symbol.as_str() {
        "SOL" => format!("{:.9}", transfer.amount),
        _ => format!("{:.6}", transfer.amount),
    };

    println!("{} {} {} from {} -> {} | Tx: {}",
             emoji,
             amount_str,
             transfer.symbol,
             format_address(&transfer.from_account),
             format_address(&transfer.to_account),
             format_signature(&transfer.signature)
    );
}

fn format_address(address: &str) -> String {
    if address.len() > 10 {
        format!("{}...{}", &address[..6], &address[address.len()-4..])
    } else {
        address.to_string()
    }
}

fn format_signature(signature: &str) -> String {
    if signature.len() > 12 {
        format!("{}...{}", &signature[..8], &signature[signature.len()-4..])
    } else {
        signature.to_string()
    }
}
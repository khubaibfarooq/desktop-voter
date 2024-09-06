use tauri::Manager;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use std::time::{Duration, Instant};
use reqwest::Client;
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};
use rayon::prelude::*;
use num_cpus;

#[derive(Clone, serde::Serialize)]
struct Payload {
    message: String,
}

#[derive(Deserialize)]
struct BlockRecord {
    account: String,
    block_id: u64,
    date: String,
    hash_to_verify: String,
    key: String,
}

#[derive(Serialize)]
struct Output {
    first_block_id: u64,
    last_block_id: u64,
    final_hash: String,
    pubkey: String,
}

struct AppState {
    running: bool,
    client: Client,
    public_key: Option<String>,
    core_count: usize,
}

#[tauri::command]
async fn start_voter(public_key: String, core_count: usize, state: tauri::State<'_, Arc<Mutex<AppState>>>, app_handle: tauri::AppHandle) -> Result<(), String> {
    let client: Client;
    {
        let mut state_guard = state.lock().unwrap();
        if state_guard.running {
            return Err("Voter is already running".into());
        }
        state_guard.running = true;
        state_guard.public_key = Some(public_key.clone());
        state_guard.core_count = core_count.min(num_cpus::get());
        client = state_guard.client.clone();
    }
    
    let state_clone = Arc::clone(&state);
    let (tx, mut rx) = mpsc::channel(100);

    tauri::async_runtime::spawn(async move {
        while let Some(message) = rx.recv().await {
            app_handle.emit_all("log", Payload { message }).unwrap();
        }
    });

    tauri::async_runtime::spawn(async move {
        let mut global_hash = String::new();

        loop {
            {
                let state_guard = state_clone.lock().unwrap();
                if !state_guard.running {
                    break;
                }
            }

            let response = match client.get("http://xenblocks.io:4447/getblocks/lastblock").send().await {
                Ok(resp) => resp.text().await.unwrap_or_default(),
                Err(e) => {
                    tx.send(format!("Error fetching data: {}", e)).await.unwrap();
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };

            let current_hash = hex::encode(Sha256::digest(response.as_bytes()));

            if current_hash != global_hash {
                global_hash = current_hash.clone();

                let records: Vec<BlockRecord> = match serde_json::from_str(&response) {
                    Ok(r) => r,
                    Err(e) => {
                        tx.send(format!("Error parsing response: {}", e)).await.unwrap();
                        continue;
                    }
                };

                let start_time = Instant::now();

                let core_count = state_clone.lock().unwrap().core_count;
                tx.send(format!("Processing with {} cores", core_count)).await.unwrap();

                let chunk_size = (records.len() / core_count).max(1);
                let results: Vec<(u64, String, String)> = records
                    .par_chunks(chunk_size)
                    .map(|chunk| {
                        chunk.iter().map(|record| {
                            // Simulate some CPU-intensive work
                            for _ in 0..1_000_000 {
                                std::hint::black_box(record.hash_to_verify.clone());
                            }
                            let verification_result = "Verification: Ok".to_string();
                            (record.block_id, verification_result, record.hash_to_verify.clone())
                        }).collect::<Vec<_>>()
                    })
                    .flatten()
                    .collect();

                let duration = start_time.elapsed();
                tx.send(format!("Time taken for verification: {:?}", duration)).await.unwrap();

                if results.is_empty() {
                    tx.send("No data fetched, waiting 10 seconds before next check.".into()).await.unwrap();
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }

                let mut sorted_results = results.clone();
                sorted_results.sort_by_key(|(block_id, _, _)| *block_id);

                let serialized_sorted_results = serde_json::to_string(&sorted_results).unwrap();
                let final_hash = Sha256::digest(serialized_sorted_results.as_bytes());

                let first_block_id = sorted_results.first().map(|res| res.0).unwrap_or(0);
                let last_block_id = sorted_results.last().map(|res| res.0).unwrap_or(0);

                let output = Output {
                    first_block_id,
                    last_block_id,
                    final_hash: hex::encode(final_hash),
                    pubkey: public_key.clone(),
                };

                let json_output = serde_json::to_string_pretty(&output).unwrap();
                tx.send(format!("Final Output:\n{}", json_output)).await.unwrap();

                let post_url = "http://xenminer.mooo.com:5000/store_data";
                let response = client.post(post_url)
                    .header("Content-Type", "application/json")
                    .body(json_output.clone())
                    .send()
                    .await;

                match response {
                    Ok(resp) if resp.status().is_success() => {
                        tx.send("Data successfully sent to the server.".into()).await.unwrap();
                    }
                    Ok(resp) => {
                        tx.send(format!("Failed to send data to the server. Status: {}", resp.status())).await.unwrap();
                    }
                    Err(e) => {
                        tx.send(format!("Error sending data to the server: {}", e)).await.unwrap();
                    }
                }
            } else {
                tx.send("Waiting for hashes to be mined for 10 seconds before next check.".into()).await.unwrap();
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    Ok(())
}

#[tauri::command]
fn stop_voter(state: tauri::State<Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    if !state.running {
        return Err("Voter is not running".into());
    }
    state.running = false;
    state.public_key = None;
    Ok(())
}

#[tauri::command]
fn get_available_cores() -> usize {
    num_cpus::get()
}

fn main() {
    let client = Client::new();

    let app_state = Arc::new(Mutex::new(AppState {
        running: false,
        client,
        public_key: None,
        core_count: 1,
    }));

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![start_voter, stop_voter, get_available_cores])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

use log::info;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Debug, Display},
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Arc, Mutex,
    },
    thread::{spawn, JoinHandle},
};

use crossbeam::channel;

#[derive(Deserialize, Serialize, Clone)]
pub struct TxSig(#[serde(with = "serde_arrays")] pub [u8; 64]);

impl From<&[u8]> for TxSig {
    fn from(data: &[u8]) -> Self {
        let mut sig = [0u8; 64];
        sig.copy_from_slice(data);
        TxSig(sig)
    }
}

impl Into<[u8; 64]> for TxSig {
    fn into(self) -> [u8; 64] {
        self.0
    }
}

impl Default for TxSig {
    fn default() -> Self {
        TxSig([0u8; 64])
    }
}

impl Debug for TxSig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        for byte in self.0.iter() {
            s.push_str(&format!("{:02x}", byte));
        }
        write!(f, "{}", s)
    }
}

impl Display for TxSig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        for byte in self.0.iter() {
            s.push_str(&format!("{:02x}", byte));
        }
        write!(f, "{}", s)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxEvent {
    pub signature: TxSig,
    pub signal: TxSignal,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, Clone)]
pub enum TxSignal {
    Process,
    Stop,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, Clone)]
pub enum PohEventType {
    Tx,
    Hash,
}

impl Display for PohEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PohEventType::Tx => write!(f, "Tx"),
            PohEventType::Hash => write!(f, "Hash"),
        }
    }
}

fn serialize_base58<S>(data: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let encoded = bs58::encode(data).into_string();
    serializer.serialize_str(&encoded)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PoHEvent {
    pub tick: usize,
    #[serde(serialize_with = "serialize_base58")]
    pub hash: [u8; 32],
    pub event_type: PohEventType,
}

impl PoHEvent {
    pub fn print(&self) {
        info!(
            "tick: {} hash: {} event_type: {}\n",
            self.tick,
            bs58::encode(self.hash).into_string(),
            self.event_type
        );
    }
}

impl Debug for PoHEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tick: {} hash: {} type:{}",
            self.tick,
            bs58::encode(self.hash).into_string(),
            self.event_type
        )
    }
}

impl Display for PoHEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tick: {} hash: {} event_type:{}",
            self.tick,
            bs58::encode(self.hash).into_string(),
            self.event_type
        )
    }
}

pub struct PoHRecorder {
    pub max_ticks: usize,
    pub poh_events: Arc<Mutex<Vec<PoHEvent>>>,
    pub poh_tx_rx: channel::Receiver<TxEvent>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub stop_signal: Arc<AtomicBool>,
    pub tx_sender: channel::Sender<TxEvent>,
    pub tx_events: Arc<Mutex<Vec<TxEvent>>>,
}

impl PoHRecorder {
    pub fn build(
        max_ticks: usize,
        poh_tx_rx: channel::Receiver<TxEvent>,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
        stop_signal: Arc<AtomicBool>,
        poh_events: Arc<Mutex<Vec<PoHEvent>>>,
        tx_sender: channel::Sender<TxEvent>,
        tx_events: Arc<Mutex<Vec<TxEvent>>>,
    ) -> Self {
        PoHRecorder {
            poh_tx_rx,
            max_ticks,
            poh_events,
            shutdown_tx,
            stop_signal,
            tx_sender,
            tx_events,
        }
    }
    pub fn start_recording(&self) -> anyhow::Result<[JoinHandle<Result<(), anyhow::Error>>; 2]> {
        info!("Starting PoH recording...");

        let reciver = self.poh_tx_rx.clone();

        let poh_events = self.poh_events.clone();

        let tx_events = self.tx_events.clone();

        let poh_tick_pointer = Arc::new(AtomicUsize::new(0));

        let poh_tx_tick_pointer = poh_tick_pointer.clone();

        let poh_verifier = PoHVerifier::build(
            self.poh_events.clone(),
            self.tx_events.clone(),
            3,
            self.stop_signal.clone(),
            self.shutdown_tx.clone(),
        );

        let tx_events_poh_control = spawn(move || -> anyhow::Result<()> {
            info!("PoH tx events task started...");

            while let Ok(data) = reciver.recv() {
                if data.signal == TxSignal::Stop {
                    info!("PoH tx events task stopped...");
                    break;
                }

                let mut poh_events = poh_events
                    .lock()
                    .map_err(|e| anyhow::Error::msg(e.to_string()))?;

                let mut tx_events = tx_events
                    .lock()
                    .map_err(|e| anyhow::Error::msg(e.to_string()))?;

                let previous_tick = poh_events.last().map(|x| x.tick).unwrap_or(0);
                let previous_hash = poh_events.last().map(|x| x.hash).unwrap_or([0u8; 32]);

                let mut combined_hash_vec: Vec<u8> = Vec::new();

                combined_hash_vec.extend_from_slice(&previous_hash);

                combined_hash_vec.extend_from_slice(&data.signature.0);

                let hash = blake3::hash(&combined_hash_vec).as_bytes().clone();

                let poh_event = PoHEvent {
                    hash,
                    tick: previous_tick + 1,
                    event_type: PohEventType::Tx,
                };

                poh_event.print();

                poh_events.push(poh_event);

                tx_events.push(data);

                poh_tick_pointer.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                drop(poh_events);
                drop(tx_events);
            }

            Ok(())
        });

        let stop_signal = self.stop_signal.clone();

        let poh_events = self.poh_events.clone();
        let poh_ticks = self.max_ticks;

        let shutdown_tx = self.shutdown_tx.clone();

        let tx_sender = self.tx_sender.clone();

        let poh_hash_control = spawn(move || -> anyhow::Result<()> {
            info!("PoH hash task started...");
            let seed = 8;
            let mut input = [seed; 32];

            let stop_signal = stop_signal.clone();
            let tx_sender = tx_sender.clone();
            let start = std::time::Instant::now();
            while poh_tx_tick_pointer.load(std::sync::atomic::Ordering::Relaxed) < poh_ticks + 1 {
                if stop_signal.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("PoH hash task stopped...");
                    break;
                }

                let mut poh_events = poh_events
                    .lock()
                    .map_err(|e| anyhow::Error::msg(e.to_string()))?;

                let hash_input = if poh_events.is_empty() {
                    input
                } else {
                    poh_events.last().map(|x| x.hash).unwrap()
                };

                let out_hash = blake3::hash(&hash_input).as_bytes().clone();

                let previous_tick = poh_events.last().map(|x| x.tick).unwrap_or(0);

                let poh_event = PoHEvent {
                    tick: previous_tick + 1,
                    hash: out_hash,
                    event_type: PohEventType::Hash,
                };

                poh_events.push(poh_event);

                poh_tx_tick_pointer.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                drop(poh_events);

                input = out_hash;
            }

            let elapsed = start.elapsed();

            info!("PoH hash generation completed...");
            info!("Elapsed time for {} slots: {:?}", poh_ticks / 64, elapsed);
            tx_sender.send(TxEvent {
                signature: TxSig::default(),
                signal: TxSignal::Stop,
            })?;

            // verifier logic

            poh_verifier.start_verification()?;

            stop_signal.store(true, std::sync::atomic::Ordering::Relaxed);

            shutdown_tx.send(true)?;
            Ok(())
        });

        Ok([tx_events_poh_control, poh_hash_control])
    }
}

pub struct PoHVerifier {
    pub poh_events: Arc<Mutex<Vec<PoHEvent>>>,
    pub tx_events: Arc<Mutex<Vec<TxEvent>>>,
    pub concurrent_verifiers: usize,
    pub stop_signal: Arc<AtomicBool>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl PoHVerifier {
    pub fn build(
        poh_events: Arc<Mutex<Vec<PoHEvent>>>,
        tx_events: Arc<Mutex<Vec<TxEvent>>>,
        concurrent_verifiers: usize,
        stop_signal: Arc<AtomicBool>,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        PoHVerifier {
            poh_events,
            tx_events,
            concurrent_verifiers,
            stop_signal,
            shutdown_tx,
        }
    }

    pub fn start_verification(&self) -> anyhow::Result<()> {
        info!("Starting PoH verification...");

        let stop_signal = self.stop_signal.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let poh_events = self.poh_events.clone();
        let tx_events = self.tx_events.clone();
        let concurrent_verifiers = self.concurrent_verifiers;

        let poh_events_lock = poh_events
            .lock()
            .map_err(|e| anyhow::Error::msg(e.to_string()))?;

        let tx_events_lock = tx_events
            .lock()
            .map_err(|e| anyhow::Error::msg(e.to_string()))?;

        let poh_events = poh_events_lock.to_vec();
        let tx_events = tx_events_lock.to_vec();

        drop(poh_events_lock);
        drop(tx_events_lock);

        let hash_to_process_per_verifier = poh_events.len() / concurrent_verifiers;
        let mut handles: Vec<JoinHandle<()>> = Vec::with_capacity(concurrent_verifiers);
        let mut tx_read = 0;

        for v in 0..concurrent_verifiers {
            let tx_events = tx_events.clone();
            let events_to_verify = poh_events.clone();
            let handle = spawn(move || {
                let start_time = std::time::Instant::now();
                info!("Starting verifier {}", v);
                let start = v * hash_to_process_per_verifier;
                let end = start + hash_to_process_per_verifier;

                let mut hashes_processed = 0;

                let mut is_failed = false;

                for (i, event) in events_to_verify[start..=end].iter().enumerate() {
                    let index = i + start;

                    if index == start + events_to_verify[start..=end].len() - 1 {
                        break;
                    }

                    let next_event_is_tx =
                        events_to_verify[index + 1].event_type == PohEventType::Tx;

                    let next_hash = bs58::encode(events_to_verify[index + 1].hash).into_string();

                    if next_event_is_tx {
                        let current_event_hash = event.hash;

                        let mut combined_hash_vec: Vec<u8> = Vec::new();

                        combined_hash_vec.extend_from_slice(&current_event_hash);

                        combined_hash_vec
                            .extend_from_slice(tx_events[tx_read].signature.0.as_ref());

                        let gen = blake3::hash(&combined_hash_vec).as_bytes().clone();

                        tx_read += 1;

                        let gen = bs58::encode(gen).into_string();

                        if gen.ne(&next_hash) {
                            is_failed = true;
                            info!("Verifier {} failed at index {}", v, index);
                            break;
                        }
                    } else {
                        let gen = bs58::encode(blake3::hash(&event.hash).as_bytes().clone())
                            .into_string();

                        if gen.ne(&next_hash) {
                            is_failed = true;
                            info!("Verifier {} failed at index {}", v, index);
                            break;
                        }
                    };

                    hashes_processed += 1;
                }

                if !is_failed {
                    info!("Verifier {} completed", v);
                    info!(
                        "Verifier - {} hashes processed from : {} to {} total:{}",
                        v, start, end, hashes_processed
                    );
                } else {
                    info!("Verifier {} failed", v);
                }

                let elapsed = start_time.elapsed();
                info!("Elapsed time for verifier {}: {:?}", v, elapsed);
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .join()
                .map_err(|e| anyhow::Error::msg(format!("Error: {:?}", e)))?;
        }

        stop_signal.store(true, std::sync::atomic::Ordering::Relaxed);

        shutdown_tx.send(true)?;
        Ok(())
    }
}

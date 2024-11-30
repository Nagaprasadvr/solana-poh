use log::info;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Debug, Display},
    ops::Mul,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Arc, Mutex,
    },
    thread::{spawn, JoinHandle},
    time::Instant,
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

pub struct TxEvent {
    pub signature: TxSig,
    pub signal: TxSignal,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TxSignal {
    Process,
    Stop,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PoHEvent {
    pub index: usize,
    pub tx_sig: TxSig,
}

impl Debug for PoHEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "index: {} tx_sig: {}", self.index, self.tx_sig)
    }
}

impl Display for PoHEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "index: {} tx_sig: {}", self.index, self.tx_sig)
    }
}

pub struct PoHRecorder {
    pub push_hash_interval: u64,
    pub poh_events: Arc<Mutex<Vec<PoHEvent>>>,
    pub poh_tx_rx: channel::Receiver<TxEvent>,
    pub stop_signal: Arc<AtomicBool>,
}

impl PoHRecorder {
    pub fn build(
        push_hash_interval: u64,
        poh_tx_rx: channel::Receiver<TxEvent>,
        stop_signal: Arc<AtomicBool>,
        poh_events: Arc<Mutex<Vec<PoHEvent>>>,
    ) -> Self {
        PoHRecorder {
            poh_tx_rx,
            push_hash_interval,
            poh_events,
            stop_signal,
        }
    }
    pub fn start_recording(&self) -> anyhow::Result<[JoinHandle<Result<(), anyhow::Error>>; 2]> {
        info!("Starting PoH recording...");

        let poh_event_pointer = Arc::new(AtomicUsize::new(0));

        let reciver = self.poh_tx_rx.clone();

        let poh_events = self.poh_events.clone();

        let poh_event_tx_pointer: Arc<AtomicUsize> = poh_event_pointer.clone();

        let tx_events_poh_control = spawn(move || -> anyhow::Result<()> {
            info!("PoH tx events task started...");
            let poh_event_pointer = poh_event_tx_pointer.clone();
            while let Ok(data) = reciver.recv() {
                if data.signal == TxSignal::Stop {
                    info!("PoH tx events task stopped...");
                    break;
                }

                let poh_event = PoHEvent {
                    tx_sig: data.signature,
                    index: poh_event_pointer.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                };

                info!("Received tx event: {:?}", poh_event);
                let mut poh_events = poh_events.lock().unwrap();
                poh_events.push(poh_event);

                println!("PoH events: {:?}", poh_events);
            }

            Ok(())
        });

        let push_interval = self.push_hash_interval;

        let stop_signal = self.stop_signal.clone();

        let poh_hash_control = spawn(move || -> anyhow::Result<()> {
            info!("PoH hash task started...");
            let seed = rand::random::<u64>();
            let mut input = seed.to_string();
            let mut start = Instant::now();
            let stop_signal = stop_signal.clone();
            let mut previous_index_pushed = 0;

            loop {
                if stop_signal.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("PoH hash task stopped...");
                    break;
                }
                let out_hash = blake3::hash(input.as_bytes()).to_string();

                let index = poh_event_pointer.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                if index - previous_index_pushed == push_interval as usize {
                    let duration = start.elapsed().as_micros();
                    PoHRecorder::push_poh_entry(index, &out_hash, duration)?;
                    previous_index_pushed = index;
                    start = Instant::now();
                }

                input = out_hash;
            }
            Ok(())
        });

        Ok([tx_events_poh_control, poh_hash_control])
    }

    pub fn push_poh_entry(index: usize, hash: &String, duration_mcs: u128) -> anyhow::Result<()> {
        let in_seconds = (duration_mcs as f64).mul(10f64.powi(-6));
        let formatted_secs = format!("{:.5}", in_seconds);
        info!(
            "index : {}  PoH hash: {} Duration: {} secs",
            index, hash, formatted_secs
        );
        Ok(())
    }
}

pub struct _PoHVerifier {}

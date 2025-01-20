use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use log::{error, info};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::{
    future::IntoFuture,
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::net::TcpListener;

use crate::poh::{PoHRecorder, TxEvent, TxSig, TxSignal};

pub struct PoHServer {
    pub addr: SocketAddr,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TxPostReq {
    pub tx_sig: String,
}

impl PoHServer {
    pub fn build(addr: String) -> anyhow::Result<Self> {
        Ok(PoHServer {
            addr: SocketAddr::V4(addr.parse()?),
        })
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        env_logger::builder()
            .format_module_path(false)
            .format_timestamp_micros()
            .init();

        info!("Listening on http://{}", self.addr);
        let (tx, rx) = crossbeam::channel::unbounded::<TxEvent>();

        let stop_signal: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let poh_events = Arc::new(Mutex::new(Vec::new()));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let tx_events = Arc::new(Mutex::new(Vec::new()));
        let poh_recorder = PoHRecorder::build(
            64 * 3, //3 slots
            rx,
            shutdown_tx.clone(),
            stop_signal.clone(),
            poh_events.clone(),
            tx.clone(),
            tx_events,
        );

        let controls = poh_recorder.start_recording()?;

        let tx = tx.clone();

        let server_tx = tx.clone();

        let handler = service_fn(move |req: Request<Incoming>| {
            let tx = server_tx.clone();
            let poh_events_clone = poh_events.clone();
            async move {
                let response = match *req.method() {
                    Method::POST => {
                        let handle_req = || async move {
                            let data = req
                                .into_body()
                                .frame()
                                .into_future()
                                .await
                                .ok_or(anyhow::Error::msg("No body"))??
                                .into_data()
                                .map_err(|e| anyhow::Error::msg(format!("Error: {:?}", e)))?;

                            let tx_post_req: TxPostReq = serde_json::from_slice(&data)?;

                            let tx_sig = bs58::decode(tx_post_req.tx_sig)
                                .into_vec()?
                                .as_slice()
                                .try_into()?;

                            tx.send(TxEvent {
                                signature: tx_sig,
                                signal: TxSignal::Process,
                            })?;

                            Ok(())
                        };

                        let res: Result<(), anyhow::Error> = handle_req().await;

                        match res {
                            Ok(()) => Response::builder().body("Tx event recorded".to_string()),
                            Err(e) => {
                                error!("Error: {:?}", e);
                                Response::builder().body(e.to_string())
                            }
                        }
                    }
                    Method::GET => {
                        let data = poh_events_clone.lock().map_or(Vec::new(), |d| d.to_vec());

                        match serde_json::to_string(&data) {
                            Ok(data) => Response::builder().body(data),
                            Err(e) => Response::builder().body(e.to_string()),
                        }
                    }
                    _ => Response::builder().body("Method not allowed!".to_string()),
                };

                response
            }
        });

        let listener = TcpListener::bind(self.addr).await?;

        info!("Listening on http://{}", self.addr);

        loop {
            let mut shutdown_rx = shutdown_rx.clone();
            let mut rng = rand::thread_rng();
            let random: u8 = rng.gen_range(1..10);

            if random > 5 {
                info!("Randomly sending a tx event");
                let mut random_slice = [0u8; 64];
                rand::thread_rng().fill(&mut random_slice);

                tx.send(TxEvent {
                    signature: TxSig(random_slice),
                    signal: TxSignal::Process,
                })?;
            }

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Shutting down server...");
                    for control in controls.into_iter() {
                        control
                            .join()
                            .map_err(|e| anyhow::Error::msg(format!("Error: {:?}", e)))??;
                    }
                    info!("PoH recorder stopped...");
                    break;
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl-C received, shutting down...");
                    stop_signal.store(true, std::sync::atomic::Ordering::Relaxed);
                    tx.send(TxEvent {
                        signature: TxSig::default(),
                        signal: TxSignal::Stop,
                    })?;
                    shutdown_tx.send(true)?;

                }
                Ok((tcp,_)) = listener.accept() => {
                    let handler = handler.clone();
                    let io = TokioIo::new(tcp);

                    tokio::spawn(async move {
                    let mut shutdown_rx = shutdown_rx.clone();
                        tokio::select!{
                            res = http1::Builder::new()
                            .serve_connection(io, handler) => {
                                if let Err(e) = res {
                                    error!("server connection error: {}", e);
                                }
                            }
                            _ = shutdown_rx.changed() => {
                                info!("Shutting down server...");

                            }
                        }
                    });
                }


            }
        }

        Ok(())
    }
}

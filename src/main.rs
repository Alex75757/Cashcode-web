use axum::{
    extract::{ ws::{ Message, WebSocket, WebSocketUpgrade }, State },
    response::IntoResponse,
    routing::{ any, get, post },
    Json,
    Router,
};
use cashcode::{ BillMask, BillTable, CashcodeDevice, DeviceState };
use serde::Serialize;
use std::{ sync::Arc, time::Duration };
use tokio::sync::{ broadcast, mpsc, Mutex };
use tower_http::services::{ ServeDir, ServeFile };
use tracing_subscriber::{ layer::{ SubscriberExt, Layer }, util::SubscriberInitExt, EnvFilter };

// --- События для WebSocket ---
#[derive(Clone, Serialize)]
#[serde(tag = "type", content = "data")]
enum WsEvent {
    Welcome {
        message: String,
    },
    Prompt {
        message: String,
    },
    BillEscrowed {
        bill_type: u8,
    },
    BillStacked {
        bill_type: u8,
        amount_rub: f64,
    },
    BillReturned {
        bill_type: u8,
    },
    Error {
        message: String,
    },
    DeviceReady,
}

// --- Команды от HTTP к драйверу ---
#[derive(Debug)]
enum CashCmd {
    Enable,
    Disable,
    Return,
    Reset,
}

// --- Общее состояние (для /api/status) ---
#[derive(Default, Clone)]
struct CashState {
    ready: bool,
    total_rub: f64,
    bills: Vec<Bill>,
    last_error: Option<String>,
    bill_table: Option<BillTable>,
}

#[derive(Clone, Serialize, Debug)]
struct Bill {
    bill_type: u8,
    amount_rub: f64,
}

#[derive(Serialize)]
struct StatusResponse {
    ready: bool,
    total_rub: f64,
    bills: Vec<Bill>,
    last_error: Option<String>,
    bill_table_loaded: bool,
}

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<WsEvent>,
    cmd_tx: mpsc::Sender<CashCmd>,
    cash_state: Arc<Mutex<CashState>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber
        ::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::new("info,cashcode=info")))
        .init();

    let (tx, _rx) = broadcast::channel(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<CashCmd>(16);
    let cash_state = Arc::new(Mutex::new(CashState::default()));

    let state = AppState {
        tx: tx.clone(),
        cmd_tx: cmd_tx.clone(),
        cash_state: cash_state.clone(),
    };

    let device_state = state.clone();
    tokio::spawn(async move {
        run_validator_loop(device_state, cmd_rx).await;
    });

    let app = Router::new()
        .route("/ws", any(ws_handler))
        .route("/api/status", get(api_status))
        .route("/api/enable", post(api_enable))
        .route("/api/disable", post(api_disable))
        .route("/api/return", post(api_return))
        .route("/api/reset", post(api_reset))
        .with_state(state)
        .fallback_service(
            ServeDir::new("cashcode-ui/dist").not_found_service(
                ServeFile::new("cashcode-ui/dist/index.html")
            )
        );

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

// --- HTTP-эндпоинты ---
async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let s = state.cash_state.lock().await;
    Json(StatusResponse {
        ready: s.ready,
        total_rub: s.total_rub,
        bills: s.bills.clone(),
        last_error: s.last_error.clone(),
        bill_table_loaded: s.bill_table.is_some(),
    })
}

async fn api_enable(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state.cmd_tx.send(CashCmd::Enable).await;
    Json(serde_json::json!({"ok": true}))
}

async fn api_disable(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state.cmd_tx.send(CashCmd::Disable).await;
    Json(serde_json::json!({"ok": true}))
}

async fn api_return(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state.cmd_tx.send(CashCmd::Return).await;
    Json(serde_json::json!({"ok": true}))
}

async fn api_reset(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state.cmd_tx.send(CashCmd::Reset).await;
    Json(serde_json::json!({"ok": true}))
}

// --- WebSocket обработчик ---
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();

    let welcome = WsEvent::Welcome {
        message: "Подключено к купюроприёмнику".to_string(),
    };
    if let Ok(json) = serde_json::to_string(&welcome) {
        let _ = socket.send(Message::Text(json.into())).await;
    }

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if socket.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Text(text))) => tracing::info!("Client says: {}", text),
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// --- Основной цикл работы с устройством + обработка команд ---
async fn run_validator_loop(state: AppState, mut cmd_rx: mpsc::Receiver<CashCmd>) {
    let port_path = std::env::var("CASHCODE_PORT").unwrap_or_else(|_| "COM4".to_string());

    let mut dev = loop {
        match CashcodeDevice::open(&port_path, None, None) {
            Ok(d) => {
                tracing::info!("Device opened on {}", port_path);
                break d;
            }
            Err(e) => {
                tracing::error!("Failed to open device: {}. Retrying in 5s...", e);
                let _ = state.tx.send(WsEvent::Error {
                    message: format!("Не удалось открыть порт: {}", e),
                });
                {
                    let mut s = state.cash_state.lock().await;
                    s.ready = false;
                    s.last_error = Some(format!("open: {}", e));
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };

    if let Err(e) = dev.initialize() {
        tracing::error!("Initialization failed: {}", e);
        let _ = state.tx.send(WsEvent::Error {
            message: format!("Ошибка инициализации: {}", e),
        });
        {
            let mut s = state.cash_state.lock().await;
            s.ready = false;
            s.last_error = Some(format!("initialize: {}", e));
        }
        return;
    }
    tracing::info!("Device initialized successfully");
    
    if let Err(e) = dev.disable() {
        tracing::warn!("Disable after init failed: {}", e);
    } else {
        tracing::info!("Device disabled after init (waiting for cash order)");
    }

    let bill_table = match dev.bill_table() {
        Ok(t) => {
            tracing::info!("Bill table loaded");
            Some(t.clone())
        }
        Err(e) => {
            tracing::error!("Failed to get bill table: {}", e);
            None
        }
    };

    let _ = state.tx.send(WsEvent::DeviceReady);
    let _ = state.tx.send(WsEvent::Prompt {
        message: "Пожалуйста, внесите купюру".to_string(),
    });
    {
        let mut s = state.cash_state.lock().await;
        s.ready = true;
        s.last_error = None;
        s.bill_table = bill_table;
    }

    loop {
        // 1. Команды (неблокирующе)
        match cmd_rx.try_recv() {
            Ok(CashCmd::Enable) => {
                if let Err(e) = dev.enable_bill_types(BillMask::ALL) {
                    tracing::error!("Enable failed: {}", e);
                    let _ = state.tx.send(WsEvent::Error { message: format!("enable: {}", e) });
                } else {
                    tracing::info!("cashcode-web: enabled");
                    let _ = state.tx.send(WsEvent::Prompt {
                        message: "Купюроприёмник готов. Внесите купюру.".to_string(),
                    });
                }
            }
            Ok(CashCmd::Disable) => {
                if let Err(e) = dev.disable() {
                    tracing::error!("Disable failed: {}", e);
                } else {
                    tracing::info!("cashcode-web: disabled");
                }
            }
            Ok(CashCmd::Return) => {
                if let Err(e) = dev.return_bill() {
                    tracing::error!("Return failed: {}", e);
                } else {
                    tracing::info!("cashcode-web: return requested");
                }
            }
            Ok(CashCmd::Reset) => {
                tracing::info!("cashcode-web: reset");
                {
                    let mut s = state.cash_state.lock().await;
                    s.total_rub = 0.0;
                    s.bills.clear();
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                tracing::info!("cmd channel closed, stopping");
                return;
            }
        }

        // 2. Poll
        match dev.poll() {
            Ok(state_event) => {
                match state_event {
                    DeviceState::EscrowPosition { bill_type } => {
                        tracing::info!("Bill in escrow: type {}", bill_type);
                        let _ = state.tx.send(WsEvent::BillEscrowed { bill_type });
                        if let Err(e) = dev.stack() {
                            tracing::error!("Stack failed: {}", e);
                        }
                    }
                    DeviceState::BillStacked { bill_type } => {
                        tracing::info!("Bill stacked: type {}", bill_type);

                        let amount = {
                            let s = state.cash_state.lock().await;
                            match &s.bill_table {
                                Some(table) => bill_amount_from_table(table, bill_type),
                                None => {
                                    tracing::warn!(
                                        "bill table not loaded, amount=0 for type {}",
                                        bill_type
                                    );
                                    0.0
                                }
                            }
                        };

                        let _ = state.tx.send(WsEvent::BillStacked {
                            bill_type,
                            amount_rub: amount,
                        });
                        {
                            let mut s = state.cash_state.lock().await;
                            s.total_rub += amount;
                            s.bills.push(Bill { bill_type, amount_rub: amount });
                        }

                        // ── Ожидание UnitDisabled/Idling через poll ──
                        // После stack устройство само переходит в UnitDisabled (красный).
                        // Ждём это состояние через poll (ACK продвигает state machine),
                        // затем enable — это штатный путь, без RESET.
                        tracing::info!("Bill stacked, waiting for UnitDisabled before re-enable");

                        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                        loop {
                            match dev.poll() {
                                Ok(DeviceState::UnitDisabled) | Ok(DeviceState::Idling) => {
                                    tracing::info!("Device ready for re-enable");
                                    break;
                                }
                                Ok(other) => {
                                    tracing::debug!(?other, "poll after stack");
                                }
                                Err(e) => {
                                    tracing::error!("poll after stack failed: {}", e);
                                    break;
                                }
                            }
                            if tokio::time::Instant::now() > deadline {
                                tracing::warn!("timeout waiting for UnitDisabled after stack");
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }

                        // Включаем приём (штатный путь после stack, без RESET)
                        if let Err(e) = dev.enable_bill_types(BillMask::ALL) {
                            tracing::error!("Enable after stack failed: {}", e);
                        } else {
                            tracing::info!("Device re-enabled after stack (should be green)");
                        }

                        let _ = state.tx.send(WsEvent::Prompt {
                            message: "Купюра принята. Внесите следующую.".to_string(),
                        });
                    }
                    DeviceState::BillReturned { bill_type } => {
                        tracing::info!("Bill returned: type {}", bill_type);
                        let _ = state.tx.send(WsEvent::BillReturned { bill_type });
                        let _ = state.tx.send(WsEvent::Prompt {
                            message: "Купюра возвращена. Внесите другую.".to_string(),
                        });
                    }
                    _ => {}
                }
            }
            Err(e) => {
                tracing::error!("Poll error: {}", e);
                let _ = state.tx.send(WsEvent::Error {
                    message: format!("Ошибка связи: {}", e),
                });
                {
                    let mut s = state.cash_state.lock().await;
                    s.last_error = Some(format!("poll: {}", e));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Err(e) = dev.initialize() {
                    tracing::error!("Re-init failed: {}", e);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Номинал купюры из реальной таблицы устройства.
fn bill_amount_from_table(table: &BillTable, bill_type: u8) -> f64 {
    match table.get(bill_type) {
        Some(entry) => entry.denomination as f64,
        None => {
            tracing::warn!("bill_type {} not found in table", bill_type);
            0.0
        }
    }
}

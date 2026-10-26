use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::ConnectInfo;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StatsPayload {
    #[serde(rename = "type")]
    msg_type: String,
    beta_readers: usize,
    global_online: usize,
}

struct State {
    beta_readers: Arc<AtomicUsize>,
    global_online: Arc<AtomicUsize>,
    tx: broadcast::Sender<StatsPayload>,
}

#[tokio::main]
async fn main() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let (tx, _) = broadcast::channel(128);
    let state = Arc::new(State {
        beta_readers: Arc::new(AtomicUsize::new(0)),
        global_online: Arc::new(AtomicUsize::new(1)),
        tx: tx.clone(),
    });

    // Фонова задача: опитування локального бекенду Release (8080) для отримання загального онлайну
    let state_poll = state.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();

        let mut last_global = 0;
        loop {
            if let Ok(resp) = client.get("http://127.0.0.1:8080/api/health").send().await {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    let online = val.get("online_users")
                        .and_then(|u| u.as_u64())
                        .unwrap_or(1) as usize;

                    state_poll.global_online.store(online, Ordering::Relaxed);
                    if online != last_global {
                        last_global = online;
                        let payload = StatsPayload {
                            msg_type: "stats".to_string(),
                            beta_readers: state_poll.beta_readers.load(Ordering::Relaxed),
                            global_online: online,
                        };
                        let _ = state_poll.tx.send(payload);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });

    let app = Router::new()
        .route("/api/beta/readers", get(get_readers))
        .route("/ws/beta/readers", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = "127.0.0.1:8081".parse().unwrap();
    info!("Beta realtime reader counter daemon listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn get_readers(axum::extract::State(state): axum::extract::State<Arc<State>>) -> impl IntoResponse {
    let readers = state.beta_readers.load(Ordering::Relaxed);
    let global = state.global_online.load(Ordering::Relaxed);
    Json(json!({
        "status": "ok",
        "beta_readers": readers,
        "global_online": global
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    axum::extract::State(state): axum::extract::State<Arc<State>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let client_ip = headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-real-ip"))
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| addr.ip().to_string());

    ws.on_upgrade(move |socket| handle_socket(socket, state, client_ip, user_agent))
}

async fn handle_socket(
    mut socket: WebSocket,
    state: Arc<State>,
    client_ip: String,
    user_agent: String,
) {
    let current_readers = state.beta_readers.fetch_add(1, Ordering::Relaxed) + 1;
    let global = state.global_online.load(Ordering::Relaxed);

    info!(
        "[READER +] Client connected from IP: {client_ip} | UA: {user_agent} | Active readers: {current_readers}"
    );

    let initial_payload = StatsPayload {
        msg_type: "stats".to_string(),
        beta_readers: current_readers,
        global_online: global,
    };
    let _ = state.tx.send(initial_payload.clone());

    if let Ok(msg_text) = serde_json::to_string(&initial_payload) {
        let _ = socket.send(Message::Text(msg_text.into())).await;
    }

    let mut rx = state.tx.subscribe();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(15));

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = socket.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Text(text))) => {
                        if text.trim() == "ping" {
                            let _ = socket.send(Message::Text("pong".to_string().into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    _ => {}
                }
            }
            _ = ping_interval.tick() => {
                // Heartbeat ping для підтримки з'єднання через Cloudflare
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            Ok(payload) = rx.recv() => {
                if let Ok(update_text) = serde_json::to_string(&payload) {
                    if socket.send(Message::Text(update_text.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    let remaining_readers = state.beta_readers.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
    let global_now = state.global_online.load(Ordering::Relaxed);

    info!(
        "[READER -] Client disconnected: {client_ip} | Active readers: {remaining_readers}"
    );

    let leave_payload = StatsPayload {
        msg_type: "stats".to_string(),
        beta_readers: remaining_readers,
        global_online: global_now,
    };
    let _ = state.tx.send(leave_payload);
}

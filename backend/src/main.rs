mod accuracy;
mod approach;
mod borders;
mod cluster;
mod config;
mod dedup;
mod delay;
mod geo;
mod launches;
mod load;
mod models;
mod monitor;
mod parsers;
mod target_tracker;
pub mod track_storage;
mod tracker;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use axum::{
    extract::{
        ws::{Message, WebSocketUpgrade},
        Query, State,
    },
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::warn;

use config::Config;
use cluster::cluster_threats;
use delay::DelayBuffer;
use models::{in_theater, HealthResponse, SourceInfo};
use tracker::{now_secs, Tracker};

const VERSION: &str = "0.1.0-refresh";

struct AppState {
    cfg: Config,
    buffer: DelayBuffer,
    tracker: tokio::sync::Mutex<Tracker>,
    target_tracker: tokio::sync::Mutex<target_tracker::TargetTracker>,
    monitor: tokio::sync::Mutex<monitor::LaunchMonitor>,
    alerts: RwLock<parsers::alerts::Response>,
    bounds: RwLock<parsers::alerts::Bounds>,
    official: RwLock<Vec<parsers::official::Notice>>,
    last_fetch_ok: RwLock<bool>,
    last_error: RwLock<Option<String>>,
    filtered_out: RwLock<usize>,
    deduped: RwLock<usize>,
    accuracy: Arc<accuracy::AccuracyEngine>,
    online_clients: Arc<AtomicUsize>,
    online_tx: broadcast::Sender<usize>,
    poll_tx: broadcast::Sender<()>,
    pub global_cooldown: Arc<AtomicU64>,
    cooldown_tx: broadcast::Sender<u64>,
}

/// Свіжість офіційних сповіщень у видачі, с (півтори години)
const OFFICIAL_TTL_SEC: f64 = 5400.0;

async fn fresh_official(s: &AppState) -> Vec<parsers::official::Notice> {
    let now = tracker::now_secs();
    s.official
        .read()
        .await
        .iter()
        .filter(|n| now - n.observed_at.timestamp() as f64 <= OFFICIAL_TTL_SEC)
        .take(12)
        .cloned()
        .collect()
}

/// Один повний знімок для карти: живі цілі + пуски + тривоги.
async fn build_geo(s: &AppState) -> serde_json::Value {
    let (delayed_threats, clusters, age) = s.buffer.get_delayed().await;
    let clustered_ids: HashSet<String> = clusters
        .iter()
        .filter(|c| c.sources.len() > 1 || c.members.len() > 1)
        .flat_map(|c| c.members.iter().map(|m| m.id.clone()))
        .collect();
    let alerts_guard = s.alerts.read().await;
    let bounds_guard = s.bounds.read().await;
    let sites = launches::all_sites();
    let corridors = approach::build_corridors(&sites);
    let now = tracker::now_secs();
    // Події: евристика (за замовчуванням ВИМКНЕНА) + офіційні пости
    // з впізнаним носієм (літак/корабель/аеродром). Пости без носія —
    // лише маркери шару `official`, подій/стрілок з них немає.
    let mut events = if s.cfg.enable_heuristic {
        s.monitor.lock().await.active(now)
    } else {
        Vec::new()
    };
    let fresh_official = fresh_official(s).await;
    for e in monitor::official_events(&sites, &fresh_official, now) {
        // без дублів: евристика з того ж вікна вже могла спрацювати
        let dup = events.iter().any(|x| {
            x.launch_id == e.launch_id && (x.first_seen - e.first_seen).abs() < 600.0
        });
        if !dup {
            events.push(e);
        }
    }
    let anomalies = s.tracker.lock().await.anomalies;
    let mut uav_trails = s.target_tracker.lock().await.uav_trails_geojson(now, false);
    snap_trails_to_clusters(&mut uav_trails, &clusters);
    let lost_targets = s.target_tracker.lock().await.lost_points_geojson(now, false);
    json!({
        "server_time": Utc::now().to_rfc3339(),
        "delay_sec": s.cfg.public_delay_sec,
        "poll_interval_sec": s.cfg.poll_interval_sec,
        "cooldown_sec": s.global_cooldown.load(Ordering::Relaxed),
        "data_age_sec": age,
        "anomalies": anomalies,
        "online_count": s.online_clients.load(Ordering::Relaxed).max(1),
        "targets": geo::consolidated_targets_collection(&clusters, &s.cfg),
        "threats": geo::threats_collection(&delayed_threats, &clustered_ids, &s.cfg),
        "clusters": geo::clusters_collection(&clusters, &s.cfg),
        "trails": uav_trails,
        "lost_targets": lost_targets,
        "vectors": geo::consolidated_vectors_collection(&clusters, &s.cfg),
        "raw_trails": geo::trails_collection(&delayed_threats, &clustered_ids, &s.cfg),
        "raw_vectors": geo::vectors_collection(&delayed_threats, &clustered_ids, &s.cfg),
        "alerts": parsers::alerts::alerts_collection(&alerts_guard, &bounds_guard),
        "launches": geo::launches_collection(&sites),
        "corridors": geo::corridors_collection(&corridors, &sites),
        "launch_events": geo::launch_events_collection(&events, now),
        "official": geo::official_collection(&fresh_official, now),
    })
}

/// Окремий знімок для BETA: виключно офіційні дані (UkraineAlarm / ПС ЗСУ).
async fn build_beta_geo(s: &AppState) -> serde_json::Value {
    let (delayed_threats, raw_clusters, age) = s.buffer.get_delayed().await;

    // Фільтруємо суто офіційні цілі для BETA: ukrainealarm + official
    let mut beta_clusters: Vec<crate::models::Cluster> = Vec::new();
    for mut c in raw_clusters {
        c.members.retain(|m| m.source == "ukrainealarm" || m.source == "official");
        if c.members.is_empty() {
            continue;
        }
        c.sources = vec!["ukrainealarm".to_string()];
        c.confirmed = true;
        c.target_count = 1;
        c.agreement = "Офіційні дані ПС ЗСУ / UkraineAlarm".to_string();
        beta_clusters.push(c);
    }

    let beta_threats: Vec<crate::models::Threat> = delayed_threats
        .into_iter()
        .filter(|t| t.source == "ukrainealarm" || t.source == "official")
        .collect();

    let clustered_ids: HashSet<String> = beta_clusters
        .iter()
        .flat_map(|c| c.members.iter().map(|m| m.id.clone()))
        .collect();

    let alerts_guard = s.alerts.read().await;
    let bounds_guard = s.bounds.read().await;
    let sites = launches::all_sites();
    let corridors = approach::build_corridors(&sites);
    let now = tracker::now_secs();

    let fresh_official = fresh_official(s).await;
    let events = monitor::official_events(&sites, &fresh_official, now);
    let anomalies = 0;
    let mut uav_trails = s.target_tracker.lock().await.uav_trails_geojson(now, true);
    snap_trails_to_clusters(&mut uav_trails, &beta_clusters);
    let lost_targets = s.target_tracker.lock().await.lost_points_geojson(now, true);

    json!({
        "server_time": Utc::now().to_rfc3339(),
        "delay_sec": s.cfg.public_delay_sec,
        "poll_interval_sec": s.cfg.poll_interval_sec,
        "cooldown_sec": s.global_cooldown.load(Ordering::Relaxed),
        "data_age_sec": age,
        "anomalies": anomalies,
        "online_count": s.online_clients.load(Ordering::Relaxed).max(1),
        "targets": geo::consolidated_targets_collection(&beta_clusters, &s.cfg),
        "threats": geo::threats_collection(&beta_threats, &clustered_ids, &s.cfg),
        "clusters": geo::clusters_collection(&beta_clusters, &s.cfg),
        "trails": uav_trails,
        "lost_targets": lost_targets,
        "vectors": json!({ "type": "FeatureCollection", "features": [] }),
        "raw_trails": json!({ "type": "FeatureCollection", "features": [] }),
        "raw_vectors": json!({ "type": "FeatureCollection", "features": [] }),
        "alerts": parsers::alerts::alerts_collection(&alerts_guard, &bounds_guard),
        "launches": geo::launches_collection(&sites),
        "corridors": geo::corridors_collection(&corridors, &sites),
        "launch_events": geo::launch_events_collection(&events, now),
        "official": geo::official_collection(&fresh_official, now),
        "mode": "official_only",
    })
}

/// Притягує кінчик активного хвоста точно до координат відповідного кластера цілі (нульовий розрив між маркером і лінією)
fn snap_trails_to_clusters(trails: &mut serde_json::Value, clusters: &[crate::models::Cluster]) {
    if let Some(feats) = trails.get_mut("features").and_then(|f| f.as_array_mut()) {
        let cluster_map: std::collections::HashMap<&str, [f64; 2]> = clusters
            .iter()
            .map(|c| (c.id.as_str(), [c.lon, c.lat]))
            .collect();

        for feat in feats {
            let is_active = feat.get("properties")
                .and_then(|p| p.get("active"))
                .and_then(|a| a.as_bool())
                .unwrap_or(false);
            if !is_active {
                continue;
            }
            if let Some(id) = feat.get("properties").and_then(|p| p.get("id")).and_then(|i| i.as_str()) {
                if let Some(&[c_lon, c_lat]) = cluster_map.get(id) {
                    if let Some(coords) = feat.get_mut("geometry").and_then(|g| g.get_mut("coordinates")).and_then(|c| c.as_array_mut()) {
                        let should_push = match coords.last().and_then(|p| p.as_array()) {
                            Some(last) if last.len() >= 2 => {
                                let lo = last[0].as_f64().unwrap_or(0.0);
                                let la = last[1].as_f64().unwrap_or(0.0);
                                (lo - c_lon).hypot(la - c_lat) > 0.00005
                            }
                            _ => true,
                        };
                        if should_push {
                            coords.push(serde_json::json!([c_lon, c_lat]));
                        } else if let Some(last) = coords.last_mut() {
                            *last = serde_json::json!([c_lon, c_lat]);
                        }
                    }
                }
            }
        }
    }
}

async fn poll_once(state: &Arc<AppState>, last_official: &mut f64) {
    let (neptun_res, dimap_res, shahmap_res, radarua_res, aviacontrol_res, kupol_res, eradar_res, ukrainealarm_res) = tokio::join!(
        parsers::neptun::fetch(&state.cfg.neptun_base_url),
        parsers::dimap::fetch(&state.cfg.dimap_base_url, state.cfg.enable_dimap),
        parsers::shahmap::fetch(&state.cfg.shahmap_base_url, state.cfg.enable_shahmap),
        parsers::radarua::fetch(&state.cfg.radarua_base_url, state.cfg.enable_radarua),
        parsers::aviacontrol::fetch(&state.cfg.aviacontrol_base_url, state.cfg.enable_aviacontrol),
        parsers::kupol::fetch(&state.cfg.kupol_base_url, state.cfg.enable_kupol),
        parsers::eradar::fetch(&state.cfg.eradar_base_url, state.cfg.enable_eradar),
        parsers::ukrainealarm::fetch(&state.cfg.ukrainealarm_base_url, state.cfg.enable_ukrainealarm)
    );
    let mut incoming = vec![];
    let mut errors = vec![];
    match neptun_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("neptun fetch failed: {e}");
            errors.push(format!("neptun: {e}"));
        }
    }
    match dimap_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("dimap fetch failed: {e}");
            errors.push(format!("dimap: {e}"));
        }
    }
    match shahmap_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("shahmap fetch failed: {e}");
            errors.push(format!("shahmap: {e}"));
        }
    }
    match radarua_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("radarua fetch failed: {e}");
            errors.push(format!("radarua: {e}"));
        }
    }
    match aviacontrol_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("aviacontrol fetch failed: {e}");
            errors.push(format!("aviacontrol: {e}"));
        }
    }
    match kupol_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("kupol fetch failed: {e}");
            errors.push(format!("kupol: {e}"));
        }
    }
    match eradar_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("eradar fetch failed: {e}");
            errors.push(format!("eradar: {e}"));
        }
    }
    match ukrainealarm_res {
        Ok(t) => incoming.extend(t),
        Err(e) => {
            warn!("ukrainealarm fetch failed: {e}");
            errors.push(format!("ukrainealarm: {e}"));
        }
    }
    // Офіційні сповіщення про пуски (швидкий моніторинг ПС ЗСУ, за замовчуванням 10с)
    if state.cfg.enable_official && now_secs() - *last_official >= state.cfg.official_poll_interval_sec {
        *last_official = now_secs();
        match parsers::official::fetch(&state.cfg.official_channel_list()).await {
            Ok(n) => {
                if !n.is_empty() {
                    tracing::info!("official: {} notices", n.len());
                }
                *state.official.write().await = n;
            }
            Err(e) => {
                warn!("official fetch failed: {e}");
                errors.push(format!("official: {e}"));
            }
        }
    }
    // Тривоги: межі тримаємо в кеші (оновлення раз на 6 год), статуси — щоциклу
    {
        let need_bounds = {
            let b = state.bounds.read().await;
            b.raions.is_empty() || now_secs() - b.loaded_at > 21600.0
        };
        if need_bounds {
            match parsers::alerts::fetch_bounds(&state.cfg.neptun_base_url).await {
                Ok(b) => {
                    tracing::info!(
                        "bounds loaded: {} raions, {} oblasts",
                        b.raions.len(),
                        b.oblasts.len()
                    );
                    *state.bounds.write().await = b;
                }
                Err(e) => warn!("bounds fetch failed: {e}"),
            }
        }
        match parsers::alerts::fetch(&state.cfg.ukrainealarm_base_url, &state.cfg.neptun_base_url).await {
            Ok(a) => {
                state.target_tracker.lock().await.record_alert_events(&a, now_secs() as i64);
                *state.alerts.write().await = a;
            }
            Err(e) => {
                warn!("alerts fetch failed: {e}");
                errors.push(format!("alerts: {e}"));
            }
        }
    }
    if incoming.is_empty() && !errors.is_empty() {
        *state.last_fetch_ok.write().await = false;
        *state.last_error.write().await = Some(errors.join("; "));
        return;
    }
    // Рубіж 1: грубий bbox-фільтр театру
    let before = incoming.len();
    incoming.retain(|t| in_theater(t.lat, t.lon));
    let dropped = before - incoming.len();
    if dropped > 0 {
        warn!("filtered {dropped} out-of-theater threats");
    }
    *state.filtered_out.write().await = dropped;
    // Рубіж 1.5: дедуп (фрагменти треків + точні клони одного джерела)
    let (incoming, dedup_dropped) = dedup::dedup_threats(incoming);
    if dedup_dropped > 0 {
        warn!("deduped {dedup_dropped} duplicate threats");
    }
    *state.deduped.write().await = dedup_dropped;
    // Рубіж 2: трекер (speed-gate, trail, expire)
    let tracked = state.tracker.lock().await.update(incoming, now_secs());
    // Консенсус-кластеринг
    let mut clusters = cluster_threats(&tracked);
    // Рубіж 3: супровід цілей та присвоєння стабільних TACMAP ідентифікаторів (uav_001, uav_002...)
    let now = now_secs();
    {
        let mut tt = state.target_tracker.lock().await;
        tt.update(&mut clusters, now);
        tt.record_raw_threats(&tracked, now);
    }
    // Оцінка точності карт (журнал засічок по шахедах/реактивах зі штрафом за БПЛА без тривоги та звіркою з ПС ЗСУ)
    {
        let alerts = state.alerts.read().await;
        let bounds = state.bounds.read().await;
        let official = state.official.read().await;
        state.accuracy.record_clusters(&clusters, &alerts, &bounds, now_secs());
        state.accuracy.record_official_notices(&official, &tracked, now_secs());
    }
    if state.cfg.enable_heuristic {
        let sites = launches::all_sites();
        let fresh = state.monitor.lock().await.observe(&tracked, &sites, now_secs());
        if fresh > 0 {
            tracing::warn!("launch event: {fresh} new");
        }
    }
    state.buffer.push(tracked, clusters).await;
    *state.last_fetch_ok.write().await = true;
    *state.last_error.write().await = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };
    let _ = state.poll_tx.send(());
}

async fn poll_loop(state: Arc<AppState>, mut last_official: f64) {
    let interval_sec = state.cfg.poll_interval_sec.max(1);
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Одразу виконуємо перший збір даних у фоновому режимі без затримки старту HTTP сервера
    tracing::info!("Старт глобального циклу: негайний первинний збір даних...");
    poll_once(&state, &mut last_official).await;
    tracing::info!("Первинний збір завершено, глобальний цикл активний!");

    let mut remaining = interval_sec;
    state.global_cooldown.store(remaining, Ordering::Relaxed);
    let _ = state.cooldown_tx.send(remaining);

    loop {
        ticker.tick().await;

        if remaining > 0 {
            remaining -= 1;
        }

        state.global_cooldown.store(remaining, Ordering::Relaxed);
        let _ = state.cooldown_tx.send(remaining);

        if remaining == 0 {
            poll_once(&state, &mut last_official).await;
            remaining = interval_sec;
            state.global_cooldown.store(remaining, Ordering::Relaxed);
            let _ = state.cooldown_tx.send(remaining);
        }
    }
}

async fn health(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let tr = s.tracker.lock().await;
    let now = tracker::now_secs();
    let mut active_launches = if s.cfg.enable_heuristic {
        s.monitor.lock().await.active(now).len()
    } else {
        0
    };
    active_launches +=
        monitor::official_events(&launches::all_sites(), &fresh_official(&s).await, now).len();
    let official_notices = s.official.read().await.len();
    Json(HealthResponse {
        status: "ok".to_string(),
        version: VERSION.to_string(),
        delay_sec: s.cfg.public_delay_sec,
        buffer_len: s.buffer.len().await,
        last_fetch_ok: *s.last_fetch_ok.read().await,
        last_error: s.last_error.read().await.clone(),
        filtered_out: *s.filtered_out.read().await,
        deduped: *s.deduped.read().await,
        tracks: tr.len(),
        anomalies: tr.anomalies,
        active_launches,
        official_notices,
        online_users: s.online_clients.load(Ordering::Relaxed).max(1),
    })
}

async fn sources(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json(vec![
        SourceInfo {
            name: "neptun".to_string(),
            color: s.cfg.color_neptun.clone(),
            enabled: true,
            reason: None,
        },
        SourceInfo {
            name: "dimap".to_string(),
            color: s.cfg.color_dimap.clone(),
            enabled: s.cfg.enable_dimap,
            reason: if s.cfg.enable_dimap {
                Some("reverse-engineered /api/targets, потрібна атрибуція".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_DIMAP".to_string())
            },
        },
        SourceInfo {
            name: "shahmap".to_string(),
            color: s.cfg.color_shahmap.clone(),
            enabled: s.cfg.enable_shahmap,
            reason: if s.cfg.enable_shahmap {
                Some("відкритий /api/targets shahmap.online".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_SHAHMAP".to_string())
            },
        },
        SourceInfo {
            name: "radarua".to_string(),
            color: s.cfg.color_radarua.clone(),
            enabled: s.cfg.enable_radarua,
            reason: if s.cfg.enable_radarua {
                Some("відкритий /api/tracks radarua.com".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_RADARUA".to_string())
            },
        },
        SourceInfo {
            name: "aviacontrol".to_string(),
            color: s.cfg.color_aviacontrol.clone(),
            enabled: s.cfg.enable_aviacontrol,
            reason: if s.cfg.enable_aviacontrol {
                Some("відкритий /api/targets aviacontrol.com.ua".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_AVIACONTROL".to_string())
            },
        },
        SourceInfo {
            name: "kupol".to_string(),
            color: s.cfg.color_kupol.clone(),
            enabled: s.cfg.enable_kupol,
            reason: if s.cfg.enable_kupol {
                Some("відкритий /api/threats/active kupol.in.ua".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_KUPOL".to_string())
            },
        },
        SourceInfo {
            name: "eradar".to_string(),
            color: s.cfg.color_eradar.clone(),
            enabled: s.cfg.enable_eradar,
            reason: if s.cfg.enable_eradar {
                Some("відкритий /api/dangers eradar.app".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_ERADAR".to_string())
            },
        },
        SourceInfo {
            name: "ukrainealarm".to_string(),
            color: s.cfg.color_ukrainealarm.clone(),
            enabled: s.cfg.enable_ukrainealarm,
            reason: if s.cfg.enable_ukrainealarm {
                Some("Офіційна карта «Повітряна тривога» (Ajax Systems / Мінцифра / ПС ЗСУ)".to_string())
            } else {
                Some("Вимкнено флагом ENABLE_UKRAINEALARM".to_string())
            },
        },
        SourceInfo {
            name: "official".to_string(),
            color: "#7dff9a".to_string(),
            enabled: s.cfg.enable_official,
            reason: if s.cfg.enable_official {
                Some(format!(
                    "t.me/s превʼю ПС ЗСУ без ключів ({})",
                    s.cfg.official_channel_list().join(", ")
                ))
            } else {
                Some("Вимкнено флагом ENABLE_OFFICIAL".to_string())
            },
        },
    ])
}

async fn geo(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json(build_geo(&s).await)
}

async fn beta_geo(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json(build_beta_geo(&s).await)
}

async fn bounds(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    // Статичні межі областей (фронт тягне один раз): сітка + контур країни
    let b = s.bounds.read().await;
    let features: Vec<serde_json::Value> = b
        .oblasts
        .values()
        .map(|geom| {
            json!({
                "type": "Feature",
                "geometry": geom,
                "properties": {},
            })
        })
        .collect();
    Json(json!({"type": "FeatureCollection", "features": features}))
}

async fn launches() -> impl IntoResponse {
    // Довідковий шар, без затримки і трекера: фронт може тягнути окремо
    Json(geo::launches_collection(&launches::all_sites()))
}

async fn launch_events(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let now = tracker::now_secs();
    let events = s.monitor.lock().await.active(now);
    Json(geo::launch_events_collection(&events, now))
}

async fn official(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let now = tracker::now_secs();
    let notices: Vec<parsers::official::Notice> = s
        .official
        .read()
        .await
        .iter()
        .filter(|n| now - n.observed_at.timestamp() as f64 <= OFFICIAL_TTL_SEC)
        .take(12)
        .cloned()
        .collect();
    Json(geo::official_collection(&notices, now))
}

async fn outliers(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    // OPSEC: тільки лічильники, жодних координат/зразків треків
    let (delayed, _, _) = s.buffer.get_delayed().await;
    let bad = delayed.iter().filter(|t| !in_theater(t.lat, t.lon)).count();
    Json(json!({
        "count": bad,
        "tracks": s.tracker.lock().await.len(),
    }))
}

struct ActiveClientGuard {
    counter: Arc<AtomicUsize>,
    tx: broadcast::Sender<usize>,
}

impl Drop for ActiveClientGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
        let count = self.counter.load(Ordering::Relaxed);
        let _ = self.tx.send(count);
    }
}

async fn ws_geo(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        s.online_clients.fetch_add(1, Ordering::Relaxed);
        let current = s.online_clients.load(Ordering::Relaxed);
        let _ = s.online_tx.send(current);

        let _guard = ActiveClientGuard {
            counter: s.online_clients.clone(),
            tx: s.online_tx.clone(),
        };

        let mut online_rx = s.online_tx.subscribe();
        let mut poll_rx = s.poll_tx.subscribe();
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));

        // Відправляємо перший повний знімок одразу при підключенні
        let initial = serde_json::to_string(&build_geo(&s).await).unwrap_or_default();
        if !initial.is_empty() {
            if socket.send(Message::Text(initial)).await.is_err() {
                return;
            }
        }

        loop {
            tokio::select! {
                recv_res = poll_rx.recv() => {
                    match recv_res {
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            let txt = serde_json::to_string(&build_geo(&s).await).unwrap_or_default();
                            if !txt.is_empty() {
                                if socket.send(Message::Text(txt)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    // Легкий ping для запобігання Cloudflare 524 timeouts та збереження з'єднання
                    if socket.send(Message::Ping(vec![])).await.is_err() {
                        break;
                    }
                }
                Ok(new_count) = online_rx.recv() => {
                    let msg = json!({
                        "type": "online",
                        "online_count": new_count.max(1)
                    }).to_string();
                    if socket.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(Message::Ping(p))) => {
                            if socket.send(Message::Pong(p)).await.is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    })
}

async fn ws_beta_geo(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        s.online_clients.fetch_add(1, Ordering::Relaxed);
        let current = s.online_clients.load(Ordering::Relaxed);
        let _ = s.online_tx.send(current);

        let _guard = ActiveClientGuard {
            counter: s.online_clients.clone(),
            tx: s.online_tx.clone(),
        };

        let mut online_rx = s.online_tx.subscribe();
        let mut poll_rx = s.poll_tx.subscribe();
        let mut cooldown_rx = s.cooldown_tx.subscribe();
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));

        let initial = serde_json::to_string(&build_beta_geo(&s).await).unwrap_or_default();
        if !initial.is_empty() {
            if socket.send(Message::Text(initial)).await.is_err() {
                return;
            }
        }

        // Відправляємо поточний глобальний кулдаун одразу при з'єднанні
        let cd = s.global_cooldown.load(Ordering::Relaxed);
        let cd_msg = json!({
            "type": "cooldown",
            "remaining": cd,
            "poll_interval": s.cfg.poll_interval_sec
        }).to_string();
        let _ = socket.send(Message::Text(cd_msg)).await;

        loop {
            tokio::select! {
                recv_cd = cooldown_rx.recv() => {
                    match recv_cd {
                        Ok(remaining) => {
                            let msg = json!({
                                "type": "cooldown",
                                "remaining": remaining,
                                "poll_interval": s.cfg.poll_interval_sec
                            }).to_string();
                            if socket.send(Message::Text(msg)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                recv_res = poll_rx.recv() => {
                    match recv_res {
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            let txt = serde_json::to_string(&build_beta_geo(&s).await).unwrap_or_default();
                            if !txt.is_empty() {
                                if socket.send(Message::Text(txt)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    // Легкий ping для запобігання Cloudflare 524 timeouts та збереження з'єднання
                    if socket.send(Message::Ping(vec![])).await.is_err() {
                        break;
                    }
                }
                Ok(new_count) = online_rx.recv() => {
                    let msg = json!({
                        "type": "online",
                        "online_count": new_count.max(1)
                    }).to_string();
                    if socket.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(Message::Ping(p))) => {
                            if socket.send(Message::Pong(p)).await.is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    })
}

async fn redirect_live() -> axum::response::Redirect {
    axum::response::Redirect::permanent("/")
}

async fn set_smart_cache(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        axum::http::header::HeaderName::from_static("x-content-type-options"),
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("x-frame-options"),
        axum::http::HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("referrer-policy"),
        axum::http::HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("permissions-policy"),
        axum::http::HeaderValue::from_static("geolocation=(), camera=(), microphone=()"),
    );

    if path == "/api/geo" || path == "/api/beta/geo" {
        // Дозволяємо Cloudflare Edge кешувати геодані на 3 секунди
        // та віддавати stale під час оновлення 5 секунд (знижує навантаження на origin на 90%)
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=3, stale-while-revalidate=5"),
        );
    } else if path == "/api/bounds" {
        // Статичні межі районів — кешуємо на 1 добу
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=86400, immutable"),
        );
    } else if path == "/api/sources" || path == "/api/launches" {
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=60"),
        );
    } else if path.starts_with("/api/stats/") {
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=15"),
        );
    } else if path.ends_with(".css") || path.ends_with(".js") || path.ends_with(".png") || path.ends_with(".svg") || path.ends_with(".ico") {
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=86400"),
        );
    } else {
        // Для HTML та динамічних запитів (consent, health, тощо)
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache, must-revalidate"),
        );
    }

    response
}

#[derive(Debug, Deserialize)]
struct AccuracyQuery {
    period: Option<String>,
    #[serde(rename = "type")]
    threat_type: Option<String>,
}

async fn accuracy_stats(
    State(s): State<Arc<AppState>>,
    Query(q): Query<AccuracyQuery>,
) -> impl IntoResponse {
    let period = q.period.as_deref().unwrap_or("1h");
    let t_type = q.threat_type.as_deref().unwrap_or("all");
    match s.accuracy.calculate_report(period, t_type, tracker::now_secs()) {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e})),
        )
            .into_response(),
    }
}

async fn accuracy_methodology() -> impl IntoResponse {
    Json(accuracy::AccuracyEngine::methodology())
}

#[derive(Debug, Deserialize)]
struct DatasetQueryParams {
    since: Option<i64>,
    #[serde(rename = "type")]
    threat_type: Option<String>,
    limit: Option<usize>,
}

async fn dataset_tracks(
    State(s): State<Arc<AppState>>,
    Query(q): Query<DatasetQueryParams>,
) -> impl IntoResponse {
    let tt = s.target_tracker.lock().await;
    let data = tt.export_dataset(q.since, q.threat_type.as_deref(), q.limit);
    Json(data)
}

async fn dataset_stats(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let tt = s.target_tracker.lock().await;
    let data = tt.dataset_stats();
    Json(data)
}

#[derive(Debug, Deserialize)]
pub struct ConsentRequest {
    pub consent_id: Option<String>,
    pub public_key: String,
    pub signature: String,
    pub statement: String,
    pub version: Option<String>,
    pub client_timestamp: Option<i64>,
}

async fn handle_consent(
    State(s): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ConsentRequest>,
) -> impl IntoResponse {
    let consent_id = payload.consent_id.unwrap_or_else(|| {
        let prefix = if payload.public_key.len() >= 12 {
            &payload.public_key[..12]
        } else {
            "key"
        };
        format!("consent_{}_{}", Utc::now().timestamp(), prefix)
    });
    let version = payload.version.unwrap_or_else(|| "1.0".to_string());
    let accepted_at = payload.client_timestamp.unwrap_or_else(|| Utc::now().timestamp());

    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let ip = headers.get("cf-connecting-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok());

    let tt = s.target_tracker.lock().await;
    match tt.record_consent(
        &consent_id,
        &payload.public_key,
        &payload.signature,
        &payload.statement,
        user_agent,
        ip,
        accepted_at,
        &version,
    ) {
        Ok(_) => {
            tracing::info!("User consent cryptographically recorded in DB: id={}, version={}", consent_id, version);
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "status": "recorded",
                    "consent_id": consent_id,
                    "accepted_at": accepted_at,
                    "version": version
                })),
            )
        }
        Err(e) => {
            tracing::error!("Failed to record user consent: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "error": e
                })),
            )
        }
    }
}

async fn check_consent(
    State(s): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let tt = s.target_tracker.lock().await;
    match tt.check_consent(&id) {
        Ok(exists) => Json(json!({ "consent_id": id, "verified": exists })),
        Err(e) => Json(json!({ "consent_id": id, "verified": false, "error": e })),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env();
    let data_dir = std::path::Path::new(&cfg.data_dir);
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        tracing::warn!("Не вдалося створити директорію даних {:?}: {e}", data_dir);
    }
    let db_path_buf = data_dir.join("accuracy.db");
    let db_path = db_path_buf.as_path();

    let accuracy = Arc::new(
        accuracy::AccuracyEngine::new(db_path)
            .expect("Failed to initialize SQLite accuracy database"),
    );
    let target_tracker = tokio::sync::Mutex::new(target_tracker::TargetTracker::new(db_path));

    let (online_tx, _) = broadcast::channel(128);
    let (poll_tx, _) = broadcast::channel(64);
    let (cooldown_tx, _) = broadcast::channel(256);
    let online_clients = Arc::new(AtomicUsize::new(0));
    let global_cooldown = Arc::new(AtomicU64::new(cfg.poll_interval_sec));

    let state = Arc::new(AppState {
        buffer: DelayBuffer::new(cfg.public_delay_sec),
        cfg: cfg.clone(),
        tracker: tokio::sync::Mutex::new(Tracker::with_expire(cfg.track_expire_sec)),
        target_tracker,
        monitor: tokio::sync::Mutex::new(monitor::LaunchMonitor::new()),
        alerts: RwLock::new(parsers::alerts::Response::default()),
        bounds: RwLock::new(parsers::alerts::Bounds::default()),
        official: RwLock::new(vec![]),
        last_fetch_ok: RwLock::new(false),
        last_error: RwLock::new(None),
        filtered_out: RwLock::new(0),
        deduped: RwLock::new(0),
        accuracy,
        online_clients,
        online_tx,
        poll_tx,
        global_cooldown,
        cooldown_tx,
    });

    let initial_official = 0.0;
    tokio::spawn(poll_loop(state.clone(), initial_official));

    let mut api = Router::new()
        .route("/api/health", get(health))
        .route("/api/beta/health", get(health))
        .route("/api/sources", get(sources))
        .route("/api/beta/sources", get(sources))
        .route("/api/geo", get(geo))
        .route("/api/beta/geo", get(beta_geo))
        .route("/api/bounds", get(bounds))
        .route("/api/beta/bounds", get(bounds))
        .route("/api/launches", get(launches))
        .route("/api/launch-events", get(launch_events))
        .route("/api/official", get(official))
        .route("/api/stats/accuracy", get(accuracy_stats))
        .route("/api/stats/methodology", get(accuracy_methodology))
        .route("/api/dataset/tracks", get(dataset_tracks))
        .route("/api/dataset/stats", get(dataset_stats))
        .route("/api/consent", post(handle_consent))
        .route("/api/beta/consent", post(handle_consent))
        .route("/api/consent/:id", get(check_consent))
        .route("/api/beta/consent/:id", get(check_consent))
        .route("/ws/geo", get(ws_geo))
        .route("/ws/beta/geo", get(ws_beta_geo));

    if cfg.enable_debug {
        api = api.route("/api/debug/outliers", get(outliers));
    }

    let frontend_dir = if let Some(ref d) = cfg.frontend_dir {
        d.clone()
    } else if std::path::Path::new("../frontend/index.html").exists() {
        "../frontend".to_string()
    } else if std::path::Path::new("frontend/index.html").exists() {
        "frontend".to_string()
    } else if std::path::Path::new("/opt/tacmap/frontend/index.html").exists() {
        "/opt/tacmap/frontend".to_string()
    } else {
        "../frontend".to_string()
    };

    let beta_dir = if std::path::Path::new("../frontend_beta/index.html").exists() {
        "../frontend_beta".to_string()
    } else if std::path::Path::new("frontend_beta/index.html").exists() {
        "frontend_beta".to_string()
    } else if std::path::Path::new("/opt/tacmap/frontend_beta/index.html").exists() {
        "/opt/tacmap/frontend_beta".to_string()
    } else {
        format!("{}_beta", frontend_dir)
    };

    let cors = if cfg.cors_allowed_origin == "*" {
        CorsLayer::permissive()
    } else if let Ok(origin) = cfg.cors_allowed_origin.parse::<axum::http::HeaderValue>() {
        CorsLayer::new()
            .allow_origin(origin)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    } else {
        CorsLayer::permissive()
    };

    let app = Router::new()
        .route("/live", get(redirect_live))
        .route("/live/", get(redirect_live))
        .route("/app", get(redirect_live))
        .route("/app/", get(redirect_live))
        .route("/map", get(redirect_live))
        .route("/map/", get(redirect_live))
        .nest_service(
            "/beta",
            tower_http::services::ServeDir::new(&beta_dir).append_index_html_on_directories(true),
        )
        .merge(api)
        .fallback_service(
            tower_http::services::ServeDir::new(&frontend_dir).append_index_html_on_directories(true),
        )
        .layer(axum::middleware::from_fn(set_smart_cache))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Не вдалося прив'язати {addr}: {e}. Перевірте, чи не запущено інший інстанс (pkill -f alerts-map-parser)");
            std::process::exit(1);
        }
    };
    tracing::info!("tacmap v{} listening on {addr}, delay={}s", env!("CARGO_PKG_VERSION"), cfg.public_delay_sec);
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("Помилка роботи сервера: {e}");
    }
}

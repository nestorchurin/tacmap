//! Трекер: пам'ять агрегатора між опитуваннями.
//!
//! Джерела іноді смикають координати (корекції в кілометри, ID reuse,
//! тестові викиди). Снапшот-проксі без пам'яті малює все це як стрибки.
//! Трекер веде історію кожної цілі і відкидає оновлення, фізично
//! неможливі для її типу (speed-gate з великим запасом).

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::borders::haversine_km;
use crate::models::Threat;

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Пороги speed-gate, км/год. Запас ~10x від фізики: різати тільки
/// телепорти (тисячі км за секунди), шуми і корекції пропускати.
fn max_speed_kmh(threat_type: &str) -> f64 {
    match threat_type {
        "uav" | "uav_jet" => 2000.0,
        "fpv" => 1500.0,
        "recon" => 4000.0,
        "missile" => 8000.0,
        "ballistic" => 25000.0,
        "kab" => 5000.0,
        "tactical" | "mig31k" => 8000.0,
        _ => 5000.0,
    }
}

#[derive(Debug, Clone)]
struct TrackPoint {
    time: f64,
    lat: f64,
    lon: f64,
}

#[derive(Debug)]
struct Track {
    latest: Threat,
    history: VecDeque<TrackPoint>,
    last_seen: f64,
    consec_reject: u32,
}

pub struct Tracker {
    tracks: HashMap<String, Track>,
    pub anomalies: u64,
    expire_after_sec: f64,
    max_consec_reject: u32,
    max_history: usize,
    trail_len: usize,
}

impl Tracker {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::with_expire(180.0)
    }

    pub fn with_expire(expire_after_sec: f64) -> Self {
        Self {
            tracks: HashMap::new(),
            anomalies: 0,
            expire_after_sec,
            max_consec_reject: 24,
            max_history: 24,
            trail_len: 8,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Один цикл опитування на вході, очищений стан на виході.
    /// Повертає тільки треки з кінцевими координатами в театрі
    /// (другий рубіж оборони, перший — bbox у поллері).
    pub fn update(&mut self, incoming: Vec<Threat>, now: f64) -> Vec<Threat> {
        for t in incoming {
            if !t.lat.is_finite() || !t.lon.is_finite() {
                self.anomalies += 1;
                continue;
            }
            match self.tracks.get_mut(&t.id) {
                Some(tr) => {
                    tr.last_seen = now;
                    let prev = tr.history.back().cloned().unwrap_or(TrackPoint {
                        time: now,
                        lat: tr.latest.lat,
                        lon: tr.latest.lon,
                    });
                    let dt = now - prev.time;
                    let dist = haversine_km(prev.lat, prev.lon, t.lat, t.lon);
                    let speed = if dt > 0.5 { dist / dt * 3600.0 } else { 0.0 };
                    if speed <= max_speed_kmh(&t.threat_type) {
                        tr.history.push_back(TrackPoint {
                            time: now,
                            lat: t.lat,
                            lon: t.lon,
                        });
                        while tr.history.len() > self.max_history {
                            tr.history.pop_front();
                        }
                        tr.latest = t;
                        tr.consec_reject = 0;
                    } else {
                        tr.consec_reject += 1;
                        self.anomalies += 1;
                    }
                }
                None => {
                    let mut history = VecDeque::with_capacity(self.max_history);
                    history.push_back(TrackPoint {
                        time: now,
                        lat: t.lat,
                        lon: t.lon,
                    });
                    self.tracks.insert(
                        t.id.clone(),
                        Track {
                            latest: t,
                            history,
                            last_seen: now,
                            consec_reject: 0,
                        },
                    );
                }
            }
        }
        self.tracks.retain(|_, tr| {
            now - tr.last_seen <= self.expire_after_sec
                && tr.consec_reject <= self.max_consec_reject
        });
        self.tracks
            .values()
            .filter(|tr| {
                crate::models::in_theater(tr.latest.lat, tr.latest.lon)
            })
            .map(|tr| {
                let mut t = tr.latest.clone();
                let n = tr.history.len();
                let from = n.saturating_sub(self.trail_len + 1);
                t.trail = tr.history
                    .iter()
                    .take(n.saturating_sub(1))
                    .skip(from)
                    .map(|p| [p.lat, p.lon])
                    .collect();
                t
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn mk(id: &str, lat: f64, lon: f64) -> Threat {
        Threat {
            id: id.to_string(),
            source: "dimap".to_string(),
            threat_type: "uav".to_string(),
            title: "SHAHED".to_string(),
            lat,
            lon,
            heading: Some(180.0),
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: "unknown".to_string(),
            source_count: 1,
            target_count: 1,
            updated_at: Some(Utc::now()),
            area_only: false,
            advisory: false,
            trail: vec![],
        }
    }

    #[test]
    fn accepts_normal_flight() {
        let mut tr = Tracker::new();
        let out = tr.update(vec![mk("a", 50.0, 31.0)], 0.0);
        assert_eq!(out.len(), 1);
        let out = tr.update(vec![mk("a", 50.1, 31.05)], 300.0);
        assert_eq!(out.len(), 1);
        assert!((out[0].lat - 50.1).abs() < 1e-9);
        assert_eq!(tr.anomalies, 0);
    }

    #[test]
    fn accepts_source_correction() {
        let mut tr = Tracker::new();
        tr.update(vec![mk("s", 50.0, 31.0)], 0.0);
        let out = tr.update(vec![mk("s", 50.018, 31.0)], 5.0);
        assert!((out[0].lat - 50.018).abs() < 1e-9);
        assert_eq!(tr.anomalies, 0);
    }

    #[test]
    fn rejects_teleport_to_africa() {
        let mut tr = Tracker::new();
        tr.update(vec![mk("s", 51.0, 31.0)], 0.0);
        let out = tr.update(vec![mk("s", 0.0, 31.0)], 5.0);
        assert_eq!(out.len(), 1);
        assert!((out[0].lat - 51.0).abs() < 1e-9);
        assert_eq!(tr.anomalies, 1);
    }

    #[test]
    fn drops_non_finite() {
        let mut t = mk("s", 50.0, 31.0);
        t.lat = f64::NAN;
        let mut tr = Tracker::new();
        let out = tr.update(vec![t], 0.0);
        assert!(out.is_empty());
        assert_eq!(tr.anomalies, 1);
    }

    #[test]
    fn expires_stale_tracks() {
        let mut tr = Tracker::new();
        tr.update(vec![mk("old", 50.0, 31.0)], 0.0);
        let out = tr.update(vec![], 700.0);
        assert!(out.is_empty());
        assert_eq!(tr.len(), 0);
    }

    #[test]
    fn expire_window_is_configurable() {
        let mut tr = Tracker::with_expire(30.0);
        tr.update(vec![mk("a", 50.0, 31.0)], 0.0);
        assert_eq!(tr.update(vec![], 29.0).len(), 1, "на 29с ще живий");
        assert!(tr.update(vec![], 31.0).is_empty(), "на 31с вже прибрано");
    }

    #[test]
    fn builds_trail() {
        let mut tr = Tracker::new();
        tr.update(vec![mk("t", 50.0, 31.0)], 0.0);
        tr.update(vec![mk("t", 50.05, 31.0)], 60.0);
        let out = tr.update(vec![mk("t", 50.1, 31.0)], 120.0);
        assert_eq!(out[0].trail.len(), 2);
    }
}

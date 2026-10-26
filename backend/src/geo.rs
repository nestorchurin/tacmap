//! ЄДИНА точка, де координати стають картографічними об'єктами.
//! Фронт малює FeatureCollection 1-в-1 через setData (повна заміна,
//! ніяких DOM-маркерів і пулів — привиди неможливі за конструкцією).
//! GeoJSON вимагає порядок [lon, lat] — він збирається ТІЛЬКИ тут.

use serde_json::{json, Value};

use crate::approach::{fmt_eta, Corridor};
use crate::borders::{bearing_deg, dest_point_ll, haversine_km, inside_ukraine};
use crate::config::Config;
use crate::launches::{kind_color, kind_label, LaunchSite};
use crate::models::{ua_label, Cluster, Threat};
use crate::monitor::LaunchEvent;

fn fmt_ll(lat: f64, lon: f64) -> String {
    format!("{lat:.3}, {lon:.3}")
}

/// Порожня колекція (сумісність).
#[allow(dead_code)]
pub fn empty_collection() -> Value {
    json!({"type": "FeatureCollection", "features": []})
}

/// Точки пусків: статичний довідковий шар. Порядок координат той самий —
/// [lon, lat]. Не фільтруємо театром: частина баз (Оленья, Каспій) поза ним.
pub fn launches_collection(sites: &[LaunchSite]) -> Value {
    let features: Vec<Value> = sites
        .iter()
        .map(|s| {
            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [s.lon, s.lat]},
                "properties": {
                    "id": s.id,
                    "name": s.name,
                    "kind": s.kind,
                    "kind_label": kind_label(&s.kind),
                    "color": kind_color(&s.kind),
                    "note": s.note,
                    "latlon": fmt_ll(s.lat, s.lon),
                },
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// Вхід: шлях у (lat, lon); вихід: зовнішні (поза Україною) лінії у [lon, lat].
/// Щільна вибірка кроком 4 км, внутрішні прогони викидаються.
fn clip_outside_ukraine(path_ll: &[[f64; 2]]) -> Vec<Vec<[f64; 2]>> {
    const STEP_KM: f64 = 4.0;
    // 1. щільна вибірка вздовж шляху
    let mut pts: Vec<[f64; 2]> = Vec::new();
    for w in path_ll.windows(2) {
        let (a, b) = (w[0], w[1]);
        let dist = haversine_km(a[0], a[1], b[0], b[1]);
        let n = (dist / STEP_KM).ceil() as usize;
        let brg = bearing_deg(a[0], a[1], b[0], b[1]);
        for k in 0..n.max(1) {
            let d = (k as f64) * STEP_KM;
            if d >= dist {
                break;
            }
            pts.push(dest_point_ll(a[0], a[1], brg, d));
        }
    }
    if let Some(last) = path_ll.last() {
        pts.push(*last);
    }
    // 2. безперервні зовнішні прогони
    let mut lines: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    for p in pts {
        if inside_ukraine(p[0], p[1]) {
            if cur.len() >= 2 {
                lines.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        } else {
            cur.push([p[1], p[0]]);
        }
    }
    if cur.len() >= 2 {
        lines.push(cur);
    }
    lines
}

/// Статичні коридори: пуск → найближча точка кордону + ETA (довідково).
/// Так само тільки зовнішня частина; кримські пуски (всередині) не малюються.
pub fn corridors_collection(corridors: &[Corridor], sites: &[LaunchSite]) -> Value {
    let features: Vec<Value> = corridors
        .iter()
        .filter_map(|c| {
            let s = &sites[c.launch_idx];
            let lines = clip_outside_ukraine(&[[s.lat, s.lon], [c.border_lat, c.border_lon]]);
            if lines.is_empty() {
                return None;
            }
            let geometry = if lines.len() == 1 {
                json!({"type": "LineString", "coordinates": lines[0]})
            } else {
                json!({"type": "MultiLineString", "coordinates": lines})
            };
            Some(json!({
                "type": "Feature",
                "geometry": geometry,
                "properties": {
                    "launch": s.name,
                    "launch_id": s.id,
                    "kind": s.kind,
                    "kind_label": kind_label(&s.kind),
                    "color": kind_color(&s.kind),
                    "dist_km": (c.dist_km * 10.0).round() / 10.0,
                    "eta_min": (c.eta_min * 10.0).round() / 10.0,
                    "eta_label": fmt_eta(c.eta_min),
                },
            }))
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// Активні події пусків для банера (ймовірні, евристика).
pub fn launch_events_collection(events: &[LaunchEvent], now: f64) -> Value {
    let features: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [e.lon, e.lat]},
                "properties": {
                    "launch_id": e.launch_id,
                    "name": e.launch_name,
                    "kind": e.kind,
                    "kind_label": kind_label(&e.kind),
                    "color": kind_color(&e.kind),
                    "count": e.count,
                    "types": e.types.join(","),
                    "ago_min": ((e.ago_min(now) * 10.0).round() / 10.0),
                    "ago_sec": ((now - e.last_seen).max(0.0).round()),
                    "is_fresh": (now - e.last_seen) <= 120.0,
                    "first_seen": e.first_seen,
                    "last_seen": e.last_seen,
                    "origin": e.origin,
                    "latlon": fmt_ll(e.lat, e.lon),
                },
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// Офіційні сповіщення: плоский масив (не GeoJSON — у частини немає точки).
/// Фронт малює банер за текстом, а точки — за lat/lon, якщо є.
pub fn official_collection(
    notices: &[crate::parsers::official::Notice],
    now: f64,
) -> Value {
    use crate::parsers::official::{kind_color, kind_label};
    Value::Array(
        notices
            .iter()
            .map(|n| {
                let ts = n.observed_at.timestamp() as f64;
                let age = (now - ts).max(0.0);
                json!({
                    "id": n.id,
                    "channel": n.channel,
                    "post_url": n.post_url,
                    "text": n.text,
                    "kind": n.kind,
                    "kind_label": kind_label(&n.kind),
                    "color": kind_color(&n.kind),
                    "oblast": n.oblast,
                    "lat": n.lat,
                    "lon": n.lon,
                    "observed_at": n.observed_at.to_rfc3339(),
                    "observed_ts": ts,
                    "ago_min": (((age / 60.0) * 10.0).round() / 10.0),
                    "ago_sec": age.round(),
                    "is_fresh": age <= 3600.0,
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Живі цілі з джерел: threats + консенсус + trail + вектори напрямку.
// ---------------------------------------------------------------------------

pub const CONSENSUS_COLOR: &str = "#7dff9a";

/// Довжина вектора напрямку, км (~15 хв польоту типом).
fn vector_km(threat_type: &str) -> f64 {
    match threat_type {
        "uav" | "uav_jet" => 40.0,
        "fpv" => 10.0,
        "recon" => 60.0,
        "missile" => 150.0,
        "ballistic" => 200.0,
        "kab" => 30.0,
        "tactical" => 120.0,
        "mig31k" => 200.0,
        _ => 25.0,
    }
}

fn threat_props(t: &Threat, color: &str, clustered: bool) -> Value {
    json!({
        "id": t.id,
        "source": t.source,
        "color": color,
        "rtype": t.threat_type,
        "label": ua_label(&t.threat_type, &t.title),
        "region": t.region,
        "locality": t.locality,
        "confidence": t.confidence,
        "source_count": t.source_count,
        "updated_at": t.updated_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
        "heading": t.heading,
        "has_heading": t.heading.is_some(),
        "is_static": t.heading.is_none(),
        "lat": t.lat,
        "lon": t.lon,
        "latlon": fmt_ll(t.lat, t.lon),
        "clustered": clustered,
        "area": t.area_only,
    })
}

pub fn threats_collection(
    threats: &[Threat],
    clustered_ids: &std::collections::HashSet<String>,
    cfg: &Config,
) -> Value {
    let features: Vec<Value> = threats
        .iter()
        .map(|t| {
            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [t.lon, t.lat]},
                "properties": threat_props(t, cfg.color_for(&t.source), clustered_ids.contains(&t.id)),
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

pub fn consolidated_targets_collection(clusters: &[Cluster], cfg: &Config) -> Value {
    let features: Vec<Value> = clusters
        .iter()
        .map(|c| {
            let member_lines: Vec<String> = c
                .members
                .iter()
                .map(|m| {
                    let loc = if !m.locality.is_empty() {
                        format!(" ({})", m.locality)
                    } else if !m.region.is_empty() {
                        format!(" ({})", m.region)
                    } else {
                        String::new()
                    };
                    let hd = m.heading.map(|h| format!(" · курс {:.0}°", h)).unwrap_or_default();
                    format!("{}: {}{}{}", m.source.to_uppercase(), m.title, loc, hd)
                })
                .collect();

            let color = if c.confirmed {
                CONSENSUS_COLOR
            } else {
                cfg.color_for(&c.sources[0])
            };

            let target_count = c.target_count.max(1);
            let badge = if target_count >= 2 {
                format!("×{}", target_count)
            } else {
                String::new()
            };

            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [c.lon, c.lat]},
                "properties": {
                    "id": c.id,
                    "rtype": c.threat_type,
                    "title": c.title,
                    "label": c.label,
                    "color": color,
                    "source": if c.confirmed { "consensus" } else { &c.sources[0] },
                    "sources": c.sources,
                    "source_count": c.sources.len(),
                    "target_count": target_count,
                    "confirmed": c.confirmed,
                    "confirmable": matches!(c.threat_type.as_str(), "uav" | "uav_jet" | "missile" | "ballistic"),
                    "confidence": c.confidence,
                    "agreement": c.agreement,
                    "spread_km": c.spread_km,
                    "heading": c.heading,
                    "has_heading": c.heading.is_some(),
                    "is_static": c.heading.is_none(),
                    "region": c.region,
                    "locality": c.locality,
                    "badge": badge,
                    "members": member_lines.join("\n"),
                    "latlon": fmt_ll(c.lat, c.lon),
                    "trail": c.trail.iter().map(|[la, lo]| [*lo, *la]).collect::<Vec<[f64; 2]>>(),
                    "area": false,
                },
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

pub fn consolidated_vectors_collection(clusters: &[Cluster], cfg: &Config) -> Value {
    let features: Vec<Value> = clusters
        .iter()
        .filter(|c| c.heading.is_some())
        .map(|c| {
            let h = c.heading.unwrap();
            let end = dest_point_ll(c.lat, c.lon, h, vector_km(&c.threat_type));
            let color = if c.confirmed {
                CONSENSUS_COLOR
            } else {
                cfg.color_for(&c.sources[0])
            };
            json!({
                "type": "Feature",
                "properties": {
                    "color": color,
                    "rtype": c.threat_type,
                    "source": if c.confirmed { "consensus" } else { &c.sources[0] },
                    "confirmed": c.confirmed,
                },
                "geometry": {"type": "LineString", "coordinates": [[c.lon, c.lat], [end[1], end[0]]]},
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

#[allow(dead_code)]
pub fn consolidated_trails_collection(clusters: &[Cluster], cfg: &Config) -> Value {
    let features: Vec<Value> = clusters
        .iter()
        .filter(|c| !c.trail.is_empty())
        .map(|c| {
            let mut line: Vec<[f64; 2]> =
                c.trail.iter().map(|[la, lo]| [*lo, *la]).collect();
            line.push([c.lon, c.lat]);
            let color = if c.confirmed {
                CONSENSUS_COLOR
            } else {
                cfg.color_for(&c.sources[0])
            };
            json!({
                "type": "Feature",
                "properties": {
                    "color": color,
                    "rtype": c.threat_type,
                    "source": if c.confirmed { "consensus" } else { &c.sources[0] },
                    "confirmed": c.confirmed,
                },
                "geometry": {"type": "LineString", "coordinates": line},
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

pub fn clusters_collection(clusters: &[Cluster], cfg: &Config) -> Value {
    let _ = cfg;
    let features: Vec<Value> = clusters
        .iter()
        .filter(|c| c.sources.len() > 1)
        .map(|c| {
            let members: Vec<String> = c
                .members
                .iter()
                .map(|m| {
                    format!(
                        "{}: {}",
                        m.source.to_uppercase(),
                        if m.locality.is_empty() { m.id.clone() } else { m.locality.clone() }
                    )
                })
                .collect();
            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [c.lon, c.lat]},
                "properties": {
                    "id": c.id,
                    "rtype": c.threat_type,
                    "label": ua_label(&c.threat_type, ""),
                    "color": CONSENSUS_COLOR,
                    "count": c.sources.len(),
                    "agreement": c.agreement,
                    "spread_km": c.spread_km,
                    "confidence": c.confidence,
                    "members": members.join("\n"),
                    "latlon": fmt_ll(c.lat, c.lon),
                },
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

pub fn trails_collection(
    threats: &[Threat],
    clustered_ids: &std::collections::HashSet<String>,
    cfg: &Config,
) -> Value {
    let features: Vec<Value> = threats
        .iter()
        .filter(|t| !t.trail.is_empty())
        .map(|t| {
            let mut line: Vec<[f64; 2]> =
                t.trail.iter().map(|[la, lo]| [*lo, *la]).collect();
            line.push([t.lon, t.lat]);
            json!({
                "type": "Feature",
                "properties": {
                    "color": cfg.color_for(&t.source),
                    "rtype": t.threat_type,
                    "source": t.source,
                    "clustered": clustered_ids.contains(&t.id),
                },
                "geometry": {"type": "LineString", "coordinates": line},
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// Вектори напрямку: коротка лінія від мітки за курсом.
pub fn vectors_collection(
    threats: &[Threat],
    clustered_ids: &std::collections::HashSet<String>,
    cfg: &Config,
) -> Value {
    let features: Vec<Value> = threats
        .iter()
        .filter(|t| t.heading.is_some() && !t.area_only)
        .map(|t| {
            // dest_point_ll повертає (lat, lon) — для GeoJSON перевертаємо
            let end = dest_point_ll(t.lat, t.lon, t.heading.unwrap_or(0.0), vector_km(&t.threat_type));
            json!({
                "type": "Feature",
                "properties": {
                    "color": cfg.color_for(&t.source),
                    "rtype": t.threat_type,
                    "source": t.source,
                    "clustered": clustered_ids.contains(&t.id),
                },
                "geometry": {"type": "LineString", "coordinates": [[t.lon, t.lat], [end[1], end[0]]]},
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

#[cfg(test)]
mod threat_tests {
    use super::*;
    use crate::models::Threat;
    use chrono::Utc;
    use std::collections::HashSet;

    fn mk() -> Threat {
        Threat {
            id: "dimap:x".to_string(),
            source: "dimap".to_string(),
            threat_type: "uav".to_string(),
            title: "SHAHED".to_string(),
            lat: 50.0,
            lon: 31.0,
            heading: Some(90.0),
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
    fn geojson_order_is_lon_lat() {
        let cfg = Config::from_env();
        let fc = threats_collection(&[mk()], &HashSet::new(), &cfg);
        let coords = &fc["features"][0]["geometry"]["coordinates"];
        assert_eq!(coords[0], 31.0, "першим йде lon");
        assert_eq!(coords[1], 50.0, "другим йде lat");
        assert_eq!(fc["features"][0]["properties"]["latlon"], "50.000, 31.000");
        assert_eq!(fc["features"][0]["properties"]["label"], "Шахед / БПЛА");
    }

    #[test]
    fn vectors_start_at_threat_and_point_east() {
        let cfg = Config::from_env();
        let fc = vectors_collection(&[mk()], &HashSet::new(), &cfg);
        let line = &fc["features"][0]["geometry"]["coordinates"];
        assert_eq!(line[0][0], 31.0, "початок — сама мітка");
        assert!(
            line[1][0].as_f64().unwrap() > 31.0,
            "курс 90° веде на схід (збільшує lon)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_order_is_lon_lat() {
        let sites = crate::launches::all_sites();
        let fc = launches_collection(&sites);
        let feats = fc["features"].as_array().unwrap();
        assert_eq!(feats.len(), sites.len());
        // Приморсько-Ахтарськ: lon першим
        let f0 = feats.iter().find(|f| f["properties"]["id"] == "primorsko-akhtarsk").unwrap();
        assert!((f0["geometry"]["coordinates"][0].as_f64().unwrap() - 38.172).abs() < 1e-9);
        assert!((f0["geometry"]["coordinates"][1].as_f64().unwrap() - 46.059).abs() < 1e-9);
        assert_eq!(f0["properties"]["kind_label"], "Пуск БПЛА / Шахед");
    }

    /// Всі координати геометрії (LineString або MultiLineString).
    fn all_coords(f: &serde_json::Value) -> Vec<[f64; 2]> {
        let g = &f["geometry"];
        let mut out = Vec::new();
        let push = |c: &serde_json::Value, out: &mut Vec<[f64; 2]>| {
            out.push([
                c[0].as_f64().unwrap(),
                c[1].as_f64().unwrap(),
            ]);
        };
        match g["type"].as_str().unwrap() {
            "LineString" => {
                for c in g["coordinates"].as_array().unwrap() {
                    push(c, &mut out);
                }
            }
            "MultiLineString" => {
                for line in g["coordinates"].as_array().unwrap() {
                    for c in line.as_array().unwrap() {
                        push(c, &mut out);
                    }
                }
            }
            t => panic!("несподівана геометрія {t}"),
        }
        out
    }

    fn assert_all_outside(fc: &serde_json::Value) {
        for f in fc["features"].as_array().unwrap() {
            for [lon, lat] in all_coords(f) {
                assert!(
                    !crate::borders::inside_ukraine(lat, lon),
                    "точка {lat},{lon} всередині України"
                );
            }
        }
    }

    #[test]
    fn corridors_skip_inside_sites() {        use crate::approach::build_corridors;
        let sites = crate::launches::all_sites();
        let corr = build_corridors(&sites);
        let fc = corridors_collection(&corr, &sites);
        let n = fc["features"].as_array().unwrap().len();
        assert!(n < sites.len(), "кримські пуски всередині — без ліній, є {n}");
        assert!(n > 0);
        assert_all_outside(&fc);
        // Оленья (зовні) — є
        assert!(fc["features"].as_array().unwrap().iter().any(|f| f["properties"]["launch_id"] == "olenya"));
    }
}

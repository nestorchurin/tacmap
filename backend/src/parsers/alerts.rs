//! Парсер тривог: офіційні статуси + межі районів/областей.
//! Як роблять усі карти: статуси → ключі → полігони з keyed geojson → заливка.
//! GET {base}/api/v1/alerts → {raions: [{key,…}], oblasts: [{key,…}]}
//! Межі: {base}/raions.geojson (136 фіч, props {key, rayon}),
//!       {base}/oblasts.geojson (27 фіч, props {key, region}).
//! Межі кешуються в пам'яті, оновлюються раз на 6 год.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use super::client;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AlertEntry {
    pub key: Option<String>,
    pub name: Option<String>,
    pub oblast: Option<String>,
    pub since: Option<String>,
    pub level: Option<String>,
    pub reasons: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub raions: Vec<AlertEntry>,
    #[serde(default)]
    pub oblasts: Vec<AlertEntry>,
}

pub fn normalize_alert_key(name: &str) -> String {
    let mut s = name.to_lowercase();
    for ch in ['’', '\u{02bc}', '\'', '`'] {
        s = s.replace(ch, "");
    }
    s = s
        .replace(" область", "")
        .replace(" район", "")
        .replace(" територіальна громада", "")
        .replace(" громада", "")
        .replace("місто ", "")
        .trim()
        .to_string();

    match s.as_str() {
        "звягельський" => "новоград-волинський".to_string(),
        "самарівський" => "новомосковський".to_string(),
        "м. харків та харківська" | "харківська міська" => "харківський".to_string(),
        _ => s,
    }
}

/// Офіційне державне джерело тривог: map.ukrainealarm.com (Ajax Systems / Мінцифра / ПС ЗСУ).
/// Містить точні рівні небезпеки в реальному часі:
/// - Yellow (жовтий): дронова загроза (ПКМУ №1092)
/// - Red (червоний): ракетна небезпека / загальна повітряна тривога
/// - Orange (помаранчевий): артилерійський обстріл
async fn fetch_ukrainealarm(base_url: &str) -> Result<Response, String> {
    let url = format!("{}/api/v2/data/mapUpdate", base_url.trim_end_matches('/'));
    let c = client()?;
    let resp = c.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("UkraineAlarm HTTP error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("UkraineAlarm HTTP status {}", resp.status()));
    }

    let val: Value = resp.json().await.map_err(|e| format!("UkraineAlarm JSON error: {e}"))?;
    let list = val.get("alerts").and_then(|a| a.as_array()).ok_or_else(|| "No alerts array".to_string())?;

    let mut raions = Vec::new();
    let mut oblasts = Vec::new();

    for item in list {
        let name = item.get("regionName").and_then(|n| n.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let key = normalize_alert_key(name);
        let r_type = item.get("regionType").and_then(|t| t.as_str()).unwrap_or("");

        let mut levels = Vec::new();
        let mut reasons = Vec::new();

        if let Some(active_alerts) = item.get("activeAlerts").and_then(|a| a.as_array()) {
            for a in active_alerts {
                if let Some(alert_levels) = a.get("activeAlertLevels").and_then(|l| l.as_array()) {
                    for lvl in alert_levels {
                        if let Some(l) = lvl.get("alertLevel").and_then(|v| v.as_str()) {
                            levels.push(l.to_lowercase());
                        }
                        if let Some(r) = lvl.get("reason").and_then(|v| v.as_str()) {
                            if !r.trim().is_empty() {
                                reasons.push(r.trim().to_string());
                            }
                        }
                    }
                }
            }
        }

        if levels.is_empty() {
            continue;
        }

        let level = if levels.iter().any(|l| l == "red") {
            "red"
        } else if levels.iter().any(|l| l == "yellow") {
            "yellow"
        } else if levels.iter().any(|l| l == "orange") {
            "orange"
        } else {
            "red"
        };

        let since = item.get("lastUpdate").and_then(|u| u.as_str()).map(|s| s.to_string());

        let entry = AlertEntry {
            key: Some(key),
            name: Some(name.to_string()),
            oblast: None,
            since,
            level: Some(level.to_string()),
            reasons: Some(reasons),
        };

        if r_type == "District" {
            raions.push(entry);
        } else if r_type == "State" {
            oblasts.push(entry);
        }
    }

    if raions.is_empty() && oblasts.is_empty() {
        return Err("UkraineAlarm returned 0 active alerts".to_string());
    }

    Ok(Response { raions, oblasts })
}

pub async fn fetch(ukrainealarm_base_url: &str, fallback_base_url: &str) -> Result<Response, String> {
    // 1. Офіційне джерело тривог — «Повітряна тривога» (map.ukrainealarm.com)
    match fetch_ukrainealarm(ukrainealarm_base_url).await {
        Ok(data) => return Ok(data),
        Err(e) => {
            tracing::warn!("UkraineAlarm alerts fetch failed, using fallback: {e}");
        }
    }

    // 2. Резервне джерело (Neptun API /api/v1/alerts)
    let url = format!("{}/api/v1/alerts", fallback_base_url.trim_end_matches('/'));
    if let Ok(c) = client() {
        if let Ok(resp) = c.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<Response>().await {
                    return Ok(data);
                }
            }
        }
    }

    // 3. Друге резервне джерело (shahmap.online/api/alerts)
    if let Ok(c) = client() {
        if let Ok(resp) = c.get("https://shahmap.online/api/alerts").send().await {
            if resp.status().is_success() {
                if let Ok(val) = resp.json::<Value>().await {
                    if let Some(list) = val.get("alerts").and_then(|a| a.as_array()) {
                        let mut raions = vec![];
                        let mut oblasts = vec![];
                        for item in list {
                            if !item.get("active").and_then(|a| a.as_bool()).unwrap_or(true) {
                                continue;
                            }
                            let name = item.get("regionName").and_then(|n| n.as_str()).unwrap_or("");
                            let level = item.get("alertLevel").and_then(|l| l.as_str()).unwrap_or("red");
                            let r_type = item.get("regionType").and_then(|t| t.as_str()).unwrap_or("");
                            let entry = AlertEntry {
                                key: Some(normalize_alert_key(name)),
                                name: Some(name.to_string()),
                                oblast: None,
                                since: item.get("updatedAt").and_then(|u| u.as_str()).map(|s| s.to_string()),
                                level: Some(level.to_lowercase()),
                                reasons: Some(vec![]),
                            };
                            if r_type == "State" {
                                oblasts.push(entry);
                            } else {
                                raions.push(entry);
                            }
                        }
                        return Ok(Response { raions, oblasts });
                    }
                }
            }
        }
    }

    Err("Не вдалося завантажити тривоги з офіційного (UkraineAlarm) та резервних джерел".to_string())
}

// ---------------------------------------------------------------------------
// Кеш меж: key → geometry
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Bounds {
    pub raions: HashMap<String, Value>,
    pub oblasts: HashMap<String, Value>,
    pub loaded_at: f64,
}

pub async fn fetch_bounds(base_url: &str) -> Result<Bounds, String> {
    let c = client()?;
    let base = base_url.trim_end_matches('/');
    let rj: Value = c
        .get(format!("{base}/raions.geojson"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let oj: Value = c
        .get(format!("{base}/oblasts.geojson"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(Bounds {
        raions: index_fc(&rj),
        oblasts: index_fc(&oj),
        loaded_at: crate::tracker::now_secs(),
    })
}

fn index_fc(fc: &Value) -> HashMap<String, Value> {
    fc.get("features")
        .and_then(|f| f.as_array())
        .map(|fs| {
            fs.iter()
                .filter_map(|f| {
                    let raw_key = f.get("properties")?.get("key")?.as_str()?;
                    let key = normalize_alert_key(raw_key);
                    let geom = f.get("geometry")?.clone();
                    Some((key, geom))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Збірка шару тривог: тільки райони/області зі статусом
// ---------------------------------------------------------------------------

pub fn alerts_collection(resp: &Response, bounds: &Bounds) -> Value {
    let mut features = vec![];
    let mut push = |kind: &str, e: &AlertEntry, geoms: &HashMap<String, Value>| {
        let key = e.key.clone().unwrap_or_default();
        let Some(geom) = geoms.get(&key) else {
            return;
        };
        let level = e.level.as_deref().unwrap_or("red").to_lowercase();
        let (color, level_title) = match level.as_str() {
            "yellow" => ("#ffd60a", "Жовтий рівень (Дронова загроза)"),
            "orange" => ("#ff9f0a", "Помаранчевий рівень (Артилерійська загроза)"),
            _ => ("#ff3b30", "Червоний рівень (Ракетна небезпека)"),
        };
        features.push(json!({
            "type": "Feature",
            "geometry": geom,
            "properties": {
                "kind": kind,
                "key": key,
                "name": e.name.clone().unwrap_or_default(),
                "oblast": e.oblast.clone().unwrap_or_default(),
                "since": e.since.clone().unwrap_or_default(),
                "level": level,
                "level_title": level_title,
                "color": color,
                "reasons": e.reasons.clone().unwrap_or_default().join("; "),
            },
        }));
    };
    for e in &resp.raions {
        push("raion", e, &bounds.raions);
    }
    for e in &resp.oblasts {
        push("oblast", e, &bounds.oblasts);
    }
    json!({"type": "FeatureCollection", "features": features})
}

// ---------------------------------------------------------------------------
// Геометрична перевірка: чи точка всередині зони тривоги (Ray casting)
// ---------------------------------------------------------------------------

/// Перевірка чи точка (lat, lon) знаходиться всередині полігонального кільця [lon, lat]
pub fn point_in_ring(lat: f64, lon: f64, ring: &Value) -> bool {
    let Some(coords) = ring.as_array() else {
        return false;
    };
    if coords.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = coords.len();
    let mut j = n - 1;
    for i in 0..n {
        let (Some(xi), Some(yi)) = (
            coords[i].get(0).and_then(|v| v.as_f64()),
            coords[i].get(1).and_then(|v| v.as_f64()),
        ) else {
            j = i;
            continue;
        };
        let (Some(xj), Some(yj)) = (
            coords[j].get(0).and_then(|v| v.as_f64()),
            coords[j].get(1).and_then(|v| v.as_f64()),
        ) else {
            j = i;
            continue;
        };

        if (yi > lat) != (yj > lat) {
            let xhit = (xj - xi) * (lat - yi) / (yj - yi) + xi;
            if lon < xhit {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Перевірка точки у полігоні GeoJSON (з урахуванням дірок/отворів)
pub fn point_in_polygon(lat: f64, lon: f64, rings: &Value) -> bool {
    let Some(rings_arr) = rings.as_array() else {
        return false;
    };
    if rings_arr.is_empty() {
        return false;
    }
    // Точка повинна бути всередині зовнішнього контуру
    if !point_in_ring(lat, lon, &rings_arr[0]) {
        return false;
    }
    // І НЕ повинна потрапляти у жодну внутрішню дірку
    for hole in &rings_arr[1..] {
        if point_in_ring(lat, lon, hole) {
            return false;
        }
    }
    true
}

/// Перевірка чи точка (lat, lon) потрапляє в геометрію GeoJSON (Polygon або MultiPolygon)
pub fn point_in_geometry(lat: f64, lon: f64, geom: &Value) -> bool {
    let geom_type = geom.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let Some(coords) = geom.get("coordinates") else {
        return false;
    };
    match geom_type {
        "Polygon" => point_in_polygon(lat, lon, coords),
        "MultiPolygon" => {
            if let Some(poly_list) = coords.as_array() {
                for poly in poly_list {
                    if point_in_polygon(lat, lon, poly) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Перевіряє, чи знаходиться точка у зоні оголошеної тривоги (жовтий, помаранчевий або червоний рівень).
/// Перевіряє спочатку активні райони, потім активні області.
pub fn is_point_in_alert_zone(lat: f64, lon: f64, alerts: &Response, bounds: &Bounds) -> bool {
    // Якщо геометрії меж ще не завантажилися (холодний старт), захищаємо від помилкових штрафів
    if bounds.raions.is_empty() && bounds.oblasts.is_empty() {
        return true;
    }

    let is_active_level = |level: Option<&str>| -> bool {
        let l = level.unwrap_or("red").to_lowercase();
        l != "none" && l != "off" && l != "green" && l != "clear"
    };

    // 1. Активні райони
    for entry in &alerts.raions {
        if !is_active_level(entry.level.as_deref()) {
            continue;
        }
        if let Some(key) = &entry.key {
            if let Some(geom) = bounds.raions.get(key) {
                if point_in_geometry(lat, lon, geom) {
                    return true;
                }
            }
        }
    }

    // 2. Активні області
    for entry in &alerts.oblasts {
        if !is_active_level(entry.level.as_deref()) {
            continue;
        }
        if let Some(key) = &entry.key {
            if let Some(geom) = bounds.oblasts.get(key) {
                if point_in_geometry(lat, lon, geom) {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_alerts_to_bounds() {
        let resp: Response = serde_json::from_str(
            r#"{"raions": [
                {"key": "бахмутський", "name": "Бахмутський район",
                 "oblast": "Донецька область", "level": "red", "reasons": ["Ракети"]},
                {"key": "неіснуючий", "name": "X"}
            ], "oblasts": []}"#,
        )
        .unwrap();
        let bounds = Bounds {
            raions: [(
                "бахмутський".to_string(),
                json!({"type": "Polygon", "coordinates": [[[30.0, 50.0], [31.0, 50.0], [31.0, 49.0], [30.0, 50.0]]]}),
            )]
            .into_iter()
            .collect(),
            oblasts: HashMap::new(),
            loaded_at: 0.0,
        };
        let fc = alerts_collection(&resp, &bounds);
        let feats = fc["features"].as_array().unwrap();
        assert_eq!(feats.len(), 1, "без межі — не малюємо");
        assert_eq!(feats[0]["properties"]["name"], "Бахмутський район");
        assert_eq!(feats[0]["properties"]["reasons"], "Ракети");
        assert_eq!(feats[0]["geometry"]["type"], "Polygon");
    }

    #[test]
    fn test_point_in_geometry() {
        // Квадратний полігон [lon, lat]: (30.0, 50.0) до (32.0, 52.0)
        let poly = json!({
            "type": "Polygon",
            "coordinates": [[
                [30.0, 50.0],
                [32.0, 50.0],
                [32.0, 52.0],
                [30.0, 52.0],
                [30.0, 50.0]
            ]]
        });

        // Всередині
        assert!(point_in_geometry(51.0, 31.0, &poly));
        // Ззовні (північніше)
        assert!(!point_in_geometry(53.0, 31.0, &poly));
        // Ззовні (східніше)
        assert!(!point_in_geometry(51.0, 33.0, &poly));
    }

    #[test]
    fn test_point_in_geometry_multipolygon() {
        let multi = json!({
            "type": "MultiPolygon",
            "coordinates": [
                [[[10.0, 10.0], [12.0, 10.0], [12.0, 12.0], [10.0, 12.0], [10.0, 10.0]]],
                [[[20.0, 20.0], [22.0, 20.0], [22.0, 22.0], [20.0, 22.0], [20.0, 20.0]]]
            ]
        });

        assert!(point_in_geometry(11.0, 11.0, &multi));
        assert!(point_in_geometry(21.0, 21.0, &multi));
        assert!(!point_in_geometry(15.0, 15.0, &multi));
    }

    #[test]
    fn test_is_point_in_alert_zone() {
        let resp = Response {
            raions: vec![
                AlertEntry {
                    key: Some("київський".to_string()),
                    level: Some("yellow".to_string()),
                    ..Default::default()
                }
            ],
            oblasts: vec![
                AlertEntry {
                    key: Some("полтавська".to_string()),
                    level: Some("red".to_string()),
                    ..Default::default()
                }
            ],
        };

        let bounds = Bounds {
            raions: [(
                "київський".to_string(),
                json!({"type": "Polygon", "coordinates": [[[30.0, 50.0], [31.0, 50.0], [31.0, 51.0], [30.0, 51.0], [30.0, 50.0]]]}),
            )]
            .into_iter()
            .collect(),
            oblasts: [(
                "полтавська".to_string(),
                json!({"type": "Polygon", "coordinates": [[[34.0, 49.0], [35.0, 49.0], [35.0, 50.0], [34.0, 50.0], [34.0, 49.0]]]}),
            )]
            .into_iter()
            .collect(),
            loaded_at: 100.0,
        };

        // Точка в районі з жовтим рівнем (дронова небезпека)
        assert!(is_point_in_alert_zone(50.5, 30.5, &resp, &bounds));

        // Точка в області з червоним рівнем (тривога)
        assert!(is_point_in_alert_zone(49.5, 34.5, &resp, &bounds));

        // Спокійна точка без тривоги
        assert!(!is_point_in_alert_zone(48.0, 25.0, &resp, &bounds));
    }
}

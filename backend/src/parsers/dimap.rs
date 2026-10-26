//! Парсер DIMAP: GET {base}/targets (знайдено реверсом фронту dimap.live).
//! CORS у них тільки під свій домен — ходимо сервер-сервер.
//! Потрібна атрибуція; вимикається ENABLE_DIMAP=false.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::client;
use crate::models::Threat;

#[derive(Debug, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub targets: Vec<RawTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTarget {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub target_type: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub bearing: Option<f64>,
    #[allow(dead_code)]
    pub speed: Option<f64>,
    pub last_seen: Option<String>,
}

pub fn map_type(raw: &str) -> String {
    let u = raw.to_uppercase();
    if u.contains("REACTIVE") || u.contains("JET") {
        return "uav_jet".to_string();
    }
    if u.contains("BANDEROL") || u.contains("БАНДЕРОЛ") {
        return "uav".to_string();
    }
    if u.contains("FPV") {
        return "fpv".to_string();
    }
    if u.contains("SCOUT") || u.contains("RECON") || u.contains("ORLAN") || u.contains("ZALA") {
        return "recon".to_string();
    }
    if u.contains("BALLISTIC") || u.contains("БАЛІСТ") {
        return "ballistic".to_string();
    }
    if u.contains("MISSILE") || u.contains("РАКЕТ") || u.contains("KALIBR") || u.contains("KH-") || u.contains("X-101") {
        return "missile".to_string();
    }
    if u.contains("KAB") || u.contains("КАБ") {
        return "kab".to_string();
    }
    if u.contains("MIG") || u.contains("МІГ") {
        return "mig31k".to_string();
    }
    if u.contains("TACTICAL")
        || u.contains("ТАКТИЧ")
        || u.contains("FIGHTER")
        || u.contains("ВИНИЩУВАЧ")
        || u.contains("СУ-34")
        || u.contains("СУ-35")
        || u.contains("СУ-30")
        || u.contains("СУ-24")
        || u.contains("СУ-25")
        || u.contains("СУ-57")
        || u.contains("SU-34")
        || u.contains("SU-35")
        || u.contains("SU-30")
        || u.contains("SU-25")
        || u.contains("SU-24")
        || u.contains("SU-57")
    {
        return "tactical".to_string();
    }
    if u.contains("SHAHED") || u.contains("GERAN") || u.contains("UAV") || u.contains("БПЛА") {
        return "uav".to_string();
    }
    match raw {
        "SHAHED" => "uav".to_string(),
        "REACTIVE_SHAHED" => "uav_jet".to_string(),
        "FPV" => "fpv".to_string(),
        "scout" => "recon".to_string(),
        other => other.to_lowercase(),
    }
}

impl RawTarget {
    pub fn into_threat(self) -> Option<Threat> {
        let lat = self.lat.filter(|v| v.is_finite())?;
        let lon = self.lng.filter(|v| v.is_finite())?;
        let raw_type = self.target_type.clone().unwrap_or_default();
        let updated_at = self
            .last_seen
            .as_deref()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());
        let target_count = crate::models::extract_target_count(&raw_type, None);
        Some(Threat {
            id: format!("dimap:{}", self.id.unwrap_or_default()),
            source: "dimap".to_string(),
            threat_type: map_type(&raw_type),
            title: raw_type,
            lat,
            lon,
            heading: self.bearing.filter(|v| v.is_finite()),
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: "unknown".to_string(),
            source_count: 1,
            target_count,
            updated_at,
            area_only: false,
            advisory: false,
            trail: vec![],
        })
    }
}

pub async fn fetch(base_url: &str, enabled: bool) -> Result<Vec<Threat>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    let url = format!("{}/targets", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .header("Referer", "https://dimap.live/")
        .header("Origin", "https://dimap.live")
        .send()
        .await
        .map_err(|e| format!("dimap conn: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("dimap http {}", resp.status()));
    }
    let data: Response = resp.json().await.map_err(|e| format!("dimap json: {e}"))?;
    Ok(data.targets.into_iter().filter_map(|t| t.into_threat()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<Threat> {
        serde_json::from_str::<Response>(s)
            .unwrap()
            .targets
            .into_iter()
            .filter_map(|t| t.into_threat())
            .collect()
    }

    #[test]
    fn maps_dimap_types() {
        assert_eq!(map_type("SHAHED"), "uav");
        assert_eq!(map_type("REACTIVE_SHAHED"), "uav_jet");
        assert_eq!(map_type("FPV"), "fpv");
        assert_eq!(map_type("scout"), "recon");
        assert_eq!(map_type("Су-34"), "tactical");
    }

    #[test]
    fn parses_targets_and_skips_bad_coords() {
        let out = parse(
            r#"{"targets": [
            {"id": "w1", "type": "SHAHED", "lat": 50.0, "lng": 31.0, "bearing": 180},
            {"id": "w2", "type": "REACTIVE_SHAHED", "lat": 49.0, "lng": 32.0},
            {"id": "w3", "type": "FPV", "lat": null, "lng": 33.0},
            {"id": "w4", "type": "scout", "lat": 48.0}
        ]}"#,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "dimap:w1");
        assert_eq!(out[0].heading, Some(180.0));
        // сирий title зберігаємо — українську назву дає ua_label
        assert_eq!(out[1].title, "REACTIVE_SHAHED");
        assert_eq!(out[1].threat_type, "uav_jet");
    }

    #[test]
    fn disabled_returns_empty() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt.block_on(fetch("https://api.dimap.live/api", false)).unwrap();
        assert!(out.is_empty());
    }
}

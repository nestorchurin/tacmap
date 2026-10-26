//! Парсер ShahMap: відкритий REST, без ключів.
//! GET {base}/api/targets → {targets: [{id, type, lat, lng, bearing,
//!   speedKmh, confidence 0..1, status, currentPlace, destination,
//!   observedAt, sourceCount, ...}]}
//! Типи: GERAN3, UNKNOWN_UAV, ... — маппимо за ключовими словами.

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
    pub confidence: Option<f64>,
    #[allow(dead_code)]
    pub status: Option<String>,
    pub current_place: Option<String>,
    pub destination: Option<String>,
    pub observed_at: Option<String>,
    #[allow(dead_code)]
    pub source_count: Option<i32>,
}

pub fn map_type(raw: &str) -> String {
    let u = raw.to_uppercase();
    if u.contains("RECON") || u.contains("ORLAN") || u.contains("ZALA") || u.contains("ROZVID") {
        return "recon".to_string();
    }
    if u.contains("BALLISTIC") {
        return "ballistic".to_string();
    }
    if u.contains("MISSILE")
        || u.contains("ISKANDER")
        || u.contains("KALIBR")
        || u.contains("KH-")
        || u.contains("X-101")
        || u.contains("X-55")
        || u.contains("RAKETA")
    {
        return "missile".to_string();
    }
    if u.contains("KAB") {
        return "kab".to_string();
    }
    if u.contains("MIG") {
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
    if u.contains("FPV") {
        return "fpv".to_string();
    }
    if u.contains("BANDEROL") || u.contains("БАНДЕРОЛ") {
        return "uav".to_string();
    }
    if u.contains("REACTIVE") || u.contains("РЕАКТИВ") || u.contains("JET") {
        return "uav_jet".to_string();
    }
    if u.contains("UAV")
        || u.contains("SHAHED")
        || u.contains("GERAN")
        || u.contains("GERBERA")
        || u.contains("MOLNIYA")
        || u.contains("LANCET")
        || u.contains("DECOY")
        || u.contains("IMITATOR")
    {
        return "uav".to_string();
    }
    "unknown".to_string()
}

pub fn confidence_level(c: Option<f64>) -> String {
    match c.unwrap_or(0.0) {
        v if v >= 0.75 => "high".to_string(),
        v if v >= 0.4 => "medium".to_string(),
        _ => "low".to_string(),
    }
}

impl RawTarget {
    pub fn into_threat(self) -> Option<Threat> {
        let lat = self.lat.filter(|v| v.is_finite())?;
        let lon = self.lng.filter(|v| v.is_finite())?;
        let raw_type = self.target_type.clone().unwrap_or_default();
        let updated_at = self
            .observed_at
            .as_deref()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());
        let locality = self
            .current_place
            .filter(|s| !s.is_empty())
            .or(self.destination)
            .unwrap_or_default();
        let target_count = crate::models::extract_target_count(&raw_type, None);
        Some(Threat {
            id: format!("shahmap:{}", self.id.unwrap_or_default()),
            source: "shahmap".to_string(),
            threat_type: map_type(&raw_type),
            title: raw_type,
            lat,
            lon,
            heading: self.bearing.filter(|v| v.is_finite()),
            region: String::new(),
            district: String::new(),
            locality,
            confidence: confidence_level(self.confidence),
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
    let url = format!("{}/api/targets", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .header("Referer", "https://shahmap.online/")
        .send()
        .await
        .map_err(|e| format!("shahmap conn: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("shahmap http {}", resp.status()));
    }
    let data: Response = resp.json().await.map_err(|e| format!("shahmap json: {e}"))?;
    Ok(data.targets.into_iter().filter_map(|t| t.into_threat()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_types() {
        assert_eq!(map_type("GERAN3"), "uav");
        assert_eq!(map_type("UNKNOWN_UAV"), "uav");
        assert_eq!(map_type("ORLAN-10"), "recon");
        assert_eq!(map_type("KH-101"), "missile");
        assert_eq!(map_type("FPV"), "fpv");
        assert_eq!(map_type("???"), "unknown");
    }

    #[test]
    fn parses_targets() {
        let data: Response = serde_json::from_str(
            r#"{"targets": [
            {"id": "a", "type": "GERAN3", "lat": 47.7, "lng": 35.2, "bearing": 316.0,
             "confidence": 0.97, "currentPlace": "Григорівка", "destination": "Запоріжжя",
             "observedAt": "2026-10-11T18:25:24.000Z", "sourceCount": 1},
            {"id": "b", "type": "FPV", "lat": null, "lng": 35.0}
        ]}"#,
        )
        .unwrap();
        let out: Vec<Threat> = data.targets.into_iter().filter_map(|t| t.into_threat()).collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "shahmap:a");
        assert_eq!(out[0].threat_type, "uav");
        assert_eq!(out[0].confidence, "high");
        assert_eq!(out[0].locality, "Григорівка");
        assert_eq!(out[0].heading, Some(316.0));
    }
}

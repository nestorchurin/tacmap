//! Парсер Neptun: відкритий API, без ключів.
//! GET {base}/api/v1/threats — опитувати не частіше ніж раз на 5с.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::client;
use crate::models::Threat;

#[derive(Debug, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub threats: Vec<RawThreat>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawThreat {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub threat_type: Option<String>,
    pub title: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub heading: Option<f64>,
    pub region: Option<String>,
    pub district: Option<String>,
    pub locality: Option<String>,
    pub confidence_level: Option<String>,
    #[allow(dead_code)]
    pub source_count: Option<i32>,
    pub updated_at: Option<String>,
    pub area_only: Option<bool>,
    pub advisory: Option<bool>,
}

pub fn map_type(raw: &str, title: &str) -> String {
    let combined = format!("{} {}", raw, title).to_uppercase();
    if combined.contains("FPV") {
        return "fpv".to_string();
    }
    if combined.contains("RECON")
        || combined.contains("ORLAN")
        || combined.contains("ZALA")
        || combined.contains("SUPER")
        || combined.contains("РОЗВІД")
    {
        return "recon".to_string();
    }
    if combined.contains("BALLISTIC")
        || combined.contains("БАЛІСТ")
        || combined.contains("ІСКАНДЕР-М")
        || combined.contains("КИНЖАЛ")
    {
        return "ballistic".to_string();
    }
    if combined.contains("MISSILE")
        || combined.contains("KALIBR")
        || combined.contains("КАЛІБР")
        || combined.contains("РАКЕТ")
        || combined.contains("X-101")
        || combined.contains("Х-101")
        || combined.contains("Х-59")
        || combined.contains("Х-69")
    {
        return "missile".to_string();
    }
    if combined.contains("KAB") || combined.contains("КАБ") {
        return "kab".to_string();
    }
    if combined.contains("MIG") || combined.contains("МІГ") {
        return "mig31k".to_string();
    }
    if combined.contains("TACTICAL")
        || combined.contains("ТАКТИЧ")
        || combined.contains("FIGHTER")
        || combined.contains("ВИНИЩУВАЧ")
        || combined.contains("СУ-34")
        || combined.contains("СУ-35")
        || combined.contains("СУ-30")
        || combined.contains("СУ-24")
        || combined.contains("СУ-25")
        || combined.contains("СУ-57")
        || combined.contains("SU-34")
        || combined.contains("SU-35")
        || combined.contains("SU-30")
        || combined.contains("SU-25")
        || combined.contains("SU-24")
        || combined.contains("SU-57")
    {
        return "tactical".to_string();
    }
    if combined.contains("BANDEROL") || combined.contains("БАНДЕРОЛ") {
        return "uav".to_string();
    }
    if combined.contains("РЕАКТИВ") || combined.contains("REACTIVE") || combined.contains("JET") {
        return "uav_jet".to_string();
    }
    if combined.contains("UAV")
        || combined.contains("SHAHED")
        || combined.contains("ШАХЕД")
        || combined.contains("GERAN")
        || combined.contains("ГЕРАН")
        || combined.contains("DRONE")
        || combined.contains("ДРОН")
        || combined.contains("БПЛА")
    {
        return "uav".to_string();
    }
    raw.to_lowercase()
}

impl RawThreat {
    pub fn into_threat(self) -> Option<Threat> {
        let lat = self.lat.filter(|v| v.is_finite())?;
        let lon = self.lon.filter(|v| v.is_finite())?;
        let updated_at = self
            .updated_at
            .as_deref()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());
        let title = self.title.unwrap_or_default();
        let raw_type = self.threat_type.as_deref().unwrap_or("unknown");
        let threat_type = map_type(raw_type, &title);
        let target_count = crate::models::extract_target_count(&title, None);
        Some(Threat {
            id: format!("neptun:{}", self.id.unwrap_or_default()),
            source: "neptun".to_string(),
            threat_type,
            title,
            lat,
            lon,
            heading: self.heading.filter(|v| v.is_finite()),
            region: self.region.unwrap_or_default(),
            district: self.district.unwrap_or_default(),
            locality: self.locality.unwrap_or_default(),
            confidence: self
                .confidence_level
                .unwrap_or_else(|| "unknown".to_string()),
            source_count: 1,
            target_count,
            updated_at,
            area_only: self.area_only.unwrap_or(false),
            advisory: self.advisory.unwrap_or(false),
            trail: vec![],
        })
    }
}

pub async fn fetch(base_url: &str) -> Result<Vec<Threat>, String> {
    let url = format!("{}/api/v1/threats", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("neptun http {}", resp.status()));
    }
    let data: Response = resp.json().await.map_err(|e| e.to_string())?;
    Ok(data.threats.into_iter().filter_map(|t| t.into_threat()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<Threat> {
        serde_json::from_str::<Response>(s)
            .unwrap()
            .threats
            .into_iter()
            .filter_map(|t| t.into_threat())
            .collect()
    }

    #[test]
    fn parses_threats_and_skips_bad_coords() {
        let out = parse(
            r#"{"threats": [
            {"id": "a1", "type": "uav", "title": "БпЛА", "lat": 50.0, "lon": 31.0,
             "region": "Київська область", "confidenceLevel": "high", "sourceCount": 2},
            {"id": "a2", "type": "kab", "lat": 48.0, "lon": null},
            {"id": "a3", "type": "uav", "lat": 49.0}
        ]}"#,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "neptun:a1");
        assert_eq!(out[0].source_count, 1);
        assert_eq!(out[0].confidence, "high");
    }
}

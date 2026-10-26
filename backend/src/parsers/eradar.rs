//! Парсер єРадар (eradar.app): відкритий REST /api/dangers
//! GET {base}/dangers → {dangers: [{
//!   id, threat_type, lat, lng, canonical_name, message_excerpt, observed_at, confidence
//! }]}

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::client;
use crate::models::{in_theater, Threat};

#[derive(Debug, Deserialize)]
pub struct DangersResponse {
    #[serde(default)]
    pub dangers: Vec<RawDanger>,
}

#[derive(Debug, Deserialize)]
pub struct RawDanger {
    pub id: Option<serde_json::Value>,
    pub threat_type: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub canonical_name: Option<String>,
    pub message_excerpt: Option<String>,
    pub observed_at: Option<String>,
    pub confidence: Option<f64>,
}

pub fn map_type(threat_type: Option<&str>, excerpt: Option<&str>) -> String {
    let combined = format!(
        "{} {}",
        threat_type.unwrap_or_default(),
        excerpt.unwrap_or_default()
    ).to_lowercase();

    if combined.contains("jet") || combined.contains("реактив") {
        return "uav_jet".to_string();
    }
    if combined.contains("fpv") {
        return "fpv".to_string();
    }
    if combined.contains("recon") || combined.contains("орлан") || combined.contains("zala") || combined.contains("розвід") {
        return "recon".to_string();
    }
    if combined.contains("ballistic") || combined.contains("баліст") || combined.contains("іскандер") {
        return "ballistic".to_string();
    }
    if combined.contains("missile") || combined.contains("ракет") || combined.contains("калібр") || combined.contains("х-101") {
        return "missile".to_string();
    }
    if combined.contains("kab") || combined.contains("каб") || combined.contains("фаб") {
        return "kab".to_string();
    }
    if combined.contains("mig") || combined.contains("міг") || combined.contains("кинджал") {
        return "mig31k".to_string();
    }
    if combined.contains("fighter") || combined.contains("су-34") || combined.contains("су-35") || combined.contains("тактич") {
        return "tactical".to_string();
    }
    "uav".to_string()
}

pub async fn fetch(base_url: &str, enabled: bool) -> Result<Vec<Threat>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    let url = format!("{}/dangers", base_url.trim_end_matches('/'));
    let c = client()?;
    let resp = c.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("eradar http error {}", resp.status()));
    }
    let body = resp.json::<DangersResponse>().await.map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let now = Utc::now();
    for d in body.dangers {
        let lat = match d.lat {
            Some(l) => l,
            None => continue,
        };
        let lon = match d.lng {
            Some(l) => l,
            None => continue,
        };
        if !in_theater(lat, lon) {
            continue;
        }

        let id_str = match d.id {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s,
            _ => format!("eradar_{}_{}", (lat * 1000.0) as i64, (lon * 1000.0) as i64),
        };

        let threat_type = map_type(d.threat_type.as_deref(), d.message_excerpt.as_deref());
        let title = d.canonical_name.unwrap_or_else(|| "Ціль (єРадар)".to_string());

        let updated_at = d.observed_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc)))
            .unwrap_or(now);

        let conf_str = if d.confidence.unwrap_or(0.8) > 0.85 { "high" } else { "medium" };

        let target_count = crate::models::extract_target_count(&title, None);

        out.push(Threat {
            id: format!("eradar:{}", id_str),
            source: "eradar".to_string(),
            threat_type,
            title,
            lat,
            lon,
            heading: None,
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: conf_str.to_string(),
            source_count: 1,
            target_count,
            updated_at: Some(updated_at),
            area_only: false,
            advisory: false,
            trail: Vec::new(),
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_eradar_json() {
        let sample = r#"{
            "dangers": [
                {
                    "id": 40412,
                    "threat_type": "UAV",
                    "lat": 48.5,
                    "lng": 35.0,
                    "canonical_name": "Шахед у напрямку Дніпра",
                    "message_excerpt": "БпЛА в напрямку міста",
                    "observed_at": "2026-10-17T11:00:00Z",
                    "confidence": 0.95
                }
            ]
        }"#;
        let res: DangersResponse = serde_json::from_str(sample).unwrap();
        assert_eq!(res.dangers.len(), 1);
        let d = &res.dangers[0];
        assert_eq!(map_type(d.threat_type.as_deref(), d.message_excerpt.as_deref()), "uav");
    }
}

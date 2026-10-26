//! Парсер KUPOL (kupol.in.ua): відкритий REST, без ключів.
//! GET {base}/threats/active → {threats: [{
//!   id, provider, kind, labelUk, coordinates: [lon, lat],
//!   regionNameUk, headingDeg, speedKmh, updatedAt, noteUk
//! }]}

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::client;
use crate::models::{in_theater, Threat};

#[derive(Debug, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub threats: Vec<RawThreat>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawThreat {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub label_uk: Option<String>,
    pub coordinates: Option<Vec<f64>>,
    pub region_name_uk: Option<String>,
    pub heading_deg: Option<f64>,
    #[allow(dead_code)]
    pub speed_kmh: Option<f64>,
    pub updated_at: Option<String>,
    pub note_uk: Option<String>,
}

pub fn map_type(raw: &str) -> String {
    let u = raw.to_lowercase();
    if u.contains("jet") || u.contains("реактив") {
        return "uav_jet".to_string();
    }
    if u.contains("fpv") {
        return "fpv".to_string();
    }
    if u.contains("recon") || u.contains("orlan") || u.contains("zala") || u.contains("розвід") {
        return "recon".to_string();
    }
    if u.contains("ballistic") || u.contains("баліст") {
        return "ballistic".to_string();
    }
    if u.contains("missile") || u.contains("ракет") || u.contains("kalibr") || u.contains("х-") {
        return "missile".to_string();
    }
    if u.contains("kab") || u.contains("каб") {
        return "kab".to_string();
    }
    if u.contains("mig") || u.contains("міг") {
        return "mig31k".to_string();
    }
    if u.contains("tactical") || u.contains("тактич") || u.contains("fighter") {
        return "tactical".to_string();
    }
    "uav".to_string()
}

pub async fn fetch(base_url: &str, enabled: bool) -> Result<Vec<Threat>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    let url = format!("{}/threats/active", base_url.trim_end_matches('/'));
    let c = client()?;
    let resp = c.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("kupol http error {}", resp.status()));
    }
    let body = resp.json::<Response>().await.map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let now = Utc::now();
    for t in body.threats {
        let (lon, lat) = match t.coordinates {
            Some(ref coords) if coords.len() >= 2 => (coords[0], coords[1]),
            _ => continue,
        };
        if !in_theater(lat, lon) {
            continue;
        }

        let id = t.id.unwrap_or_else(|| format!("kupol_{}_{}", (lat * 1000.0) as i64, (lon * 1000.0) as i64));
        let raw_kind = t.kind.unwrap_or_else(|| "uav".to_string());
        let threat_type = map_type(&raw_kind);

        let title = t.label_uk.or(t.note_uk).unwrap_or_else(|| "БпЛА (Купол)".to_string());
        let region = t.region_name_uk.unwrap_or_default();

        let updated_at = t.updated_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc)))
            .unwrap_or(now);

        let target_count = crate::models::extract_target_count(&title, None);

        out.push(Threat {
            id: format!("kupol:{}", id),
            source: "kupol".to_string(),
            threat_type,
            title,
            lat,
            lon,
            heading: t.heading_deg,
            region,
            district: String::new(),
            locality: String::new(),
            confidence: "high".to_string(),
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
    fn parses_kupol_json() {
        let sample = r#"{
            "threats": [
                {
                    "id": "trk_101",
                    "kind": "uav_jet",
                    "labelUk": "Реактивний дрон",
                    "coordinates": [33.5, 50.8],
                    "regionNameUk": "Сумська",
                    "headingDeg": 225.0,
                    "updatedAt": "2026-10-17T11:00:00Z"
                }
            ]
        }"#;
        let res: Response = serde_json::from_str(sample).unwrap();
        assert_eq!(res.threats.len(), 1);
        let t = &res.threats[0];
        assert_eq!(t.id.as_deref(), Some("trk_101"));
        assert_eq!(map_type(t.kind.as_deref().unwrap_or("")), "uav_jet");
    }
}

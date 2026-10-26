//! Парсер RadarUA: відкритий REST, без ключів.
//! GET {base}/api/tracks → {tracks: [{id, threatType, lat, lng, place,
//!   region, placeApprox, count, courseBearing, confidence 0..1,
//!   sources[], updatedAt, trajectory [[lat,lng],...]}]}
//! Поруч лежать /api/alerts (зони тривог) і /api/ballistic — тривоги
//! беремо з Neptun (є ключі полігонів), сюди не тягнемо.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::client;
use crate::models::Threat;

#[derive(Debug, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub tracks: Vec<RawTrack>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTrack {
    pub id: Option<String>,
    pub threat_type: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub place: Option<String>,
    pub region: Option<String>,
    pub place_approx: Option<bool>,
    pub course_bearing: Option<f64>,
    pub count: Option<i32>,
    pub confidence: Option<f64>,
    #[allow(dead_code)]
    pub sources: Option<Vec<String>>,
    pub updated_at: Option<String>,
    pub last_seen_at: Option<String>,
}

pub fn map_type(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("reac") || low.contains("реактив") || low.contains("jet") {
        return "uav_jet".to_string();
    }
    if low.contains("banderol") || low.contains("бандерол") {
        return "uav".to_string();
    }
    if low.contains("tactical")
        || low.contains("тактич")
        || low.contains("fighter")
        || low.contains("винищувач")
        || low.contains("су-34")
        || low.contains("су-35")
        || low.contains("су-30")
        || low.contains("су-24")
        || low.contains("су-25")
        || low.contains("su-34")
        || low.contains("su-35")
        || low.contains("su-30")
    {
        return "tactical".to_string();
    }
    match low.as_str() {
        "shahed" | "geran" | "uav" => "uav".to_string(),
        "fpv" => "fpv".to_string(),
        "recon" => "recon".to_string(),
        "missile" => "missile".to_string(),
        "ballistic" => "ballistic".to_string(),
        "kab" => "kab".to_string(),
        "mig31k" => "mig31k".to_string(),
        _ => "unknown".to_string(),
    }
}

pub fn confidence_level(c: Option<f64>) -> String {
    match c.unwrap_or(0.0) {
        v if v >= 0.75 => "high".to_string(),
        v if v >= 0.4 => "medium".to_string(),
        _ => "low".to_string(),
    }
}

fn parse_dt(s: Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref().and_then(|v| v.parse().ok())
}

impl RawTrack {
    pub fn into_threat(self) -> Option<Threat> {
        let lat = self.lat.filter(|v| v.is_finite())?;
        let lon = self.lng.filter(|v| v.is_finite())?;
        let raw_type = self.threat_type.clone().unwrap_or_default();
        let target_count = crate::models::extract_target_count(&raw_type, self.count);
        Some(Threat {
            id: format!("radarua:{}", self.id.unwrap_or_default()),
            source: "radarua".to_string(),
            threat_type: map_type(&raw_type),
            title: raw_type,
            lat,
            lon,
            heading: self.course_bearing.filter(|v| v.is_finite()),
            region: self.region.unwrap_or_default(),
            district: String::new(),
            locality: self.place.unwrap_or_default(),
            confidence: confidence_level(self.confidence),
            source_count: 1,
            target_count,
            updated_at: parse_dt(self.updated_at).or_else(|| parse_dt(self.last_seen_at)),
            area_only: self.place_approx.unwrap_or(false),
            advisory: false,
            trail: vec![],
        })
    }
}

pub async fn fetch(base_url: &str, enabled: bool) -> Result<Vec<Threat>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    let url = format!("{}/api/tracks", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .header("Referer", "https://radarua.com/")
        .send()
        .await
        .map_err(|e| format!("radarua conn: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("radarua http {}", resp.status()));
    }
    let data: Response = resp.json().await.map_err(|e| format!("radarua json: {e}"))?;
    Ok(data.tracks.into_iter().filter_map(|t| t.into_threat()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracks() {
        let data: Response = serde_json::from_str(
            r#"{"tracks": [
            {"id": "t1", "threatType": "shahed", "lat": 52.089, "lng": 31.7804,
             "place": "Сеньківка", "region": "Чернігівська", "placeApprox": false,
             "courseBearing": 180, "confidence": 0.95,
             "sources": ["a", "b", "c"], "updatedAt": "2026-10-11T18:28:54.679Z",
             "trajectory": [[51.89, 31.59], [52.089, 31.7804]]},
            {"id": "t2", "threatType": "fpv", "lat": null, "lng": 30.0}
        ]}"#,
        )
        .unwrap();
        let out: Vec<Threat> = data.tracks.into_iter().filter_map(|t| t.into_threat()).collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "radarua:t1");
        assert_eq!(out[0].threat_type, "uav");
        assert_eq!(out[0].locality, "Сеньківка");
        assert_eq!(out[0].source_count, 1);
        assert_eq!(out[0].heading, Some(180.0));
    }
}

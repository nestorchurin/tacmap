//! Парсер aviacontrol.com.ua: відкритий REST, без ключів.
//! GET {base}/api/targets → {ok, rev, server_time (мс), targets: [{
//!   id, type, label (українською), color, count, region,
//!   pos [lat, lon], bearing, speed_kmh, route [[lat, lon], ...],
//!   t0/created/updated (мс), life_min, ...}]}

use chrono::{TimeZone, Utc};
use serde::Deserialize;

use super::client;
use crate::models::Threat;

#[derive(Debug, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub targets: Vec<RawTarget>,
}

#[derive(Debug, Deserialize)]
pub struct RawTarget {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub target_type: Option<String>,
    pub label: Option<String>,
    pub region: Option<String>,
    pub pos: Option<Vec<f64>>,
    pub bearing: Option<f64>,
    pub route: Option<Vec<Vec<f64>>>,
    pub updated: Option<i64>,
    pub count: Option<i32>,
}

pub fn map_type(raw: &str) -> String {
    let u = raw.to_uppercase();
    if u.contains("FPV") {
        return "fpv".to_string();
    }
    if u.contains("RECON") || u.contains("ORLAN") || u.contains("ZALA") {
        return "recon".to_string();
    }
    if u.contains("BALLISTIC") {
        return "ballistic".to_string();
    }
    if u.contains("MISSILE") || u.contains("KALIBR") || u.contains("KH-") || u.contains("X-101") {
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
    if u.contains("BANDEROL") || u.contains("БАНДЕРОЛ") {
        return "uav".to_string();
    }
    if u.contains("JET") || u.contains("РЕАКТИВ") {
        return "uav_jet".to_string();
    }
    if u.contains("UAV") || u.contains("SHAHED") || u.contains("GERAN") {
        return "uav".to_string();
    }
    "unknown".to_string()
}

impl RawTarget {
    pub fn into_threat(self) -> Option<Threat> {
        let p = self.pos?;
        if p.len() < 2 || !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let raw_type = self.target_type.clone().unwrap_or_default();
        let base_title = self.label.filter(|s| !s.is_empty()).unwrap_or(raw_type.clone());
        let title = if let Some(c) = self.count.filter(|&c| c > 1) {
            format!("{} (група ×{})", base_title, c)
        } else {
            base_title
        };
        let mut trail: Vec<[f64; 2]> = self
            .route
            .unwrap_or_default()
            .iter()
            .filter_map(|q| {
                if q.len() >= 2 && q[0].is_finite() && q[1].is_finite() {
                    Some([q[0], q[1]]) // вже [lat, lon]
                } else {
                    None
                }
            })
            .collect();
        if trail.len() > 8 {
            trail = trail[trail.len() - 8..].to_vec(); // останні 8 точок
        }
        let target_count = crate::models::extract_target_count(&title, self.count);
        Some(Threat {
            id: format!("aviacontrol:{}", self.id.unwrap_or_default()),
            source: "aviacontrol".to_string(),
            threat_type: map_type(&raw_type),
            title,
            lat: p[0],
            lon: p[1],
            heading: self.bearing.filter(|v| v.is_finite()),
            region: self.region.unwrap_or_default(),
            district: String::new(),
            locality: String::new(),
            confidence: "medium".to_string(),
            source_count: 1,
            target_count,
            updated_at: self.updated.and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
            area_only: false,
            advisory: false,
            trail,
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
        .header("Referer", "https://aviacontrol.com.ua/")
        .send()
        .await
        .map_err(|e| format!("aviacontrol conn: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("aviacontrol http {}", resp.status()));
    }
    let data: Response = resp.json().await.map_err(|e| format!("aviacontrol json: {e}"))?;
    Ok(data.targets.into_iter().filter_map(|t| t.into_threat()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        let data: Response = serde_json::from_str(
            r#"{"ok": true, "targets": [
            {"id": "ta_1", "type": "jet_uav", "label": "Реактивний БПЛА",
             "region": "Сумська", "pos": [51.0, 33.0], "bearing": 135.2,
             "route": [[51.2, 32.8], [51.0, 33.0]], "updated": 1792085241898, "count": 1},
            {"id": "ta_2", "type": "aircraft", "label": "Тактична авіація",
             "pos": [46.6, 35.6]},
            {"id": "ta_3", "type": "x", "pos": [10.0]}
        ]}"#,
        )
        .unwrap();
        let out: Vec<Threat> = data.targets.into_iter().filter_map(|t| t.into_threat()).collect();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "aviacontrol:ta_1");
        assert_eq!(out[0].threat_type, "uav_jet");
        assert_eq!(out[0].heading, Some(135.2));
        assert_eq!(out[0].trail, vec![[51.2, 32.8], [51.0, 33.0]]);
        assert!(out[0].updated_at.is_some());
        // aircraft без маппинга, але український label зберігається в title
        assert_eq!(out[1].threat_type, "unknown");
        assert_eq!(out[1].title, "Тактична авіація");
    }
}

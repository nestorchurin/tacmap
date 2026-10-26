//! Атрибуція пусків + статичні коридори до кордону.
//! Ймовірний пуск: сайт позаду цілі по зворотному курсу.
//! Евристика, НЕ точний прогноз: швидкості крейсерські, кордон спрощений.

use crate::borders::{bearing_deg, haversine_km, nearest_border_point};
use crate::launches::LaunchSite;
use crate::models::Threat;

/// Швидкість для статичного коридору за видом пуску.
pub fn launch_cruise_kmh(kind: &str) -> f64 {
    match kind {
        "uav" => 180.0,
        "missile" => 850.0,
        "mig31k" => 2500.0,
        "ballistic" => 4500.0,
        "naval" => 850.0,
        _ => 400.0,
    }
}

/// Сумісні види пусків для типу цілі.
pub fn compatible_kinds(threat_type: &str) -> &'static [&'static str] {
    match threat_type {
        "uav" | "uav_jet" | "fpv" | "recon" => &["uav"],
        "missile" => &["missile", "naval"],
        "ballistic" => &["ballistic", "naval"],
        "mig31k" => &["mig31k"],
        "tactical" => &["mig31k", "missile"],
        // КАБи — прифронтові, зі статичних аеродромів не атрибутуються
        "kab" => &[],
        _ => &["uav", "missile", "mig31k", "ballistic", "naval"],
    }
}

fn ang_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

/// Курс цілі: свій heading або похідна з trail (останній сегмент).
pub fn effective_heading(t: &Threat) -> Option<f64> {
    if let Some(h) = t.heading {
        if h.is_finite() {
            return Some(h);
        }
    }
    if let Some(last) = t.trail.last() {
        if last[0].is_finite() && last[1].is_finite() {
            return Some(bearing_deg(last[0], last[1], t.lat, t.lon));
        }
    }
    None
}

/// Ймовірний пуск: сайт позаду по зворотному курсу (конус 30°, дальність <900 км).
/// Повертає індекс сайта. КАБи і area_only не атрибутуються.
pub fn attribute_launch(t: &Threat, heading: f64, sites: &[LaunchSite]) -> Option<usize> {
    if t.area_only {
        return None;
    }
    let compat = compatible_kinds(&t.threat_type);
    if compat.is_empty() {
        return None;
    }
    let mut best: Option<(usize, f64, f64)> = None; // (idx, ang_diff, dist)
    for (i, s) in sites.iter().enumerate() {
        if !compat.contains(&s.kind.as_str()) {
            continue;
        }
        let dist = haversine_km(s.lat, s.lon, t.lat, t.lon);
        if dist > 900.0 {
            continue;
        }
        let to_threat = bearing_deg(s.lat, s.lon, t.lat, t.lon);
        let ad = ang_diff(to_threat, heading);
        if ad > 30.0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, bad, bdist)) => ad < bad - 1e-9 || (ad - bad).abs() < 1e-9 && dist < bdist,
        };
        if better {
            best = Some((i, ad, dist));
        }
    }
    best.map(|(i, _, _)| i)
}

/// Статичні коридори: кожен пуск → найближча точка кордону + ETA.
#[derive(Debug, Clone)]
pub struct Corridor {
    pub launch_idx: usize,
    pub border_lat: f64,
    pub border_lon: f64,
    pub dist_km: f64,
    pub eta_min: f64,
}

/// Статичні коридори: кожен пуск → найближча точка кордону + ETA.
pub fn build_corridors(sites: &[LaunchSite]) -> Vec<Corridor> {
    sites
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let n = nearest_border_point(s.lat, s.lon);
            Corridor {
                launch_idx: i,
                border_lat: n[0],
                border_lon: n[1],
                dist_km: n[2],
                eta_min: n[2] / launch_cruise_kmh(&s.kind) * 60.0,
            }
        })
        .collect()
}

pub fn fmt_eta(min: f64) -> String {
    if !min.is_finite() {
        return "?".to_string();
    }
    if min < 1.0 {
        return "<1 хв".to_string();
    }
    if min < 60.0 {
        return format!("~{} хв", min.round() as i64);
    }
    let h = (min / 60.0).floor() as i64;
    let m = (min % 60.0).round() as i64;
    if m == 0 {
        format!("~{h} год")
    } else {
        format!("~{h} год {m} хв")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launches::all_sites;
    use chrono::Utc;

    fn mk(id: &str, typ: &str, lat: f64, lon: f64, heading: Option<f64>) -> Threat {
        Threat {
            id: id.to_string(),
            source: "dimap".to_string(),
            threat_type: typ.to_string(),
            title: String::new(),
            lat,
            lon,
            heading,
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: "medium".to_string(),
            source_count: 1,
            target_count: 1,
            updated_at: Some(Utc::now()),
            area_only: false,
            advisory: false,
            trail: vec![],
        }
    }

    #[test]
    fn attributes_shahed_to_primorsko() {
        let sites = all_sites();
        // шахед над морем південніше Запоріжжя, курс на північний захід —
        // позаду Приморсько-Ахтарськ
        let t = mk("x", "uav", 46.8, 35.5, Some(320.0));
        let idx = attribute_launch(&t, 320.0, &sites).expect("має атрибутуватися");
        let back = bearing_deg(sites[idx].lat, sites[idx].lon, t.lat, t.lon);
        assert!(ang_diff(back, 320.0) <= 30.0);
    }

    #[test]
    fn kab_never_attributed() {
        let sites = all_sites();
        let t = mk("k", "kab", 49.0, 36.0, Some(270.0));
        assert!(attribute_launch(&t, 270.0, &sites).is_none());
    }

    #[test]
    fn heading_from_trail() {
        let mut t = mk("t", "uav", 50.0, 31.0, None);
        t.trail = vec![[49.9, 31.0]];
        let h = effective_heading(&t).unwrap();
        assert!((h - 0.0).abs() < 3.0 || (h - 360.0).abs() < 3.0, "{h}");
    }

    #[test]
    fn corridors_cover_all_sites() {
        let sites = all_sites();
        let c = build_corridors(&sites);
        assert_eq!(c.len(), sites.len());
        for k in c {
            // Оленья — далекий рубіж (~1700 км), решта < 900 км
            assert!(k.dist_km >= 0.0 && k.dist_km < 2000.0, "{}", k.dist_km);
            assert!(k.eta_min >= 0.0, "{}", k.eta_min);
        }
    }

    #[test]
    fn fmt_eta_units() {
        assert_eq!(fmt_eta(0.4), "<1 хв");
        assert_eq!(fmt_eta(24.0), "~24 хв");
        assert_eq!(fmt_eta(90.0), "~1 год 30 хв");
    }
}

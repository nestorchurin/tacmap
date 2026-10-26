//! Дедуп до трекера: прибирає дублікати, які консенсус не ловить.
//!
//! Два випадки:
//! 1. Фрагменти одного трека: DIMAP плодить `ws_17431`, `ws_17431_2`, … —
//!    один фізичний борт під різними ID. Стем = ID без хвоста `_\d+`.
//! 2. Точні клони: те саме джерело, той самий тип, дистанція < 1 км
//!    (напр. два radarua-треки в одній точці, 0.0 км).
//!
//! В обох випадках лишаємо свіжіший (потім — з більшим source_count).
//! Крос-тип і далеких не чіпаємо: різний тип поруч = різні об'єкти,
//! далеких зливає консенсус на показі.

use std::collections::HashMap;

use crate::borders::haversine_km;
use crate::models::Threat;

/// Стем ID: відрізає хвіст `_число` тільки якщо основа ТЕЖ закінчується
/// на `_число` (ws_17431_2 → ws_17431 — фрагмент трека).
/// Звичайні ID з цифрами (trk_93283) не чіпаємо.
pub fn stem(id: &str) -> &str {
    let is_num = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if let Some(i) = id.rfind('_') {
        let (base, tail) = (&id[..i], &id[i + 1..]);
        if is_num(tail) {
            if let Some(j) = base.rfind('_') {
                if is_num(&base[j + 1..]) {
                    return base;
                }
            }
        }
    }
    id
}

fn fresher(a: &Threat, b: &Threat) -> bool {
    match (a.updated_at, b.updated_at) {
        (Some(x), Some(y)) if x != y => x > y,
        _ => a.source_count >= b.source_count,
    }
}

/// Повертає (очищені, кількість_викинутих).
pub fn dedup_threats(threats: Vec<Threat>) -> (Vec<Threat>, usize) {
    let initial = threats.len();
    // Крок 1: групування за стемом (джерело + тип + стем).
    let mut by_stem: HashMap<(String, String, String), Threat> = HashMap::new();
    for t in threats {
        let key = (t.source.clone(), t.threat_type.clone(), stem(&t.id).to_string());
        match by_stem.get(&key) {
            Some(old) if fresher(old, &t) => {}
            _ => {
                by_stem.insert(key, t);
            }
        }
    }
    let mut out: Vec<Threat> = by_stem.into_values().collect();
    // Крок 2: суб-км клони (те саме джерело+тип, < 1 км).
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let mut keep = vec![true; out.len()];
    for i in 0..out.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..out.len() {
            if !keep[j]
                || out[i].source != out[j].source
                || out[i].threat_type != out[j].threat_type
            {
                continue;
            }
            if haversine_km(out[i].lat, out[i].lon, out[j].lat, out[j].lon) < 1.0 {
                if fresher(&out[i], &out[j]) {
                    keep[j] = false;
                } else {
                    keep[i] = false;
                    break;
                }
            }
        }
    }
    let mut res = vec![];
    for (t, k) in out.into_iter().zip(keep) {
        if k {
            res.push(t);
        }
    }
    let dropped_total = initial.saturating_sub(res.len());
    (res, dropped_total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn mk(id: &str, source: &str, lat: f64, lon: f64, secs: i64) -> Threat {
        Threat {
            id: id.to_string(),
            source: source.to_string(),
            threat_type: "uav".to_string(),
            title: String::new(),
            lat,
            lon,
            heading: None,
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: "medium".to_string(),
            source_count: 1,
            target_count: 1,
            updated_at: Some(Utc.timestamp_opt(secs, 0).single().unwrap()),
            area_only: false,
            advisory: false,
            trail: vec![],
        }
    }

    #[test]
    fn stem_strips_numeric_tail() {
        assert_eq!(stem("dimap:ws_17431_2"), "dimap:ws_17431");
        assert_eq!(stem("dimap:ws_17431"), "dimap:ws_17431");
        assert_eq!(stem("radarua:trk_93283"), "radarua:trk_93283");
        assert_eq!(stem("a_b_c"), "a_b_c");
    }

    #[test]
    fn merges_fragments_keeping_freshest() {
        let ts = vec![
            mk("dimap:ws_1", "dimap", 50.0, 31.0, 100),
            mk("dimap:ws_1_2", "dimap", 50.05, 31.0, 200),
        ];
        let (out, _) = dedup_threats(ts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "dimap:ws_1_2");
    }

    #[test]
    fn merges_exact_clones() {
        let ts = vec![
            mk("radarua:trk_1", "radarua", 52.089, 31.7804, 100),
            mk("radarua:trk_2", "radarua", 52.089, 31.7804, 200),
        ];
        let (out, dropped) = dedup_threats(ts);
        assert_eq!(out.len(), 1);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn keeps_distinct_and_cross_type() {
        let a = mk("x:1", "dimap", 50.0, 31.0, 100);
        let mut b = mk("x:2", "dimap", 50.0, 31.0, 100);
        b.threat_type = "fpv".to_string();
        let c = mk("y:1", "dimap", 51.0, 31.0, 100);
        let (out, _) = dedup_threats(vec![a, b, c]);
        assert_eq!(out.len(), 3);
    }
}

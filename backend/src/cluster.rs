//! Консенсус-кластеринг: мітки одного типу поруч = одна подія.
//! Карта А каже шахед + карта Б каже шахед поруч
//! → один кластер з sources=["dimap","neptun"].
//!
//! Правила злиття (дві мітки — один об'єкт):
//! - один тип (шахед ≠ ракета);
//! - дистанція ≤ радіуса типу (точні КАБ/FPV — вузко, швидкі ракети — широко);
//! - курси не суперечать (розбіжність > 60° = різні борти, що розходяться).
//!
//! Алгоритм — union-find: не залежить від порядку, транзитивне замикання.
//! Центр — зважений за source_count (краще підтверджені тягнуть сильніше).
//! ID стабільний (хеш складу) — бейдж не мерехтить між опитуваннями.
//! Геоматематика — з borders.rs (єдине місце).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::borders::haversine_km;
use crate::models::{ua_label, Cluster, Threat};

/// Радіус злиття по типу, км.
pub fn radius_for(threat_type: &str) -> f64 {
    match threat_type {
        "fpv" => 15.0,      // прифронтові
        "kab" => 25.0,      // скидання КАБ
        "uav" | "uav_jet" => 55.0, // шахеди та далекобійні БпЛА
        "recon" => 55.0,
        "missile" => 80.0,  // швидкі (800+ км/год)
        "ballistic" => 120.0,
        "tactical" => 120.0, // винищувачі/бомбардувальники (~900-1500 км/год)
        "mig31k" => 200.0,
        _ => 45.0,
    }
}

/// Максимальна розбіжність курсів для одного об'єкта, градуси.
pub const MAX_HEADING_DIFF_DEG: f64 = 110.0;

/// Сумісність типів цілей між різними джерелами (напр. БпЛА і Розвідник часто плутають).
pub fn are_types_compatible(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let is_uav_a = a == "uav" || a == "uav_jet";
    let is_uav_b = b == "uav" || b == "uav_jet";
    if is_uav_a && is_uav_b {
        return true;
    }
    if (is_uav_a && b == "recon") || (a == "recon" && is_uav_b) {
        return true;
    }
    if (a == "missile" && b == "ballistic") || (a == "ballistic" && b == "missile") {
        return true;
    }
    if (a == "tactical" && b == "mig31k") || (a == "mig31k" && b == "tactical") {
        return true;
    }
    // Невідомі цілі не зливаються з БпЛА чи авіацією
    false
}

/// Мінімальна кутова різниця 0..180.
fn heading_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

fn can_merge(a: &Threat, b: &Threat) -> bool {
    // Одне й те саме джерело дає дві окремі цілі далі 5 км — це два різних фізичних борти, їх не можна зливати!
    if a.source == b.source && a.id != b.id {
        let dist = haversine_km(a.lat, a.lon, b.lat, b.lon);
        if dist > 5.0 {
            return false;
        }
    }

    if !are_types_compatible(&a.threat_type, &b.threat_type) {
        return false;
    }
    let dist = haversine_km(a.lat, a.lon, b.lat, b.lon);
    let r = radius_for(&a.threat_type).max(radius_for(&b.threat_type));
    if dist > r {
        return false;
    }
    // Якщо дуже близько (<= 25 км) — зливаємо, крім випадку абсолютно зустрічних курсів (> 150°)
    // Якщо далі 25 км — перевіряємо, щоб курси не розходилися більше ніж на MAX_HEADING_DIFF_DEG (110°)
    if let (Some(ha), Some(hb)) = (a.heading, b.heading) {
        let diff = heading_diff(ha, hb);
        if dist > 25.0 {
            if diff > MAX_HEADING_DIFF_DEG {
                return false;
            }
        } else if diff > 150.0 {
            return false;
        }
    }
    true
}

fn stable_id(member_ids: &[String]) -> String {
    let mut ids = member_ids.to_vec();
    ids.sort();
    let mut h = DefaultHasher::new();
    ids.hash(&mut h);
    format!("c{:x}", h.finish())
}

fn weight(t: &Threat) -> f64 {
    (t.source_count.max(1)) as f64
}

pub fn cluster_threats(threats: &[Threat]) -> Vec<Cluster> {
    let mut groups: Vec<Vec<Threat>> = Vec::new();
    let mut pts: Vec<Threat> = Vec::new();
    for t in threats {
        if t.area_only {
            // Злиття орієнтовних засічок однієї області або в радіусі 40 км
            let mut merged = false;
            for g in &mut groups {
                if g[0].area_only
                    && (!t.region.is_empty() && g[0].region == t.region
                        || haversine_km(g[0].lat, g[0].lon, t.lat, t.lon) < 40.0)
                {
                    g.push(t.clone());
                    merged = true;
                    break;
                }
            }
            if !merged {
                groups.push(vec![t.clone()]);
            }
        } else {
            pts.push(t.clone());
        }
    }

    // Повний кластеринг без ланцюгового розтягування (complete linkage):
    // Зливаємо пари від найближчих до найвіддаленіших.
    // Дві групи об'єднуються ТІЛЬКИ якщо ВСІ елементи між ними сумісні
    // і попарна відстань між будь-якими двома елементами не перевищує радіус.
    let n = pts.len();
    let mut cluster_of: Vec<usize> = (0..n).collect();
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

    // Збираємо всі валідні пари з відстанями
    let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            if can_merge(&pts[i], &pts[j]) {
                let dist = haversine_km(pts[i].lat, pts[i].lon, pts[j].lat, pts[j].lon);
                pairs.push((i, j, dist));
            }
        }
    }
    // Сортуємо: спершу найближчі
    pairs.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    for (i, j, _) in pairs {
        let ci = cluster_of[i];
        let cj = cluster_of[j];
        if ci == cj {
            continue;
        }

        // Перевіряємо complete linkage: всі точки ci повинні зливатися з усіма точками cj
        let can_combine = clusters[ci].iter().all(|&u| {
            clusters[cj].iter().all(|&v| can_merge(&pts[u], &pts[v]))
        });

        if can_combine {
            let moved = clusters[cj].clone();
            for &m in &moved {
                cluster_of[m] = ci;
            }
            clusters[ci].extend(moved);
            clusters[cj].clear();
        }
    }

    let mut ordered: Vec<Vec<Threat>> = clusters
        .into_iter()
        .filter(|c| !c.is_empty())
        .map(|c| c.into_iter().map(|idx| pts[idx].clone()).collect())
        .collect();

    ordered.sort_by(|a, b| {
        let ka = a.iter().map(|t| &t.id).min();
        let kb = b.iter().map(|t| &t.id).min();
        ka.cmp(&kb)
    });
    groups.extend(ordered);

    groups
        .into_iter()
        .map(|members| {
            let wsum: f64 = members.iter().map(weight).sum();
            let lat = members.iter().map(|m| m.lat * weight(m)).sum::<f64>() / wsum;
            let lon = members.iter().map(|m| m.lon * weight(m)).sum::<f64>() / wsum;
            let mut sources: Vec<String> = members.iter().map(|m| m.source.clone()).collect();
            sources.sort();
            sources.dedup();
            // розкид групи: макс попарна дистанція
            let mut spread: f64 = 0.0;
            for (i, a) in members.iter().enumerate() {
                for b in members.iter().skip(i + 1) {
                    spread = spread.max(haversine_km(a.lat, a.lon, b.lat, b.lon));
                }
            }
            // Кількість джерел — це виключно кількість УНІКАЛЬНИХ зовнішніх платформ
            // з нашого пулу (neptun, dimap, shahmap, radarua, aviacontrol).
            // Вона ніколи не перевищує загальну кількість джерел (максимум 5).
            let total_sources: usize = sources.len();
            let confidence = match total_sources {
                0..=1 => "low",
                2 => "medium",
                _ => "high",
            };
            // Вибір найінформативнішого члена для підпису
            let best_member = members
                .iter()
                .max_by_key(|m| {
                    let mut score = 0;
                    if !m.title.is_empty() && m.title != "БпЛА" && m.title != "UNKNOWN_UAV" {
                        score += 10;
                    }
                    if !m.locality.is_empty() {
                        score += 5;
                    }
                    if !m.region.is_empty() {
                        score += 2;
                    }
                    if m.heading.is_some() {
                        score += 2;
                    }
                    score
                })
                .unwrap_or(&members[0]);

            let is_reactive = members.iter().any(|m| {
                m.threat_type == "uav_jet"
                    || m.title.to_lowercase().contains("reac")
                    || m.title.to_lowercase().contains("реактив")
                    || m.title.to_lowercase().contains("jet")
            });

            let threat_type = if is_reactive {
                "uav_jet".to_string()
            } else if members.iter().any(|m| m.threat_type == "ballistic") {
                "ballistic".to_string()
            } else if members.iter().any(|m| m.threat_type == "missile") {
                "missile".to_string()
            } else if members.iter().any(|m| m.threat_type == "mig31k") {
                "mig31k".to_string()
            } else if members.iter().any(|m| m.threat_type == "tactical") {
                "tactical".to_string()
            } else if members.iter().any(|m| m.threat_type == "kab") {
                "kab".to_string()
            } else if members.iter().any(|m| m.threat_type == "uav") {
                "uav".to_string()
            } else if members.iter().any(|m| m.threat_type == "recon") {
                "recon".to_string()
            } else if members.iter().any(|m| m.threat_type == "fpv") {
                "fpv".to_string()
            } else {
                best_member.threat_type.clone()
            };

            // Підтвердження (консенсус 3+ джерел) застосовується до Шахедів та ракет.
            // FPV та інші тактичні цілі парсяться з одного джерела і не мають підтвердження.
            let is_confirmable = matches!(threat_type.as_str(), "uav" | "uav_jet" | "missile" | "ballistic");
            let confirmed = is_confirmable && total_sources >= 3;

            // Консенсусний курс (кругове усереднення кутів)
            let headings: Vec<f64> = members.iter().filter_map(|m| m.heading).collect();
            let heading = if headings.is_empty() {
                None
            } else {
                let mut sin_sum = 0.0;
                let mut cos_sum = 0.0;
                for h in &headings {
                    let rad = h.to_radians();
                    sin_sum += rad.sin();
                    cos_sum += rad.cos();
                }
                let avg_rad = sin_sum.atan2(cos_sum);
                let mut deg = avg_rad.to_degrees();
                if deg < 0.0 {
                    deg += 360.0;
                }
                Some((deg * 10.0).round() / 10.0)
            };

            // Найповніша історія треку
            let trail = members
                .iter()
                .max_by_key(|m| m.trail.len())
                .map(|m| m.trail.clone())
                .unwrap_or_default();

            let title = if best_member.title.is_empty() {
                members[0].title.clone()
            } else {
                best_member.title.clone()
            };
            let label = ua_label(&threat_type, &title);
            let region = best_member.region.clone();
            let locality = best_member.locality.clone();
            let agreement = if !is_confirmable {
                format!("1 джерело ({}) · прямий канал", sources.first().cloned().unwrap_or_else(|| "radar".to_string()))
            } else if confirmed {
                format!("×{} джерел ({}) · розкид {:.1} км", total_sources, sources.join(", "), spread)
            } else if total_sources == 2 {
                format!("2 джерела ({}) · очікує 3-го підтвердження", sources.join(", "))
            } else {
                format!("1 джерело ({}) · очікує підтвердження", sources.first().cloned().unwrap_or_else(|| "radar".to_string()))
            };

            let target_count = members
                .iter()
                .map(|m| m.target_count)
                .max()
                .unwrap_or(1)
                .max(1);

            let ids: Vec<String> = members.iter().map(|m| m.id.clone()).collect();
            Cluster {
                id: stable_id(&ids),
                lat,
                lon,
                threat_type,
                title,
                label,
                region,
                locality,
                heading,
                trail,
                members,
                agreement,
                sources,
                spread_km: (spread * 10.0).round() / 10.0,
                confidence: confidence.to_string(),
                confirmed,
                target_count,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn mk(id: &str, source: &str, lat: f64, lon: f64) -> Threat {
        mk_full(id, source, "uav", lat, lon, None, 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn mk_full(
        id: &str,
        source: &str,
        typ: &str,
        lat: f64,
        lon: f64,
        heading: Option<f64>,
        source_count: i32,
    ) -> Threat {
        Threat {
            id: id.to_string(),
            source: source.to_string(),
            threat_type: typ.to_string(),
            title: "БпЛА".to_string(),
            lat,
            lon,
            heading,
            region: String::new(),
            district: String::new(),
            locality: String::new(),
            confidence: "medium".to_string(),
            source_count,
            target_count: 1,
            updated_at: Some(Utc::now()),
            area_only: false,
            advisory: false,
            trail: vec![],
        }
    }

    #[test]
    fn merges_close_same_type() {
        let ts = vec![
            mk("neptun:1", "neptun", 50.0, 30.0),
            mk("dimap:1", "dimap", 50.05, 30.05),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].sources.len(), 2);
        assert_eq!(c[0].confidence, "medium");
    }

    #[test]
    fn keeps_far_apart() {
        let ts = vec![
            mk("neptun:1", "neptun", 50.0, 30.0),
            mk("dimap:1", "dimap", 46.0, 32.0),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn transitive_merge() {
        // A~B, B~C (ланцюжок), A і C напряму далі радіуса — все одно один кластер
        let ts = vec![
            mk("a", "neptun", 50.0, 30.0),
            mk("b", "dimap", 50.1, 30.0),   // ~11км від a
            mk("c", "shahmap", 50.2, 30.0), // ~11км від b, ~22км від a
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].sources.len(), 3);
        assert_eq!(c[0].confidence, "high");
    }

    #[test]
    fn opposite_headings_split() {
        // Поруч, але курси назустріч — різні борти
        let ts = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, Some(90.0), 1),
            mk_full("b", "dimap", "uav", 50.02, 30.0, Some(270.0), 1),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn per_type_radius() {
        // 20км: для uav (25) — разом, для fpv (10) — окремо
        let uavs = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, None, 1),
            mk_full("b", "dimap", "uav", 50.18, 30.0, None, 1),
        ];
        assert_eq!(cluster_threats(&uavs).len(), 1);
        let fpvs = vec![
            mk_full("a", "neptun", "fpv", 50.0, 30.0, None, 1),
            mk_full("b", "dimap", "fpv", 50.18, 30.0, None, 1),
        ];
        assert_eq!(cluster_threats(&fpvs).len(), 2);
    }

    #[test]
    fn weighted_center_pulls_to_confirmed() {
        // source_count 3 проти 1: центр ближче до підтвердженої
        let ts = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, None, 3),
            mk_full("b", "dimap", "uav", 50.2, 30.0, None, 1),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert!(c[0].lat < 50.1, "центр тяжіє до ваги 3: {}", c[0].lat);
        assert!(c[0].spread_km > 15.0);
    }

    #[test]
    fn ids_stable_across_order() {
        let t1 = mk("neptun:1", "neptun", 50.0, 30.0);
        let t2 = mk("dimap:1", "dimap", 50.05, 30.05);
        let a = cluster_threats(&[t1.clone(), t2.clone()]);
        let b = cluster_threats(&[t2, t1]);
        assert_eq!(a[0].id, b[0].id);
    }

    #[test]
    fn cross_source_uav_recon_merges() {
        let ts = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, Some(280.0), 1),
            mk_full("b", "radarua", "recon", 50.05, 30.05, Some(290.0), 1),
            mk_full("c", "dimap", "uav", 50.02, 30.02, Some(285.0), 1),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].threat_type, "uav");
        assert!(c[0].confirmed);
    }

    #[test]
    fn calculates_consensus_heading() {
        let ts = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, Some(350.0), 1),
            mk_full("b", "dimap", "uav", 50.02, 30.02, Some(10.0), 1),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        // Average of 350° and 10° across 0° is 0° (or 360°)
        let h = c[0].heading.unwrap();
        assert!(h < 1.0 || h > 359.0, "очікується 0/360°, отримано {}", h);
    }

    #[test]
    fn marks_confirmed_for_multiple_sources() {
        let single = vec![mk("neptun:1", "neptun", 50.0, 30.0)];
        let c1 = cluster_threats(&single);
        assert!(!c1[0].confirmed);

        let two = vec![
            mk("neptun:1", "neptun", 50.0, 30.0),
            mk("dimap:1", "dimap", 50.02, 30.02),
        ];
        let c2 = cluster_threats(&two);
        assert!(!c2[0].confirmed, "2 джерела — ще не підтверджено");

        let three = vec![
            mk("neptun:1", "neptun", 50.0, 30.0),
            mk("dimap:1", "dimap", 50.02, 30.02),
            mk("shahmap:1", "shahmap", 50.01, 30.01),
        ];
        let c3 = cluster_threats(&three);
        assert!(c3[0].confirmed, "3 джерела — підтверджено");
    }

    #[test]
    fn single_source_many_reports_never_confirmed() {
        let ts = vec![
            mk_full("a", "neptun", "uav", 50.0, 30.0, None, 9),
            mk_full("b", "neptun", "uav", 50.01, 30.01, None, 9),
            mk_full("c", "neptun", "uav", 50.02, 30.02, None, 9),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].sources.len(), 1);
        assert!(!c[0].confirmed, "1 джерело (хоч з 9 внутрішніми звітами) НЕ може бути підтверджене");
    }

    #[test]
    fn fpv_never_confirmed() {
        let ts = vec![
            mk_full("a", "radarua", "fpv", 50.0, 30.0, None, 1),
            mk_full("b", "neptun", "fpv", 50.01, 30.01, None, 1),
            mk_full("c", "dimap", "fpv", 50.02, 30.02, None, 1),
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].threat_type, "fpv");
        assert!(!c[0].confirmed, "FPV цілі ніколи не мають статусу confirmed");
    }

    #[test]
    fn prevents_chaining_across_distant_targets() {
        // Ланцюг: a (0 км), b (40 км), c (80 км).
        // a і c далі ніж 55 км -> complete linkage НЕ зливає їх в один кластер діаметром 80 км.
        let ts = vec![
            mk("a", "neptun", 50.0, 30.0),
            mk("b", "dimap", 50.36, 30.0), // ~40 км від a
            mk("c", "shahmap", 50.72, 30.0), // ~40 км від b, ~80 км від a
        ];
        let c = cluster_threats(&ts);
        assert_eq!(c.len(), 2, "має бути 2 кластери, а не один розтягнутий на 80 км");
        assert!(c.iter().all(|cl| cl.spread_km <= 55.0));
    }
}

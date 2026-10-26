//! Детектор пусків: фіксує появу НОВИХ груп бортів від одного аеродрому.
//! Евристика: новий трек + зворотна атрибуція до сайта → кандидат;
//! подія, коли за один цикл опитування від сайта ≥ порога новачків.
//! МіГ-31К / балістика — поріг 1 (критично), решта — ≥3 (проти спаму рутиною).
//! Все з міткою «ймовірний»: джерела агрегують з затримкою, ID нестабільні.

use std::collections::{HashMap, HashSet};

use crate::approach::{attribute_launch, effective_heading};
use crate::launches::LaunchSite;
use crate::models::Threat;

#[derive(Debug, Clone)]
pub struct LaunchEvent {
    pub launch_id: String,
    pub launch_name: String,
    pub kind: String,
    pub lat: f64,
    pub lon: f64,
    pub count: usize,
    pub types: Vec<String>,
    pub first_seen: f64,
    pub last_seen: f64,
    /// heuristic (детект за треками) | official (сповіщення ПС ЗСУ)
    pub origin: String,
}

impl LaunchEvent {
    pub fn ago_min(&self, now: f64) -> f64 {
        ((now - self.last_seen) / 60.0).max(0.0)
    }
}

pub struct LaunchMonitor {
    /// id треку → коли вперше побачили (для детекту новачків)
    known: HashMap<String, f64>,
    events: Vec<LaunchEvent>,
    /// вікно «один наліт» для добивки лічильника, с
    group_window_sec: f64,
    /// скільки живе подія в видачі, с
    event_ttl_sec: f64,
    /// пам'ять про id, с (проти розростання)
    known_ttl_sec: f64,
}

impl LaunchMonitor {
    pub fn new() -> Self {
        Self {
            known: HashMap::new(),
            events: Vec::new(),
            group_window_sec: 600.0,
            event_ttl_sec: 3600.0, // 1 година (до 60 хв)
            known_ttl_sec: 7200.0,
        }
    }

    pub fn active(&self, now: f64) -> Vec<LaunchEvent> {
        self.events
            .iter()
            .filter(|e| now - e.last_seen <= self.event_ttl_sec)
            .cloned()
            .collect()
    }

    /// Один цикл після трекера. Повертає кількість нових подій.
    pub fn observe(&mut self, tracked: &[Threat], sites: &[LaunchSite], now: f64) -> usize {        // 1. новачки
        let mut fresh_groups: HashMap<usize, Vec<&Threat>> = HashMap::new();
        for t in tracked {
            if self.known.contains_key(&t.id) {
                continue;
            }
            self.known.insert(t.id.clone(), now);
            let Some(h) = effective_heading(t) else {
                continue;
            };
            if let Some(idx) = attribute_launch(t, h, sites) {
                fresh_groups.entry(idx).or_default().push(t);
            }
        }
        // 2. пороги → події
        let mut created = 0;
        // детермінований порядок для тестів/логів
        let mut idxs: Vec<usize> = fresh_groups.keys().cloned().collect();
        idxs.sort();
        for idx in idxs {
            let group = &fresh_groups[&idx];
            let s = &sites[idx];
            let critical = matches!(s.kind.as_str(), "mig31k" | "ballistic");
            let threshold = if critical { 1 } else { 3 };
            if group.len() < threshold {
                continue;
            }
            let mut types: Vec<String> =
                group.iter().map(|t| t.threat_type.clone()).collect();
            types.sort();
            types.dedup();
            // добивка в активну подію того ж сайта?
            if let Some(ev) = self
                .events
                .iter_mut()
                .filter(|e| e.launch_id == s.id)
                .find(|e| now - e.last_seen <= self.group_window_sec)
            {
                ev.count += group.len();
                ev.last_seen = now;
                for ty in types {
                    if !ev.types.contains(&ty) {
                        ev.types.push(ty);
                    }
                }
                ev.types.sort();
            } else {
                self.events.push(LaunchEvent {
                    launch_id: s.id.clone(),
                    launch_name: s.name.clone(),
                    kind: s.kind.clone(),
                    lat: s.lat,
                    lon: s.lon,
                    count: group.len(),
                    types,
                    first_seen: now,
                    last_seen: now,
                    origin: "heuristic".to_string(),
                });
                created += 1;
            }
        }
        // 3. чистка
        self.events.retain(|e| now - e.last_seen <= self.event_ttl_sec);
        let cutoff = now - self.known_ttl_sec;
        self.known.retain(|_, ts| *ts >= cutoff);
        // known не має рости безмежно навіть при довгому аптаймі
        if self.known.len() > 20000 {
            let mut ids: Vec<(String, f64)> =
                self.known.iter().map(|(k, v)| (k.clone(), *v)).collect();
            ids.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let drop: HashSet<String> =
                ids.into_iter().take(5000).map(|(k, _)| k).collect();
            self.known.retain(|k, _| !drop.contains(k));
        }
        created
    }
}

/// Події пусків ТІЛЬКИ з офіційних сповіщень із впізнаним носієм
/// (літак / корабель / аеродром через `carrier_site`).
/// Пости без носія — не події (лишаються маркерами шару `official`).
/// Групування: одна подія на сайт (перший/останній пост, лічильник постів).
pub fn official_events(
    sites: &[LaunchSite],
    notices: &[crate::parsers::official::Notice],
    _now: f64,
) -> Vec<LaunchEvent> {
    use crate::parsers::official::carrier_site;
    use std::collections::HashMap;

    struct Acc {
        site_idx: usize,
        first: f64,
        last: f64,
        count: usize,
        types: Vec<String>,
    }
    let mut acc: HashMap<String, Acc> = HashMap::new();
    // детермінований порядок: старі пости першими
    let mut sorted: Vec<&crate::parsers::official::Notice> = notices.iter().collect();
    sorted.sort_by_key(|a| a.observed_at);
    for n in sorted {
        let ts = n.observed_at.timestamp() as f64;
        if _now > 0.0 && _now - ts > 3600.0 {
            continue;
        }
        let Some(site_id) = carrier_site(&n.kind, &n.text.to_lowercase()) else {
            continue;
        };
        let Some(idx) = sites.iter().position(|s| s.id == site_id) else {
            continue;
        };
        acc.entry(site_id.to_string())
            .and_modify(|a| {
                a.first = a.first.min(ts);
                a.last = a.last.max(ts);
                a.count += 1;
                if !a.types.contains(&n.kind) {
                    a.types.push(n.kind.clone());
                }
            })
            .or_insert(Acc {
                site_idx: idx,
                first: ts,
                last: ts,
                count: 1,
                types: vec![n.kind.clone()],
            });
    }
    let mut out: Vec<LaunchEvent> = acc
        .into_values()
        .map(|a| {
            let s = &sites[a.site_idx];
            let mut types = a.types;
            types.sort();
            LaunchEvent {
                launch_id: s.id.clone(),
                launch_name: s.name.clone(),
                kind: s.kind.clone(),
                lat: s.lat,
                lon: s.lon,
                count: a.count,
                types,
                first_seen: a.first,
                last_seen: a.last,
                origin: "official".to_string(),
            }
        })
        .collect();
    // найсвіжіші першими — для банера
    out.sort_by(|a, b| b.last_seen.partial_cmp(&a.last_seen).unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launches::all_sites;
    use chrono::Utc;

    fn mk(id: &str, typ: &str, lat: f64, lon: f64, heading: f64) -> Threat {
        Threat {
            id: id.to_string(),
            source: "dimap".to_string(),
            threat_type: typ.to_string(),
            title: String::new(),
            lat,
            lon,
            heading: Some(heading),
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

    /// Група шахедів з півдня курсом на північний захід (від Приморська).
    fn southern_group(n: usize, t0: usize) -> Vec<Threat> {
        (0..n)
            .map(|i| {
                mk(
                    &format!("uav:{t0}:{i}"),
                    "uav",
                    46.6 + i as f64 * 0.05,
                    35.2 + i as f64 * 0.05,
                    320.0,
                )
            })
            .collect()
    }

    #[test]
    fn detects_group_launch() {
        let sites = all_sites();
        let mut m = LaunchMonitor::new();
        // 1 борт — нижче порога, події немає
        let one = southern_group(1, 0);
        assert_eq!(m.observe(&one, &sites, 1000.0), 0);
        assert!(m.active(1000.0).is_empty());
        // +3 нових за той же цикл → подія
        let mut four = one.clone();
        four.extend(southern_group(3, 1));
        assert_eq!(m.observe(&four, &sites, 1005.0), 1);
        let ev = &m.active(1005.0)[0];
        assert_eq!(ev.count, 3);
        assert!(ev.launch_name.len() > 2);
    }

    #[test]
    fn single_mig_is_event() {
        let sites = all_sites();
        let mut m = LaunchMonitor::new();
        // МіГ біля Саваслейки курсом на південний захід
        let t = mk("m1", "mig31k", 54.5, 41.0, 225.0);
        assert_eq!(m.observe(&[t], &sites, 2000.0), 1);
        assert_eq!(m.active(2000.0).len(), 1);
    }

    #[test]
    fn no_double_count_same_tracks() {
        let sites = all_sites();
        let mut m = LaunchMonitor::new();
        let g = southern_group(4, 0);
        m.observe(&g, &sites, 1000.0);
        // той же набір повторно — новачків немає
        assert_eq!(m.observe(&g, &sites, 1005.0), 0);
    }

    #[test]
    fn events_expire() {
        let sites = all_sites();
        let mut m = LaunchMonitor::new();
        let t = mk("m9", "mig31k", 54.5, 41.0, 225.0);
        m.observe(&[t], &sites, 0.0);
        assert_eq!(m.active(0.0).len(), 1);
        assert!(m.active(10000.0).is_empty(), "після TTL подія гасне");
    }

    fn notice(id: &str, kind: &str, text: &str, ts: i64) -> crate::parsers::official::Notice {
        crate::parsers::official::Notice {
            id: id.to_string(),
            channel: "kpszsu".to_string(),
            post_url: format!("https://t.me/kpszsu/{id}"),
            text: text.to_string(),
            observed_at: chrono::DateTime::from_timestamp(ts, 0).unwrap(),
            kind: kind.to_string(),
            oblast: None,
            lat: None,
            lon: None,
        }
    }

    #[test]
    fn official_only_carriers_become_events() {
        let sites = all_sites();
        let ns = vec![
            notice("1", "mig31k", "Зліт МіГ-31К з Саваслейки", 1000),
            notice("2", "mig31k", "Зліт МіГ-31К", 1100),
            notice("3", "missile", "Пуски Калібрів з Чорного моря", 1200),
            notice("4", "uav", "Рух БпЛА на Сумщині", 1300),
            notice("5", "kab", "Пуски КАБ на Сумщині", 1400),
            notice("6", "missile", "Ракети курсом на Київ", 1500),
        ];
        let ev = official_events(&sites, &ns, 2000.0);
        // Саваслейка (2 пости → 1 подія) + Новоросійськ; рух/КАБ/безносій — ні
        assert_eq!(ev.len(), 2, "{ev:?}");
        let sav = ev.iter().find(|e| e.launch_id == "savasleyka").unwrap();
        assert_eq!(sav.count, 2);
        assert_eq!(sav.first_seen, 1000.0);
        assert_eq!(sav.last_seen, 1100.0);
        assert_eq!(sav.origin, "official");
        let nav = ev.iter().find(|e| e.launch_id == "novorossiysk").unwrap();
        assert_eq!(nav.types, vec!["missile".to_string()]);
        // найсвіжіші першими
        assert_eq!(ev[0].launch_id, "novorossiysk");
    }
}

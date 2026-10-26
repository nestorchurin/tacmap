//! Трекер стабільних цілей (TargetTracker).
//! Присвоює кожному фізичному борту / групі цілей стабільний TACMAP-ідентифікатор
//! (uav_001, uav_002, uav_jet_001, missile_001, тощо) і супроводжує його політ у часі,
//! не дозволяючи лічильнику цілей у БД накручуватися при кожному 5-секундному опитуванні.
//!
//! Також веде трекер руху БПЛА:
//! 1. На карті відображаються хвости руху за останню годину (до 3600 с), після чого старі відрізки зникають.
//! 2. Усі часопросторові точки та сесії польотів перманентно записуються в реляційні таблиці `tracks` та `track_points` для вічної хронології.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::borders::haversine_km;
use crate::cluster::are_types_compatible;
use crate::models::Cluster;
use crate::track_storage::{self, Track, TrackPoint as DbTrackPoint, TrackStatus};

/// Точка траєкторії польоту з часовою міткою
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimedPoint {
    pub lat: f64,
    pub lon: f64,
    pub time: f64,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
    pub altitude: Option<f64>,
}

/// Супроводжуваний трек цілі
#[derive(Debug, Clone)]
pub struct TrackedTarget {
    pub id: String,
    pub threat_type: String,
    pub estimated_type: String,
    pub lat: f64,
    pub lon: f64,
    pub heading: Option<f64>,
    pub first_seen: f64,
    pub last_seen: f64,
    pub confirmed: bool,
    pub matched_member_ids: HashSet<String>,
    pub has_official_source: bool,
    /// Офіційні ідентифікатори треків (UkraineAlarm trackId)
    pub official_track_ids: HashSet<String>,
    /// Хвіст засічок за останню годину (точки старші за 3600 с видаляються з пам'яті карти)
    pub history: Vec<TimedPoint>,
    pub is_active: bool,
    pub status: TrackStatus,
    /// Усі унікальні джерела, що будь-коли фіксували цей трек
    pub all_sources: HashSet<String>,
    /// Загальна дистанція польоту, км
    pub total_dist_km: f64,
    /// Загальна кількість засічок у БД
    pub points_count: i64,
    /// Максимальна швидкість, км/год
    pub max_speed: Option<f64>,
    /// Сума швидкостей для розрахунку середньої
    pub sum_speed: f64,
    pub speed_samples: u32,
    /// Координати точки першої фіксації (входу)
    pub entry_point: String,
}

pub struct TargetTracker {
    tracks: HashMap<String, TrackedTarget>,
    seq_counters: HashMap<String, u64>,
    expire_sec: f64,
    conn: Option<Connection>,
    /// Відображення сирого ID джерела на присвоєний track_id (для маркування сирих звітів)
    member_to_track: HashMap<String, String>,
    /// Пряма синхронізація офіційного trackId (UkraineAlarm) на стабільний TACMAP track_id
    pub official_to_track: HashMap<String, String>,
    /// Поточні активні тривоги (key -> alert_level) для фіксації подій початку/відбою
    active_alert_levels: HashMap<String, String>,
}

impl TargetTracker {
    pub fn new(db_path: &Path) -> Self {
        let conn = match Connection::open(db_path) {
            Ok(c) => {
                let _ = c.execute("PRAGMA journal_mode = WAL;", []);
                let _ = c.execute("PRAGMA foreign_keys = ON;", []);
                let _ = c.busy_timeout(std::time::Duration::from_millis(5000));
                let _ = track_storage::init_schema(&c);
                Some(c)
            }
            Err(e) => {
                tracing::warn!("TargetTracker: cannot open db at {:?}: {}", db_path, e);
                None
            }
        };

        let mut tracker = Self {
            tracks: HashMap::new(),
            seq_counters: HashMap::new(),
            expire_sec: 180.0, // 3 хвилини без оновлень — активний борт вважається завершеним/втраченим
            conn,
            member_to_track: HashMap::new(),
            official_to_track: HashMap::new(),
            active_alert_levels: HashMap::new(),
        };
        tracker.init_counters();
        tracker
    }

    #[cfg(test)]
    pub fn with_connection(conn: Connection) -> Self {
        let _ = track_storage::init_schema(&conn);
        Self {
            tracks: HashMap::new(),
            seq_counters: HashMap::new(),
            expire_sec: 180.0,
            conn: Some(conn),
            member_to_track: HashMap::new(),
            official_to_track: HashMap::new(),
            active_alert_levels: HashMap::new(),
        }
    }

    /// Ініціалізація лічильників з існуючої БД
    fn init_counters(&mut self) {
        let Some(conn) = &self.conn else { return };
        // Перевіряємо як cluster_observations, так і таблицю tracks
        let mut max_ids: Vec<String> = Vec::new();
        if let Ok(mut stmt) = conn.prepare("SELECT id FROM tracks") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for id in rows.flatten() {
                    max_ids.push(id);
                }
            }
        }
        if let Ok(mut stmt) = conn.prepare("SELECT DISTINCT cluster_id FROM cluster_observations") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for id in rows.flatten() {
                    max_ids.push(id);
                }
            }
        }

        for id in max_ids {
            if let Some((prefix, num_str)) = id.rsplit_once('_') {
                if let Ok(num) = num_str.parse::<u64>() {
                    let cur = self.seq_counters.entry(prefix.to_string()).or_insert(0);
                    if num > *cur {
                        *cur = num;
                    }
                }
            }
        }
    }

    /// Скидання лічильників при перестворенні бази з нуля
    #[allow(dead_code)]
    pub fn reset_counters(&mut self) {
        self.tracks.clear();
        self.seq_counters.clear();
        self.official_to_track.clear();
    }

    /// Оновлення кластерів поточного циклу: прив'язка до стабільних ID, супровід треків та запис у БД
    pub fn update(&mut self, clusters: &mut [Cluster], now: f64) {
        self.member_to_track.clear();

        // 1. Обробка статусів завершених треків та очищення хвостів старше 1 години
        for tr in self.tracks.values_mut() {
            if tr.is_active && (now - tr.last_seen > self.expire_sec) {
                tr.is_active = false;
                let dist_border = crate::borders::nearest_border_point(tr.lat, tr.lon)[2];
                let is_inside = crate::borders::inside_ukraine(tr.lat, tr.lon);
                let final_status = if !is_inside || dist_border < 15.0 {
                    TrackStatus::ExitedAirspace
                } else {
                    TrackStatus::LostEw
                };
                tr.status = final_status;
                if let Some(conn) = &self.conn {
                    let _ = track_storage::update_track_status(conn, &tr.id, final_status, tr.last_seen as i64);
                }
            }
            // Хвости на карті відображаються до 1 години (3600 с), старіші точки відкидаються
            tr.history.retain(|p| now - p.time <= 3600.0);
        }

        // Повністю прибираємо з пам'яті карти треки, де остання точка спостерігалася понад 1 годину тому
        self.tracks.retain(|_, tr| now - tr.last_seen <= 3600.0);
        self.official_to_track.retain(|_, tr_id| self.tracks.contains_key(tr_id));

        let mut matched_clusters = HashSet::new();
        let mut matched_tracks = HashSet::new();

        // 2А. ПРЯМА СИНХРОНІЗАЦІЯ З ОФІЦІЙНИМ ДЖЕРЕЛОМ (UkraineAlarm trackId / ПС ЗСУ)
        // Якщо ціль має офіційний trackId, який уже супроводжувався раніше — це 100% той самий борт!
        // Жодних штучних розривів на шматки, незалежно від відстані між оновленнями радара.
        for (i, c) in clusters.iter_mut().enumerate() {
            let official_id = c.members.iter()
                .find(|m| m.source == "ukrainealarm" && !m.id.is_empty())
                .map(|m| m.id.clone());

            if let Some(off_id) = official_id {
                if let Some(tr_id) = self.official_to_track.get(&off_id).cloned() {
                    if !matched_tracks.contains(&tr_id) {
                        if let Some(tr) = self.tracks.get_mut(&tr_id) {
                            matched_clusters.insert(i);
                            matched_tracks.insert(tr_id.clone());

                            let dt = (now - tr.last_seen).max(0.0);
                            let dist = haversine_km(tr.lat, tr.lon, c.lat, c.lon);

                            // Захист від штучної колізії якщо джерело перевикористало ID за сотні км
                            if dist > 35.0 && dt < 60.0 {
                                continue;
                            }

                            let was_inactive = !tr.is_active;

                            tr.is_active = true;
                            tr.status = TrackStatus::Active;
                            if was_inactive {
                                tracing::info!("Офіційний борт {} ({}) повернувся на радари (dt={:.0} с, dist={:.1} км). Відновлюємо супровід!", tr.id, off_id, dt, dist);
                                if let Some(conn) = &self.conn {
                                    let _ = track_storage::update_track_status(conn, &tr.id, TrackStatus::Active, now as i64);
                                }
                            }

                            tr.lat = c.lat;
                            tr.lon = c.lon;
                            if c.heading.is_some() {
                                tr.heading = c.heading;
                            }
                            tr.last_seen = now;
                            tr.confirmed = true;
                            tr.has_official_source = true;
                            tr.official_track_ids.insert(off_id.clone());
                            for m in &c.members {
                                tr.matched_member_ids.insert(m.id.clone());
                                tr.all_sources.insert(m.source.clone());
                                self.member_to_track.insert(m.id.clone(), tr.id.clone());
                            }

                            let moved = dist > 0.04 || dt >= 8.0 || tr.history.is_empty();
                            if moved {
                                let calc_speed = if dt > 0.5 && (dist / dt * 3600.0) <= 1200.0 && (dist / dt * 3600.0) >= 15.0 {
                                    Some(dist / dt * 3600.0)
                                } else {
                                    None
                                };
                                let pt = TimedPoint {
                                    lat: c.lat,
                                    lon: c.lon,
                                    time: now,
                                    speed: calc_speed,
                                    heading: c.heading,
                                    altitude: None,
                                };
                                tr.history.push(pt);
                                tr.total_dist_km += dist;
                                tr.points_count += 1;
                                if let Some(spd) = calc_speed {
                                    tr.max_speed = Some(tr.max_speed.map_or(spd, |m| m.max(spd)));
                                    tr.sum_speed += spd;
                                    tr.speed_samples += 1;
                                }
                                let avg_spd = if tr.speed_samples > 0 {
                                    Some(tr.sum_speed / tr.speed_samples as f64)
                                } else {
                                    None
                                };
                                let estimated_type = track_storage::determine_estimated_type(&tr.threat_type, avg_spd, tr.max_speed);
                                tr.estimated_type = estimated_type.clone();
                                let sources_vec: Vec<String> = tr.all_sources.iter().cloned().collect();
                                let sources_str = sources_vec.join(",");

                                if let Some(conn) = &self.conn {
                                    let _ = track_storage::insert_track_point(conn, &DbTrackPoint {
                                        id: None,
                                        track_id: tr.id.clone(),
                                        timestamp: now as i64,
                                        latitude: c.lat,
                                        longitude: c.lon,
                                        altitude: None,
                                        speed: calc_speed,
                                        heading: c.heading,
                                        source_count: c.sources.len().max(1) as i64,
                                        sources: c.sources.join(","),
                                        confirmed: c.confirmed,
                                        spread_km: Some(c.spread_km),
                                        region: if c.region.is_empty() { None } else { Some(c.region.clone()) },
                                    });
                                    let _ = track_storage::upsert_track(conn, &Track {
                                        id: tr.id.clone(),
                                        first_seen: tr.first_seen as i64,
                                        last_seen: now as i64,
                                        status: tr.status,
                                        threat_type: tr.threat_type.clone(),
                                        estimated_type,
                                        confirmed: tr.confirmed,
                                        sources: sources_str,
                                        points_count: tr.points_count,
                                        total_dist_km: (tr.total_dist_km * 10.0).round() / 10.0,
                                        avg_speed: avg_spd.map(|v| (v * 10.0).round() / 10.0),
                                        max_speed: tr.max_speed.map(|v| (v * 10.0).round() / 10.0),
                                        entry_point: Some(tr.entry_point.clone()),
                                        exit_point: Some(format!("{:.4},{:.4}", c.lat, c.lon)),
                                    });
                                }
                            }

                            tr.history.retain(|p| now - p.time <= 3600.0);
                            c.id = tr.id.clone();
                            let mut tr_trail: Vec<[f64; 2]> = tr.history.iter().map(|p| [p.lat, p.lon]).collect();
                            if let Some(last) = tr_trail.last() {
                                let d = (last[0] - c.lat).hypot(last[1] - c.lon);
                                if d > 0.00005 {
                                    tr_trail.push([c.lat, c.lon]);
                                }
                            } else {
                                tr_trail.push([c.lat, c.lon]);
                            }
                            c.trail = tr_trail;
                        }
                    }
                }
            }
        }

        // 2Б. Пошук можливих збігів серед решти кластерів та активних треків
        let current_cycle_member_ids: HashSet<String> = clusters
            .iter()
            .flat_map(|c| c.members.iter().map(|m| m.id.clone()))
            .collect();

        let mut candidate_pairs: Vec<(usize, String, f64)> = Vec::new();

        for (i, c) in clusters.iter().enumerate() {
            if matched_clusters.contains(&i) {
                continue;
            }
            let member_ids: HashSet<String> = c.members.iter().map(|m| m.id.clone()).collect();

            for (tr_id, tr) in &self.tracks {
                if matched_tracks.contains(tr_id) {
                    continue;
                }
                let dt = (now - tr.last_seen).max(0.0);
                // Для неактивних треків дозволяємо повернення не пізніше 20 хвилин (1200 с)
                if !tr.is_active && dt > 1200.0 {
                    continue;
                }
                // Перевірка сумісності типів
                if !are_types_compatible(&c.threat_type, &tr.threat_type) {
                    continue;
                }

                // Перевірка конфлікту ідентифікаторів
                let mut source_conflict = false;
                for m in &c.members {
                    if let Some((src_c, id_c)) = m.id.split_once(':') {
                        for prev_m in &tr.matched_member_ids {
                            if let Some((src_tr, id_tr)) = prev_m.split_once(':') {
                                if src_c == src_tr && id_c != id_tr && current_cycle_member_ids.contains(prev_m) {
                                    source_conflict = true;
                                    break;
                                }
                            }
                        }
                    }
                    if source_conflict {
                        break;
                    }
                }
                if source_conflict {
                    continue;
                }

                let dist = haversine_km(c.lat, c.lon, tr.lat, tr.lon);
                let overlap = member_ids.intersection(&tr.matched_member_ids).count();

                // Навіть для однакового ID: якщо дистанція > 35 км за менш ніж 60 секунд — це колізія ID або інший борт
                if overlap > 0 && dist > 35.0 && dt < 60.0 {
                    continue;
                }

                // Фізичний шлюз дальності застосовується ТІЛЬКИ якщо немає спільного ідентифікатора (overlap == 0)
                if overlap == 0 {
                    let max_speed_kmh = match c.threat_type.as_str() {
                        "uav_jet" => 650.0,
                        "missile" => 1100.0,
                        "ballistic" => 6000.0,
                        "tactical" | "mig31k" => 2000.0,
                        _ => 280.0, // Shahed-136 (макс швидкість ~185–220 км/год)
                    };

                    // Жорсткий фізичний бар'єр швидкості для сторонніх/нових засічок
                    if dist > 4.0 && dt > 2.0 {
                        let speed_kmh = dist / dt * 3600.0;
                        if speed_kmh > max_speed_kmh * 1.35 {
                            continue;
                        }
                    }

                    let max_allowed_dist = if tr.is_active {
                        (max_speed_kmh / 3600.0) * dt + 3.8
                    } else {
                        if dt > 600.0 {
                            continue;
                        }
                        (max_speed_kmh / 3600.0) * dt + 4.5
                    };

                    if dist > max_allowed_dist {
                        continue;
                    }

                    // Перевірка розбіжності курсів (якщо цілі рознесені в просторі)
                    if let (Some(ch), Some(trh)) = (c.heading, tr.heading) {
                        if dist > 6.0 {
                            let diff = (ch - trh).abs() % 360.0;
                            let min_diff = if diff > 180.0 { 360.0 - diff } else { diff };
                            if min_diff > 90.0 {
                                continue;
                            }
                        }
                    }

                    // Для неактивного борту без однакового ID перевіряємо вектор переміщення
                    if !tr.is_active && dist > 5.0 {
                        if let Some(trh) = tr.heading {
                            let bearing = crate::borders::bearing_deg(tr.lat, tr.lon, c.lat, c.lon);
                            let diff = (trh - bearing).abs() % 360.0;
                            let min_diff = if diff > 180.0 { 360.0 - diff } else { diff };
                            if min_diff > 65.0 {
                                continue;
                            }
                        }
                    }
                }

                // Пріоритет: однаковий ID має абсолютний пріоритет над просто близькими координатами
                let overlap_bonus = if overlap > 0 { 150.0 } else { 0.0 };
                let inactive_penalty = if tr.is_active { 0.0 } else { 30.0 };
                let score = dist - overlap_bonus + inactive_penalty;
                candidate_pairs.push((i, tr_id.clone(), score));
            }
        }

        // 3. Сортування кандидатів за найкращим збігом
        candidate_pairs.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        for (cluster_idx, track_id, _) in candidate_pairs {
            if matched_clusters.contains(&cluster_idx) || matched_tracks.contains(&track_id) {
                continue;
            }

            matched_clusters.insert(cluster_idx);
            matched_tracks.insert(track_id.clone());

            let c = &mut clusters[cluster_idx];
            let member_ids: HashSet<String> = c.members.iter().map(|m| m.id.clone()).collect();
            let has_official = c.members.iter().any(|m| m.source == "ukrainealarm" || m.source == "official");

            if let Some(tr) = self.tracks.get_mut(&track_id) {
                let dt = (now - tr.last_seen).max(0.0);
                let dist = haversine_km(tr.lat, tr.lon, c.lat, c.lon);
                let speed_kmh = if dt > 0.5 { dist / dt * 3600.0 } else { 0.0 };

                let was_inactive = !tr.is_active;
                tr.is_active = true;
                tr.status = TrackStatus::Active;
                if was_inactive {
                    tracing::info!("БпЛА {} повернувся на радари (dt={:.0} с, dist={:.1} км). Відновлюємо супровід!", tr.id, dt, dist);
                    if let Some(conn) = &self.conn {
                        let _ = track_storage::update_track_status(conn, &tr.id, TrackStatus::Active, now as i64);
                    }
                }

                tr.lat = c.lat;
                tr.lon = c.lon;
                if c.heading.is_some() {
                    tr.heading = c.heading;
                }
                tr.last_seen = now;
                tr.confirmed |= c.confirmed;
                tr.matched_member_ids.extend(member_ids);
                if has_official {
                    tr.has_official_source = true;
                }

                let official_id = c.members.iter()
                    .find(|m| m.source == "ukrainealarm" && !m.id.is_empty())
                    .map(|m| m.id.clone());
                if let Some(off_id) = official_id {
                    self.official_to_track.insert(off_id.clone(), tr.id.clone());
                    tr.official_track_ids.insert(off_id);
                }

                // Додаємо точку, якщо є переміщення або минуло >= 8 секунд
                let moved = dist > 0.04 || dt >= 8.0 || tr.history.is_empty();
                if moved {
                    let calc_speed = if dt > 0.5 && speed_kmh >= 15.0 && speed_kmh <= 1200.0 {
                        Some(speed_kmh)
                    } else {
                        None
                    };

                    let pt = TimedPoint {
                        lat: c.lat,
                        lon: c.lon,
                        time: now,
                        speed: calc_speed,
                        heading: c.heading,
                        altitude: None,
                    };
                    tr.history.push(pt);

                    // Перманентне збереження засічки в базі для історії/хронології
                    for s in &c.sources {
                        tr.all_sources.insert(s.clone());
                    }
                    tr.total_dist_km += dist;
                    tr.points_count += 1;
                    if let Some(spd) = calc_speed {
                        tr.max_speed = Some(tr.max_speed.map_or(spd, |m| m.max(spd)));
                        tr.sum_speed += spd;
                        tr.speed_samples += 1;
                    }
                    let avg_spd = if tr.speed_samples > 0 {
                        Some(tr.sum_speed / tr.speed_samples as f64)
                    } else {
                        None
                    };
                    let estimated_type = track_storage::determine_estimated_type(&tr.threat_type, avg_spd, tr.max_speed);
                    let sources_vec: Vec<String> = tr.all_sources.iter().cloned().collect();
                    let sources_str = sources_vec.join(",");

                    if let Some(conn) = &self.conn {
                        let _ = track_storage::insert_track_point(conn, &DbTrackPoint {
                            id: None,
                            track_id: tr.id.clone(),
                            timestamp: now as i64,
                            latitude: c.lat,
                            longitude: c.lon,
                            altitude: None,
                            speed: calc_speed,
                            heading: c.heading,
                            source_count: c.sources.len().max(1) as i64,
                            sources: c.sources.join(","),
                            confirmed: c.confirmed,
                            spread_km: Some(c.spread_km),
                            region: if c.region.is_empty() { None } else { Some(c.region.clone()) },
                        });
                        let _ = track_storage::upsert_track(conn, &Track {
                            id: tr.id.clone(),
                            first_seen: tr.first_seen as i64,
                            last_seen: now as i64,
                            status: tr.status,
                            threat_type: tr.threat_type.clone(),
                            estimated_type,
                            confirmed: tr.confirmed,
                            sources: sources_str,
                            points_count: tr.points_count,
                            total_dist_km: (tr.total_dist_km * 10.0).round() / 10.0,
                            avg_speed: avg_spd.map(|v| (v * 10.0).round() / 10.0),
                            max_speed: tr.max_speed.map(|v| (v * 10.0).round() / 10.0),
                            entry_point: Some(tr.entry_point.clone()),
                            exit_point: Some(format!("{:.4},{:.4}", c.lat, c.lon)),
                        });
                    }
                }

                for m in &c.members {
                    self.member_to_track.insert(m.id.clone(), tr.id.clone());
                }

                // Тримаємо в активній пам'яті карти тільки точки за останню годину
                tr.history.retain(|p| now - p.time <= 3600.0);

                c.id = tr.id.clone();
                let mut tr_trail: Vec<[f64; 2]> = tr.history.iter().map(|p| [p.lat, p.lon]).collect();
                if let Some(last) = tr_trail.last() {
                    let d = (last[0] - c.lat).hypot(last[1] - c.lon);
                    if d > 0.00005 {
                        tr_trail.push([c.lat, c.lon]);
                    }
                } else {
                    tr_trail.push([c.lat, c.lon]);
                }
                c.trail = tr_trail;
            }
        }

        // 4. Для всіх незбіглих кластерів створюємо новий трек
        for (i, c) in clusters.iter_mut().enumerate() {
            if matched_clusters.contains(&i) {
                continue;
            }

            let prefix = match c.threat_type.as_str() {
                "uav_jet" => "uav_jet",
                "uav" => "uav",
                "missile" => "missile",
                "ballistic" => "ballistic",
                "kab" => "kab",
                "recon" => "recon",
                "fpv" => "fpv",
                "tactical" | "mig31k" => "tactical",
                _ => "target",
            };

            let counter = self.seq_counters.entry(prefix.to_string()).or_insert(0);
            *counter += 1;
            let persistent_id = format!("{}_{:03}", prefix, *counter);

            let member_ids: HashSet<String> = c.members.iter().map(|m| m.id.clone()).collect();
            let has_official = c.members.iter().any(|m| m.source == "ukrainealarm" || m.source == "official");

            let official_id = c.members.iter()
                .find(|m| m.source == "ukrainealarm" && !m.id.is_empty())
                .map(|m| m.id.clone());

            let mut official_track_ids = HashSet::new();
            if let Some(off_id) = &official_id {
                self.official_to_track.insert(off_id.clone(), persistent_id.clone());
                official_track_ids.insert(off_id.clone());
            }

            let pt = TimedPoint {
                lat: c.lat,
                lon: c.lon,
                time: now,
                speed: None,
                heading: c.heading,
                altitude: None,
            };

            let mut all_sources = HashSet::new();
            for s in &c.sources {
                all_sources.insert(s.clone());
            }
            let entry_point = format!("{:.4},{:.4}", c.lat, c.lon);
            let estimated_type = track_storage::determine_estimated_type(&c.threat_type, None, None);
            let sources_vec: Vec<String> = all_sources.iter().cloned().collect();
            let sources_str = sources_vec.join(",");

            let tr = TrackedTarget {
                id: persistent_id.clone(),
                threat_type: c.threat_type.clone(),
                estimated_type: estimated_type.clone(),
                lat: c.lat,
                lon: c.lon,
                heading: c.heading,
                first_seen: now,
                last_seen: now,
                confirmed: c.confirmed,
                matched_member_ids: member_ids,
                has_official_source: has_official,
                official_track_ids,
                history: vec![pt],
                is_active: true,
                status: TrackStatus::Active,
                all_sources,
                total_dist_km: 0.0,
                points_count: 1,
                max_speed: None,
                sum_speed: 0.0,
                speed_samples: 0,
                entry_point: entry_point.clone(),
            };

            // Збереження нового треку та його першої засічки в БД
            if let Some(conn) = &self.conn {
                let _ = track_storage::upsert_track(conn, &Track {
                    id: persistent_id.clone(),
                    first_seen: now as i64,
                    last_seen: now as i64,
                    status: TrackStatus::Active,
                    threat_type: c.threat_type.clone(),
                    estimated_type,
                    confirmed: c.confirmed,
                    sources: sources_str.clone(),
                    points_count: 1,
                    total_dist_km: 0.0,
                    avg_speed: None,
                    max_speed: None,
                    entry_point: Some(entry_point),
                    exit_point: Some(format!("{:.4},{:.4}", c.lat, c.lon)),
                });
                let _ = track_storage::insert_track_point(conn, &DbTrackPoint {
                    id: None,
                    track_id: persistent_id.clone(),
                    timestamp: now as i64,
                    latitude: c.lat,
                    longitude: c.lon,
                    altitude: None,
                    speed: None,
                    heading: c.heading,
                    source_count: c.sources.len().max(1) as i64,
                    sources: sources_str,
                    confirmed: c.confirmed,
                    spread_km: Some(c.spread_km),
                    region: if c.region.is_empty() { None } else { Some(c.region.clone()) },
                });
            }

            for m in &c.members {
                self.member_to_track.insert(m.id.clone(), persistent_id.clone());
            }

            c.id = persistent_id.clone();
            c.trail = vec![[c.lat, c.lon]];
            self.tracks.insert(persistent_id, tr);
        }
    }

    /// Генерація GeoJSON шару хвостів руху БпЛА за останню годину
    pub fn uav_trails_geojson(&self, now: f64, official_only: bool) -> serde_json::Value {
        let mut features = Vec::new();
        for tr in self.tracks.values() {
            // Фільтруємо суто БпЛА (звичайні Shahed-136 та реактивні Shahed-238)
            if tr.threat_type != "uav" && tr.threat_type != "uav_jet" {
                continue;
            }
            if official_only && !tr.has_official_source {
                continue;
            }

            // Тільки точки за останню годину (<= 3600 секунд)
            let valid_points: Vec<&TimedPoint> = tr
                .history
                .iter()
                .filter(|p| now - p.time <= 3600.0)
                .collect();

            let mut coords: Vec<[f64; 2]> = valid_points.iter().map(|p| [p.lon, p.lat]).collect();

            // Для активного БпЛА гарантовано приєднуємо поточну позицію [tr.lon, tr.lat] до кінця лінії
            // (тягнемо хвіст прямо до дрона — без розриву між іконкою та лінією)
            if tr.is_active {
                let should_append = match coords.last() {
                    Some(last) => {
                        let d = (last[0] - tr.lon).hypot(last[1] - tr.lat);
                        d > 0.00005
                    }
                    None => true,
                };
                if should_append {
                    coords.push([tr.lon, tr.lat]);
                }
            }

            // Хвіст малюється тільки якщо є лінія (>= 2 точки)
            if coords.len() < 2 {
                continue;
            }

            // Відсікаємо 1-сегментні палички шуму, якщо загальна дистанція < 0.3 км
            if coords.len() < 3 && tr.total_dist_km < 0.3 {
                let d = haversine_km(coords[0][1], coords[0][0], coords.last().unwrap()[1], coords.last().unwrap()[0]);
                if d < 0.3 {
                    continue;
                }
            }

            let color = if tr.threat_type == "uav_jet" {
                if tr.is_active { "#ff5500" } else { "#b45309" }
            } else {
                if tr.is_active { "#ffaa00" } else { "#78716c" }
            };

            features.push(serde_json::json!({
                "type": "Feature",
                "properties": {
                    "id": tr.id,
                    "rtype": tr.threat_type,
                    "color": color,
                    "active": tr.is_active,
                    "status": tr.status.as_str(),
                    "points_count": coords.len(),
                    "age_sec": (now - tr.last_seen).max(0.0) as i64,
                    "duration_min": ((now - tr.first_seen).max(0.0) / 60.0).round() as i64,
                },
                "geometry": {
                    "type": "LineString",
                    "coordinates": coords,
                }
            }));
        }

        serde_json::json!({
            "type": "FeatureCollection",
            "features": features,
        })
    }

    /// Генерація GeoJSON шару кінцевих точок, де БпЛА зникли з радарів
    pub fn lost_points_geojson(&self, now: f64, official_only: bool) -> serde_json::Value {
        let mut features = Vec::new();
        for tr in self.tracks.values() {
            // Тільки БпЛА, які ЗНИКЛИ з радарів (!is_active)
            if tr.is_active {
                continue;
            }
            if tr.threat_type != "uav" && tr.threat_type != "uav_jet" {
                continue;
            }
            if official_only && !tr.has_official_source {
                continue;
            }

            // Кінцева точка де борт зник показується ТІЛЬКИ для РЕАЛЬНОГО польоту:
            // Мінімум 3 засічки і тривалість польоту не менше 20 секунд (відсікаємо миттєві шуми/фантоми)
            let duration_sec = tr.last_seen - tr.first_seen;
            if tr.history.len() < 3 || duration_sec < 20.0 {
                continue;
            }

            // Точка відображається на карті протягом 15 хвилин (900 секунд) з моменту зникнення
            let age_sec = now - tr.last_seen;
            if age_sec < 0.0 || age_sec > 900.0 {
                continue;
            }

            let status_text = tr.status.as_str();
            let color = match tr.status {
                TrackStatus::ExitedAirspace => "#38bdf8", // блакитний для виходу з простору
                _ => "#ef4444",                          // червоний для втрачений РЕБ / зник з радарів
            };

            features.push(serde_json::json!({
                "type": "Feature",
                "properties": {
                    "id": tr.id,
                    "rtype": tr.threat_type,
                    "color": color,
                    "status": status_text,
                    "age_sec": age_sec.max(0.0) as i64,
                    "age_min": (age_sec.max(0.0) / 60.0).round() as i64,
                    "duration_min": ((tr.last_seen - tr.first_seen).max(0.0) / 60.0).round() as i64,
                    "last_seen": tr.last_seen as i64,
                    "first_seen": tr.first_seen as i64,
                    "latlon": format!("{:.3}, {:.3}", tr.lat, tr.lon),
                },
                "geometry": {
                    "type": "Point",
                    "coordinates": [tr.lon, tr.lat],
                }
            }));
        }

        serde_json::json!({
            "type": "FeatureCollection",
            "features": features,
        })
    }

    /// Запис первинних (сирих) спостережень від окремих джерел до кластеризації
    pub fn record_raw_threats(&self, incoming: &[crate::models::Threat], now: f64) {
        let Some(conn) = &self.conn else {
            return;
        };
        let now_i = now as i64;
        let reports: Vec<track_storage::RawThreatReport> = incoming
            .iter()
            .map(|t| {
                let matched_track = self.member_to_track.get(&t.id).cloned();
                track_storage::RawThreatReport {
                    id: None,
                    observed_at: now_i,
                    source: t.source.clone(),
                    source_target_id: Some(t.id.clone()),
                    threat_type: t.threat_type.clone(),
                    lat: t.lat,
                    lon: t.lon,
                    heading: t.heading,
                    region: if t.region.is_empty() { None } else { Some(t.region.clone()) },
                    district: if t.district.is_empty() { None } else { Some(t.district.clone()) },
                    locality: if t.locality.is_empty() { None } else { Some(t.locality.clone()) },
                    confidence: Some(t.confidence.clone()),
                    target_count: t.target_count as i64,
                    matched_track_id: matched_track,
                }
            })
            .collect();

        let _ = track_storage::insert_raw_reports(conn, &reports);
    }

    /// Запис змін статусів повітряних тривог (початок, зміна рівня, відбій) для розмітки датасету
    pub fn record_alert_events(&mut self, alerts: &crate::parsers::alerts::Response, now: i64) {
        let Some(conn) = &self.conn else {
            return;
        };

        let mut current_active: HashMap<String, (String, String, String, Option<String>)> = HashMap::new();

        for r in &alerts.raions {
            if let Some(key) = &r.key {
                let name = r.name.as_deref().unwrap_or(key).to_string();
                let level = r.level.as_deref().unwrap_or("red").to_string();
                current_active.insert(key.clone(), (name, "raion".to_string(), level, r.since.clone()));
            }
        }
        for o in &alerts.oblasts {
            if let Some(key) = &o.key {
                let name = o.name.as_deref().unwrap_or(key).to_string();
                let level = o.level.as_deref().unwrap_or("red").to_string();
                current_active.insert(key.clone(), (name, "oblast".to_string(), level, o.since.clone()));
            }
        }

        // 1. Нові або змінені тривоги
        for (key, (name, reg_type, level, since)) in &current_active {
            let changed = match self.active_alert_levels.get(key) {
                Some(prev_level) => prev_level != level,
                None => true,
            };
            if changed {
                let _ = track_storage::insert_alert_event(
                    conn,
                    &track_storage::AlertEventRecord {
                        id: None,
                        observed_at: now,
                        region_id: key.clone(),
                        region_name: name.clone(),
                        region_type: reg_type.clone(),
                        alert_level: level.clone(),
                        is_active: true,
                        since: since.clone(),
                    },
                );
            }
        }

        // 2. Відбої тривог (були активні, але зникли зі списку)
        for (old_key, _) in &self.active_alert_levels {
            if !current_active.contains_key(old_key) {
                let _ = track_storage::insert_alert_event(
                    conn,
                    &track_storage::AlertEventRecord {
                        id: None,
                        observed_at: now,
                        region_id: old_key.clone(),
                        region_name: "".to_string(),
                        region_type: "".to_string(),
                        alert_level: "none".to_string(),
                        is_active: false,
                        since: None,
                    },
                );
            }
        }

        self.active_alert_levels = current_active.into_iter().map(|(k, (_, _, lvl, _))| (k, lvl)).collect();
    }

    /// Експорт датасету траєкторій для навчання ШІ (ML dataset API)
    pub fn export_dataset(&self, since_ts: Option<i64>, threat_type: Option<&str>, limit: Option<usize>) -> serde_json::Value {
        if let Some(conn) = &self.conn {
            track_storage::export_ml_dataset(conn, since_ts, threat_type, limit)
                .unwrap_or_else(|e| serde_json::json!({ "error": format!("dataset export failed: {e}") }))
        } else {
            serde_json::json!({ "error": "db not available" })
        }
    }

    /// Статистика датасету в базі
    pub fn dataset_stats(&self) -> serde_json::Value {
        if let Some(conn) = &self.conn {
            track_storage::get_dataset_stats(conn)
                .unwrap_or_else(|e| serde_json::json!({ "error": format!("stats failed: {e}") }))
        } else {
            serde_json::json!({ "error": "db not available" })
        }
    }

    /// Запис криптографічно підписаної згоди користувача
    pub fn record_consent(
        &self,
        consent_id: &str,
        public_key: &str,
        signature: &str,
        statement: &str,
        user_agent: Option<&str>,
        ip_address: Option<&str>,
        accepted_at: i64,
        version: &str,
    ) -> Result<(), String> {
        if let Some(conn) = &self.conn {
            track_storage::record_user_consent(
                conn,
                consent_id,
                public_key,
                signature,
                statement,
                user_agent,
                ip_address,
                accepted_at,
                version,
            ).map_err(|e| format!("db error: {e}"))
        } else {
            Err("db not available".to_string())
        }
    }

    /// Перевірка чи існує підтверджена згода за ID
    pub fn check_consent(&self, consent_id: &str) -> Result<bool, String> {
        if let Some(conn) = &self.conn {
            track_storage::check_user_consent(conn, consent_id)
                .map_err(|e| format!("db error: {e}"))
        } else {
            Err("db not available".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Threat;

    fn sample_cluster(threat_type: &str, confirmed: bool, sources: &[&str], lat: f64, lon: f64) -> Cluster {
        let members: Vec<Threat> = sources
            .iter()
            .map(|s| Threat {
                id: format!("{s}:test_id"),
                source: s.to_string(),
                threat_type: threat_type.to_string(),
                title: threat_type.to_string(),
                lat,
                lon,
                heading: Some(180.0),
                region: "Київська".to_string(),
                district: "".to_string(),
                locality: "Тест".to_string(),
                confidence: "high".to_string(),
                source_count: 1,
                target_count: 1,
                updated_at: None,
                area_only: false,
                advisory: false,
                trail: vec![],
            })
            .collect();

        Cluster {
            id: "initial_temp_id".to_string(),
            lat,
            lon,
            threat_type: threat_type.to_string(),
            title: threat_type.to_string(),
            label: threat_type.to_string(),
            region: "Київська".to_string(),
            locality: "Тест".to_string(),
            heading: Some(180.0),
            trail: vec![],
            members,
            sources: sources.iter().map(|s| s.to_string()).collect(),
            agreement: "".to_string(),
            spread_km: 2.5,
            confidence: "high".to_string(),
            confirmed,
            target_count: 1,
        }
    }

    fn sample_cluster_with_id(
        threat_type: &str,
        confirmed: bool,
        source: &str,
        raw_id: &str,
        lat: f64,
        lon: f64,
    ) -> Cluster {
        let members = vec![Threat {
            id: format!("{source}:{raw_id}"),
            source: source.to_string(),
            threat_type: threat_type.to_string(),
            title: threat_type.to_string(),
            lat,
            lon,
            heading: Some(180.0),
            region: "Миколаївська".to_string(),
            district: "".to_string(),
            locality: "Тест".to_string(),
            confidence: "high".to_string(),
            source_count: 1,
            target_count: 1,
            updated_at: None,
            area_only: false,
            advisory: false,
            trail: vec![],
        }];

        Cluster {
            id: "initial_temp_id".to_string(),
            lat,
            lon,
            threat_type: threat_type.to_string(),
            title: threat_type.to_string(),
            label: threat_type.to_string(),
            region: "Миколаївська".to_string(),
            locality: "Тест".to_string(),
            heading: Some(180.0),
            trail: vec![],
            members,
            sources: vec![source.to_string()],
            agreement: "".to_string(),
            spread_km: 2.5,
            confidence: "high".to_string(),
            confirmed,
            target_count: 1,
        }
    }

    #[test]
    fn test_persistent_uav_id_across_time() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        let mut tick1 = vec![
            sample_cluster("uav", true, &["dimap", "radarua", "neptun"], 50.0, 30.0),
            sample_cluster("uav", false, &["shahmap"], 48.0, 36.0),
        ];

        tracker.update(&mut tick1, 1000.0);
        assert_eq!(tick1[0].id, "uav_001");
        assert_eq!(tick1[1].id, "uav_002");

        // Tick 2: той самий перший борт змістився на 300 метрів (5 секунд польоту)
        let mut tick2 = vec![
            sample_cluster("uav", true, &["dimap", "radarua", "neptun"], 50.003, 30.001),
        ];
        tracker.update(&mut tick2, 1005.0);
        assert_eq!(tick2[0].id, "uav_001");

        // Tick 3: склад джерел змінився
        let mut tick3 = vec![
            sample_cluster("uav", false, &["dimap", "radarua"], 50.006, 30.002),
        ];
        tracker.update(&mut tick3, 1010.0);
        assert_eq!(tick3[0].id, "uav_001");

        // Tick 4: новий борт зовсім в іншій області
        let mut tick4 = vec![
            sample_cluster("uav_jet", true, &["radarua", "dimap", "neptun"], 51.5, 33.0),
        ];
        tracker.update(&mut tick4, 1015.0);
        assert_eq!(tick4[0].id, "uav_jet_001");
    }

    #[test]
    fn test_uav_trail_1hour_and_db_persistence() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        let mut tick1 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.0, 31.0),
        ];
        tracker.update(&mut tick1, 1000.0);

        let mut tick2 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.02, 31.01),
        ];
        tracker.update(&mut tick2, 1010.0);

        let mut tick3 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.04, 31.02),
        ];
        tracker.update(&mut tick3, 1020.0);

        // Перевірка наявності треку та точок у БД
        {
            let conn_ref = tracker.conn.as_ref().unwrap();
            let tr_db = track_storage::get_track(conn_ref, "uav_001").unwrap().unwrap();
            assert_eq!(tr_db.status, TrackStatus::Active);
            assert_eq!(tr_db.estimated_type, "Shahed-136");

            let pts_db = track_storage::get_track_points(conn_ref, "uav_001").unwrap();
            assert_eq!(pts_db.len(), 3);
            assert!(pts_db[1].speed.is_some());
        }

        // Перевірка формування GeoJSON хвостів
        let geo = tracker.uav_trails_geojson(1030.0, true);
        let feats = geo["features"].as_array().unwrap();
        assert_eq!(feats.len(), 1);
        assert_eq!(feats[0]["properties"]["id"], "uav_001");
        assert_eq!(feats[0]["properties"]["active"], true);

        // 4-та точка для uav_001
        let mut tick4 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.06, 31.03),
        ];
        tracker.update(&mut tick4, 1030.0);

        let pts_all = track_storage::get_track_points(tracker.conn.as_ref().unwrap(), "uav_001").unwrap();
        assert_eq!(pts_all.len(), 4);

        // Симуляція плину часу: минуло 2000 секунд (~33 хв) без оновлень
        // Трек переходить у статус неактивного (втрачений РЕБ), але хвіст усе ще видимий на карті
        let mut tick_mid = vec![];
        tracker.update(&mut tick_mid, 1030.0 + 2000.0);

        let tr_mid = tracker.tracks.get("uav_001").unwrap();
        assert_eq!(tr_mid.is_active, false);
        assert_eq!(tr_mid.status, TrackStatus::LostEw);
        assert_eq!(tr_mid.history.len(), 4);

        // Симуляція плину часу: минуло > 1 години (3605 секунд) з моменту останньої фіксації
        // Трек та його хвіст повністю прибираються з пам'яті карти
        let mut tick_late = vec![];
        tracker.update(&mut tick_late, 1030.0 + 3605.0);
        assert!(tracker.tracks.get("uav_001").is_none());

        // А в реляційній БД ВСІ 4 точки перманентно збереглися для історії та хронології!
        let pts_db_final = track_storage::get_track_points(tracker.conn.as_ref().unwrap(), "uav_001").unwrap();
        assert_eq!(pts_db_final.len(), 4);
    }

    #[test]
    fn test_lost_point_appears_and_resumes_when_returned() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        // 1. Борт з'явився і летить
        let mut tick1 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.0, 31.0),
        ];
        tracker.update(&mut tick1, 1000.0);
        assert_eq!(tick1[0].id, "uav_001");

        let mut tick2 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.03, 31.02),
        ];
        tracker.update(&mut tick2, 1010.0);

        let mut tick3 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.05, 31.03),
        ];
        tracker.update(&mut tick3, 1025.0);

        // Поки активний — кінцевої точки зникнення немає
        let lost1 = tracker.lost_points_geojson(1030.0, true);
        assert_eq!(lost1["features"].as_array().unwrap().len(), 0);

        // 2. Борт зник з радарів: минуло 200 секунд без оновлень (> expire_sec)
        let mut tick_empty = vec![];
        tracker.update(&mut tick_empty, 1230.0);

        let tr = tracker.tracks.get("uav_001").unwrap();
        assert_eq!(tr.is_active, false);
        assert_eq!(tr.status, TrackStatus::LostEw);

        // З'явилася кінцева точка де шахед зник!
        let lost2 = tracker.lost_points_geojson(1230.0, true);
        let lost_feats = lost2["features"].as_array().unwrap();
        assert_eq!(lost_feats.len(), 1);
        assert_eq!(lost_feats[0]["properties"]["id"], "uav_001");
        assert_eq!(lost_feats[0]["properties"]["status"], "втрачений РЕБ");
        let coords = lost_feats[0]["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(coords[0].as_f64().unwrap(), 31.03);
        assert_eq!(coords[1].as_f64().unwrap(), 49.05);

        // 3. Борт повернувся на радари! Через 5 хвилин (t = 1350) знову зафіксовано за 12 км далі
        let mut tick_return = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.08, 31.05),
        ];
        tracker.update(&mut tick_return, 1350.0);

        // Продовжуємо відслідковувати ТОЙ САМИЙ борт uav_001!
        assert_eq!(tick_return[0].id, "uav_001");
        let tr_resumed = tracker.tracks.get("uav_001").unwrap();
        assert_eq!(tr_resumed.is_active, true);
        assert_eq!(tr_resumed.status, TrackStatus::Active);

        // Кінцева точка зникнення знята з карти, оскільки борт знову активний!
        let lost3 = tracker.lost_points_geojson(1355.0, true);
        assert_eq!(lost3["features"].as_array().unwrap().len(), 0);

        // В базі статус знову 'активний', і всі 3 точки збережено
        let conn_ref = tracker.conn.as_ref().unwrap();
        let tr_db = track_storage::get_track(conn_ref, "uav_001").unwrap().unwrap();
        assert_eq!(tr_db.status, TrackStatus::Active);

        let pts = track_storage::get_track_points(conn_ref, "uav_001").unwrap();
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn test_two_uavs_50km_apart_never_teleport() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        // Berezyanka (46.866, 31.381) та Berezivka (47.20, 30.91) — відстань ~51.5 км
        let lat_berezyanka = 46.866;
        let lon_berezyanka = 31.381;
        let lat_berezivka = 47.200;
        let lon_berezivka = 30.910;

        // 1. У тік 1 з'являються обидва борти
        let mut tick1 = vec![
            sample_cluster_with_id("uav", true, "ukrainealarm", "target_berezyanka", lat_berezyanka, lon_berezyanka),
            sample_cluster_with_id("uav", true, "ukrainealarm", "target_berezivka", lat_berezivka, lon_berezivka),
        ];
        tracker.update(&mut tick1, 1000.0);

        assert_eq!(tick1[0].id, "uav_001");
        assert_eq!(tick1[1].id, "uav_002");

        // 2. Через 5 секунд борти зміщуються на реальну відстань польоту (180 км/год = ~0.25 км за 5 с)
        let mut tick2 = vec![
            sample_cluster_with_id("uav", true, "ukrainealarm", "target_berezyanka", lat_berezyanka + 0.002, lon_berezyanka + 0.002),
            sample_cluster_with_id("uav", true, "ukrainealarm", "target_berezivka", lat_berezivka + 0.002, lon_berezivka + 0.002),
        ];
        tracker.update(&mut tick2, 1005.0);

        // Перевіряємо що ID залишилися стабільними і не переплуталися
        assert_eq!(tick2[0].id, "uav_001");
        assert_eq!(tick2[1].id, "uav_002");

        // Перевіряємо що швидкості в історії не перевищують фізичні можливості Shahed (~150-250 км/год), а не 36,000 км/год!
        let tr1 = tracker.tracks.get("uav_001").unwrap();
        let tr2 = tracker.tracks.get("uav_002").unwrap();
        assert_eq!(tr1.history.len(), 2);
        assert_eq!(tr2.history.len(), 2);
        if let Some(spd) = tr1.history.last().unwrap().speed {
            assert!(spd < 300.0, "Speed is too high: {} km/h", spd);
        }
        if let Some(spd) = tr2.history.last().unwrap().speed {
            assert!(spd < 300.0, "Speed is too high: {} km/h", spd);
        }

        // 3. Тест на телепортацію при зникненні одного борту:
        // Якщо Berezyanka зникає, а Berezivka продовжує оновлюватися, Berezivka не повинна "притягнути" трек Berezyanka!
        let mut tick3 = vec![
            sample_cluster_with_id("uav", true, "ukrainealarm", "target_berezivka", lat_berezivka + 0.004, lon_berezivka + 0.004),
        ];
        tracker.update(&mut tick3, 1010.0);
        assert_eq!(tick3[0].id, "uav_002");

        // 4. Тест коли борт з'являється за 51 км без спільних id (наприклад, інше джерело або новий спам):
        // Через 5 секунд новий контакт за 50 км ніколи не повинен підтягуватися під uav_001
        let mut tick4 = vec![
            sample_cluster_with_id("uav", true, "radarua", "new_distant_contact", lat_berezyanka + 0.5, lon_berezyanka + 0.5),
        ];
        // 0.5 градуса ~ 55 км
        tracker.update(&mut tick4, 1015.0);
        // Повинен отримати новий id uav_003, а не телепортувати старий
        assert_eq!(tick4[0].id, "uav_003");
    }

    #[test]
    fn test_uav_trail_seamlessly_connects_to_active_uav_marker() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        let mut tick1 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.0, 31.0),
        ];
        tracker.update(&mut tick1, 1000.0);

        let mut tick2 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.02, 31.01),
        ];
        tracker.update(&mut tick2, 1010.0);

        // Перевіряємо що хвіст active БпЛА тягнеться акуратно до поточної координати маркера [31.01, 49.02]
        let geo = tracker.uav_trails_geojson(1015.0, true);
        let feats = geo["features"].as_array().unwrap();
        assert_eq!(feats.len(), 1);
        let coords = feats[0]["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(coords.len(), 2);
        let last_pt = coords.last().unwrap().as_array().unwrap();
        assert_eq!(last_pt[0].as_f64().unwrap(), 31.01);
        assert_eq!(last_pt[1].as_f64().unwrap(), 49.02);

        // Зміщення на третю точку
        let mut tick3 = vec![
            sample_cluster("uav", true, &["ukrainealarm"], 49.04, 31.02),
        ];
        tracker.update(&mut tick3, 1020.0);

        let geo3 = tracker.uav_trails_geojson(1025.0, true);
        let feats3 = geo3["features"].as_array().unwrap();
        let coords3 = feats3[0]["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(coords3.len(), 3);
        let last_pt3 = coords3.last().unwrap().as_array().unwrap();
        assert_eq!(last_pt3[0].as_f64().unwrap(), 31.02);
        assert_eq!(last_pt3[1].as_f64().unwrap(), 49.04);
    }

    #[test]
    fn test_official_source_continuity_and_no_teleport_fragmentation() {
        let conn = Connection::open_in_memory().unwrap();
        let mut tracker = TargetTracker::with_connection(conn);

        // 1. Офіційний Shahed-238 з'явився біля Монастириської (Чортків / Бучач)
        let mut tick1 = vec![
            sample_cluster_with_id("uav_jet", true, "ukrainealarm", "target_monastyryska_uuid", 49.15, 25.28),
        ];
        tracker.update(&mut tick1, 1000.0);
        assert_eq!(tick1[0].id, "uav_jet_001");

        // 2. Через 20 секунд змістився на 6 км до Підгайців
        let mut tick2 = vec![
            sample_cluster_with_id("uav_jet", true, "ukrainealarm", "target_monastyryska_uuid", 49.20, 25.10),
        ];
        tracker.update(&mut tick2, 1020.0);
        assert_eq!(tick2[0].id, "uav_jet_001");

        // 3. Через 40 секунд радар зафіксував оновлення біля Бурштина (18 км далі!)
        // Завдяки прямій синхронізації з офіційним джерелом трек НЕ розривається на новий uav_jet_002
        let mut tick3 = vec![
            sample_cluster_with_id("uav_jet", true, "ukrainealarm", "target_monastyryska_uuid", 49.29, 24.64),
        ];
        tracker.update(&mut tick3, 1060.0);
        assert_eq!(tick3[0].id, "uav_jet_001");

        // 4. Подальший рух на Рогатин (20 км далі)
        let mut tick4 = vec![
            sample_cluster_with_id("uav_jet", true, "ukrainealarm", "target_monastyryska_uuid", 49.44, 24.42),
        ];
        tracker.update(&mut tick4, 1120.0);
        assert_eq!(tick4[0].id, "uav_jet_001");

        // Перевіряємо що на карті існує рівно 1 неперервний хвіст польоту для цього борту
        let geo = tracker.uav_trails_geojson(1125.0, true);
        let feats = geo["features"].as_array().unwrap();
        assert_eq!(feats.len(), 1, "Має бути рівно 1 трек, без примарних розірваних шматків");
        assert_eq!(feats[0]["properties"]["id"], "uav_jet_001");
        assert_eq!(feats[0]["properties"]["points_count"], 4);

        // Кінцева точка хвоста закінчується точно в координатах Рогатина
        let coords = feats[0]["geometry"]["coordinates"].as_array().unwrap();
        let last_pt = coords.last().unwrap().as_array().unwrap();
        assert_eq!(last_pt[0].as_f64().unwrap(), 24.42);
        assert_eq!(last_pt[1].as_f64().unwrap(), 49.44);

        // Активних треків у системі рівно 1, ніяких фантомних дублікатів
        assert_eq!(tracker.tracks.len(), 1);
    }
}

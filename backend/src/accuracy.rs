//! Модуль оцінки точності радарних агрегаторів (Accuracy Engine) на базі SQLite.
//!
//! Зберігає історію спостережень за цілями (Шахеди та Реактиви) та розраховує
//! рейтинг точності карт у відсотках (0% - 100%) за періоди:
//! - 1 година (оперативна обстановка / поточний наліт);
//! - 24 години (доба);
//! - 7 днів (тиждень);
//! - 30 днів (місяць);
//! - весь час (глобальний рейтинг).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::borders::haversine_km;
use crate::models::Cluster;

pub const KNOWN_SOURCES: &[&str] = &["neptun", "dimap", "radarua", "aviacontrol", "shahmap", "kupol", "eradar"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAccuracy {
    pub source: String,
    pub rank: usize,
    /// Підсумковий бал точності у відсотках 0.0 .. 100.0%
    pub score: f64,
    /// Набрані бали (+100 за підтверджений дрон + до 20 за точність координат)
    pub earned_points: i64,
    /// Штрафні бали (-30 за БПЛА без тривоги, -15 за фантом без курсу)
    pub penalty_points: i64,
    /// Чисті підсумкові бали = max(0, earned - penalty)
    pub net_points: i64,
    /// Максимально можливі ідеальні бали за період (total_confirmed_clusters * 120)
    pub max_possible_points: i64,
    /// Валідність цілей (частка підтверджених консенсусом серед усіх виданих цим джерелом)
    pub precision: f64,
    /// Повнота охоплення (частка підтверджених груп у небі, які засікло це джерело)
    pub recall: f64,
    /// Геодезична точність координат (0..100%, 100% = 0 км похибки, 0% = 55+ км)
    pub spatial_score: f64,
    /// Середнє відхилення координат від центру консенсусу в кілометрах
    pub avg_spatial_error_km: f64,
    /// Стабільність джерела (0..100%, відсутність аномалій та стрибків)
    pub stability_score: f64,
    /// Кількість виявлених підтверджених цілей
    pub confirmed_detected: usize,
    /// Загальна кількість засічок/цілей джерела
    pub total_reports: usize,
    /// Кількість БПЛА без оголошеної тривоги (штрафні інциденти)
    pub no_alert_violations: usize,
    /// Кількість непідтверджених БПЛА без курсу / статичних фантомів
    pub static_phantom_violations: usize,
    /// Нараховані штрафні бали
    pub penalty_score: f64,
    /// Кількість підтверджених картою офіційних сповіщень ПС ЗСУ
    pub official_matches: usize,
    /// Загальна кількість релевантних офіційних сповіщень ПС ЗСУ за період
    pub official_total: usize,
    /// Відсоток відповідності офіційним джерелам (Official Concordance %)
    pub official_concordance_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccuracyReport {
    pub period: String,
    pub threat_type_filter: String,
    pub generated_at: String,
    pub total_confirmed_clusters: usize,
    pub total_active_sources: usize,
    pub total_official_notices: usize,
    pub leader: Option<String>,
    pub sources: Vec<SourceAccuracy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodologyMetric {
    pub name: String,
    pub weight_pct: u32,
    pub description: String,
    pub formula: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodologyDoc {
    pub title: String,
    pub version: String,
    pub summary: String,
    pub target_types: Vec<String>,
    pub periods: Vec<String>,
    pub metrics: Vec<MethodologyMetric>,
    pub consensus_rule: String,
    pub composite_formula: String,
}

pub struct AccuracyEngine {
    conn: Mutex<Connection>,
}

impl AccuracyEngine {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, String> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| format!("mkdir db dir failed: {e}"))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| format!("open sqlite failed: {e}"))?;
        
        // Налаштування для максимальної швидкодії
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -64000;",
        )
        .map_err(|e| format!("pragma failed: {e}"))?;

        let engine = Self {
            conn: Mutex::new(conn),
        };
        engine.init_schema()?;
        Ok(engine)
    }

    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cluster_observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                observed_at INTEGER NOT NULL,
                cluster_id TEXT NOT NULL,
                threat_type TEXT NOT NULL,
                confirmed INTEGER NOT NULL,
                sources_json TEXT NOT NULL,
                source_count INTEGER NOT NULL,
                spread_km REAL NOT NULL,
                lat REAL NOT NULL,
                lon REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_obs_time ON cluster_observations(observed_at);
            CREATE INDEX IF NOT EXISTS idx_obs_type ON cluster_observations(threat_type);
            CREATE INDEX IF NOT EXISTS idx_obs_cluster ON cluster_observations(cluster_id);

            CREATE TABLE IF NOT EXISTS member_deviations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                observed_at INTEGER NOT NULL,
                cluster_id TEXT NOT NULL,
                threat_type TEXT NOT NULL,
                source TEXT NOT NULL,
                deviation_km REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dev_time ON member_deviations(observed_at);
            CREATE INDEX IF NOT EXISTS idx_dev_source ON member_deviations(source);

            CREATE TABLE IF NOT EXISTS hourly_source_stats (
                hour_timestamp INTEGER NOT NULL,
                source TEXT NOT NULL,
                threat_type TEXT NOT NULL,
                total_reports INTEGER NOT NULL,
                confirmed_reports INTEGER NOT NULL,
                total_clusters_present INTEGER NOT NULL,
                detected_clusters INTEGER NOT NULL,
                sum_spatial_error_km REAL NOT NULL,
                error_samples INTEGER NOT NULL,
                PRIMARY KEY (hour_timestamp, source, threat_type)
            );
            CREATE INDEX IF NOT EXISTS idx_hourly_time ON hourly_source_stats(hour_timestamp);

            CREATE TABLE IF NOT EXISTS no_alert_violations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                observed_at INTEGER NOT NULL,
                cluster_id TEXT NOT NULL,
                threat_type TEXT NOT NULL,
                source TEXT NOT NULL,
                lat REAL NOT NULL,
                lon REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_viol_time ON no_alert_violations(observed_at);
            CREATE INDEX IF NOT EXISTS idx_viol_source ON no_alert_violations(source);
            CREATE INDEX IF NOT EXISTS idx_viol_cluster ON no_alert_violations(cluster_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_viol_unique ON no_alert_violations(cluster_id, source);

            CREATE TABLE IF NOT EXISTS static_phantom_violations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                observed_at INTEGER NOT NULL,
                cluster_id TEXT NOT NULL,
                threat_type TEXT NOT NULL,
                source TEXT NOT NULL,
                lat REAL NOT NULL,
                lon REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_static_viol_time ON static_phantom_violations(observed_at);
            CREATE INDEX IF NOT EXISTS idx_static_viol_source ON static_phantom_violations(source);
            CREATE INDEX IF NOT EXISTS idx_static_viol_cluster ON static_phantom_violations(cluster_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_static_viol_unique ON static_phantom_violations(cluster_id, source);

            CREATE TABLE IF NOT EXISTS official_notices_archive (
                id TEXT PRIMARY KEY,
                observed_at INTEGER NOT NULL,
                channel TEXT NOT NULL,
                post_url TEXT,
                kind TEXT NOT NULL,
                oblast TEXT,
                lat REAL,
                lon REAL,
                raw_text TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_off_notices_time ON official_notices_archive(observed_at);

            CREATE TABLE IF NOT EXISTS official_source_matches (
                notice_id TEXT NOT NULL,
                source TEXT NOT NULL,
                matched INTEGER NOT NULL,
                target_count INTEGER NOT NULL,
                min_distance_km REAL,
                observed_at INTEGER NOT NULL,
                PRIMARY KEY (notice_id, source)
            );
            CREATE INDEX IF NOT EXISTS idx_off_matches_source ON official_source_matches(source);
            CREATE INDEX IF NOT EXISTS idx_off_matches_time ON official_source_matches(observed_at);",
        )
        .map_err(|e| format!("init schema failed: {e}"))?;

        crate::track_storage::init_schema(&conn)
            .map_err(|e| format!("init track_storage schema failed: {e}"))?;

        Ok(())
    }

    /// Запис стану кластерів у базу (викликається щоциклу опитування)
    pub fn record_clusters(
        &self,
        clusters: &[Cluster],
        alerts: &crate::parsers::alerts::Response,
        bounds: &crate::parsers::alerts::Bounds,
        now_secs: f64,
    ) {
        let now_int = now_secs as i64;
        let shahed_clusters: Vec<&Cluster> = clusters
            .iter()
            .filter(|c| c.threat_type == "uav" || c.threat_type == "uav_jet")
            .collect();

        let Ok(mut conn) = self.conn.lock() else {
            return;
        };

        let Ok(tx) = conn.transaction() else {
            return;
        };

        if !shahed_clusters.is_empty() {
            let mut stmt_obs = match tx.prepare_cached(
                "INSERT INTO cluster_observations (
                    observed_at, cluster_id, threat_type, confirmed, sources_json,
                    source_count, spread_km, lat, lon
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            ) {
                Ok(s) => s,
                Err(_) => return,
            };

            let mut stmt_dev = match tx.prepare_cached(
                "INSERT INTO member_deviations (
                    observed_at, cluster_id, threat_type, source, deviation_km
                ) VALUES (?1, ?2, ?3, ?4, ?5)",
            ) {
                Ok(s) => s,
                Err(_) => return,
            };

            let mut stmt_viol = match tx.prepare_cached(
                "INSERT OR IGNORE INTO no_alert_violations (
                    observed_at, cluster_id, threat_type, source, lat, lon
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ) {
                Ok(s) => s,
                Err(_) => return,
            };

            let mut stmt_static_viol = match tx.prepare_cached(
                "INSERT OR IGNORE INTO static_phantom_violations (
                    observed_at, cluster_id, threat_type, source, lat, lon
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ) {
                Ok(s) => s,
                Err(_) => return,
            };

            for c in &shahed_clusters {
                // Якщо борт підтверджений кількома картами:
                // 1) Скасовуємо для нього статус статичного фантома
                // 2) Скасовуємо штраф за появу без тривоги (консенсус доводить реальність загрози)
                if c.confirmed {
                    let _ = tx.execute(
                        "DELETE FROM static_phantom_violations WHERE cluster_id = ?1",
                        params![&c.id],
                    );
                    let _ = tx.execute(
                        "DELETE FROM no_alert_violations WHERE cluster_id = ?1",
                        params![&c.id],
                    );
                }

                let sources_json = serde_json::to_string(&c.sources).unwrap_or_default();
                let _ = stmt_obs.execute(params![
                    now_int,
                    &c.id,
                    &c.threat_type,
                    if c.confirmed { 1 } else { 0 },
                    &sources_json,
                    c.sources.len() as i64,
                    c.spread_km,
                    c.lat,
                    c.lon,
                ]);

                // Запис похибок кожного джерела відносно центру кластера
                for m in &c.members {
                    let dev = haversine_km(m.lat, m.lon, c.lat, c.lon);
                    let _ = stmt_dev.execute(params![
                        now_int,
                        &c.id,
                        &c.threat_type,
                        &m.source,
                        dev,
                    ]);

                    // Штраф 1: якщо БПЛА з'являється на території України, де не оголошено жовтий чи червоний рівень.
                    // ВАЖЛИВО: підтверджені консенсусом цілі (c.confirmed) НІКОЛИ не штрафуються,
                    // оскільки консенсус 3+ джерел доводить реальність загрози (офіційні тривоги часто запізнюються на 1-5 хв).
                    if !c.confirmed
                        && crate::borders::inside_ukraine(m.lat, m.lon)
                        && !crate::parsers::alerts::is_point_in_alert_zone(m.lat, m.lon, alerts, bounds)
                    {
                        let _ = stmt_viol.execute(params![
                            now_int,
                            &c.id,
                            &c.threat_type,
                            &m.source,
                            m.lat,
                            m.lon,
                        ]);
                    }

                    // Штраф 2: непідтверджений БПЛА без курсу (зависла мітка / слуховий репорт без швидкості)
                    if !c.confirmed && m.heading.is_none() {
                        let _ = stmt_static_viol.execute(params![
                            now_int,
                            &c.id,
                            &c.threat_type,
                            &m.source,
                            m.lat,
                            m.lon,
                        ]);
                    }
                }
            }
        }

        // Амністія раннього виявлення (Early Warning Cooldown / Amnesty):
        // Якщо радарна карта попередила про загрозу раніше за сирену, і протягом останніх 10 хвилин
        // у цьому районі/області з'явилася жовта або червона тривога — це було успішне завчасне
        // виявлення, а не фантом! Скасовуємо штраф.
        // Завжди виконується, незалежно від наявності нових кластерів у поточному тіку.
        let cooldown_window = now_int - 600; // 10 хвилин кулдаун
        let mut amnestied_ids = Vec::new();
        if let Ok(mut stmt_check) = tx.prepare(
            "SELECT id, lat, lon FROM no_alert_violations WHERE observed_at >= ?1"
        ) {
            if let Ok(mut rows) = stmt_check.query(params![cooldown_window]) {
                while let Ok(Some(row)) = rows.next() {
                    let id: i64 = row.get(0).unwrap_or(0);
                    let lat: f64 = row.get(1).unwrap_or(0.0);
                    let lon: f64 = row.get(2).unwrap_or(0.0);
                    if crate::parsers::alerts::is_point_in_alert_zone(lat, lon, alerts, bounds) {
                        amnestied_ids.push(id);
                    }
                }
            }
        }
        for id in amnestied_ids {
            let _ = tx.execute("DELETE FROM no_alert_violations WHERE id = ?1", params![id]);
        }

        let _ = tx.commit();
    }

    /// Збереження та крос-верифікація офіційних сповіщень ПС ЗСУ з картами
    pub fn record_official_notices(
        &self,
        notices: &[crate::parsers::official::Notice],
        tracked: &[crate::models::Threat],
        _now_secs: f64,
    ) {
        if notices.is_empty() {
            return;
        }

        let Ok(mut conn) = self.conn.lock() else { return; };
        let Ok(tx) = conn.transaction() else { return; };

        let active_sources = ["dimap", "shahmap", "radarua", "neptun", "aviacontrol", "kupol", "eradar"];

        for n in notices {
            let obs_ts = n.observed_at.timestamp();
            // Зберігаємо кожне сповіщення в архів
            let _ = tx.execute(
                "INSERT OR IGNORE INTO official_notices_archive
                 (id, observed_at, channel, post_url, kind, oblast, lat, lon, raw_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    &n.id,
                    obs_ts,
                    &n.channel,
                    &n.post_url,
                    &n.kind,
                    &n.oblast,
                    n.lat,
                    n.lon,
                    &n.text,
                ],
            );

            // Крос-верифікація: тільки якщо сповіщення має географічну прив'язку (область або координати)
            if n.oblast.is_none() && n.lat.is_none() {
                continue;
            }

            // Перевіряємо засічки кожної карти
            for src in &active_sources {
                let mut matched = false;
                let mut target_count = 0usize;
                let mut min_dist = 999.0f64;

                for t in tracked {
                    if t.source.to_lowercase() != *src {
                        continue;
                    }

                    // Збіг за типом загрози
                    let type_matches = match n.kind.as_str() {
                        "uav" => t.threat_type == "uav" || t.threat_type == "uav_jet",
                        "missile" => t.threat_type == "missile",
                        "ballistic" => t.threat_type == "ballistic",
                        "kab" => t.threat_type == "kab",
                        "tactical" | "mig31k" => t.threat_type == "tactical" || t.threat_type == "mig31k",
                        _ => true,
                    };
                    if !type_matches {
                        continue;
                    }

                    // Збіг за локацією: або однакова область, або дистанція < 75 км
                    let mut loc_match = false;
                    if let Some(ref obl) = n.oblast {
                        let obl_lower = obl.to_lowercase();
                        let reg_lower = t.region.to_lowercase();
                        if !reg_lower.is_empty() && (reg_lower.contains(&obl_lower) || obl_lower.contains(&reg_lower)) {
                            loc_match = true;
                        }
                    }

                    if !loc_match {
                        if let (Some(n_lat), Some(n_lon)) = (n.lat, n.lon) {
                            let d = haversine_km(t.lat, t.lon, n_lat, n_lon);
                            if d <= 75.0 {
                                loc_match = true;
                                if d < min_dist {
                                    min_dist = d;
                                }
                            }
                        }
                    }

                    if loc_match {
                        matched = true;
                        target_count += 1;
                    }
                }

                if matched {
                    let _ = tx.execute(
                        "INSERT INTO official_source_matches
                         (notice_id, source, matched, target_count, min_distance_km, observed_at)
                         VALUES (?1, ?2, 1, ?3, ?4, ?5)
                         ON CONFLICT(notice_id, source) DO UPDATE SET
                         matched = 1,
                         target_count = MAX(target_count, excluded.target_count),
                         min_distance_km = MIN(min_distance_km, excluded.min_distance_km)",
                        rusqlite::params![&n.id, src, target_count as i64, min_dist, obs_ts],
                    );
                } else {
                    // Фіксуємо початковий статус (якщо ще немає запису)
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO official_source_matches
                         (notice_id, source, matched, target_count, min_distance_km, observed_at)
                         VALUES (?1, ?2, 0, 0, 999.0, ?3)",
                        rusqlite::params![&n.id, src, obs_ts],
                    );
                }
            }
        }

        let _ = tx.commit();
    }

    /// Обчислення оцінки точності за вибраний період та тип
    pub fn calculate_report(&self, period: &str, threat_type: &str, now_secs: f64) -> Result<AccuracyReport, String> {
        let now = now_secs as i64;
        let since = match period {
            "1h" => now - 3600,
            "24h" => now - 86400,
            "7d" => now - 7 * 86400,
            "30d" => now - 30 * 86400,
            "all" => 0,
            _ => now - 3600, // за замовчуванням 1 година
        };

        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // 1. Отримуємо унікальні кластери та їх джерела за період
        let type_clause = match threat_type {
            "uav" => "AND threat_type = 'uav'",
            "uav_jet" => "AND threat_type = 'uav_jet'",
            _ => "AND (threat_type = 'uav' OR threat_type = 'uav_jet')",
        };

        let query = format!(
            "SELECT cluster_id, confirmed, sources_json, threat_type
             FROM cluster_observations
             WHERE observed_at >= ?1 {type_clause}"
        );

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since], |row| {
                let id: String = row.get(0)?;
                let confirmed: i64 = row.get(1)?;
                let sources_json: String = row.get(2)?;
                let t_type: String = row.get(3)?;
                Ok((id, confirmed == 1, sources_json, t_type))
            })
            .map_err(|e| e.to_string())?;

        // cluster_id -> (is_confirmed, Set<source>)
        let mut cluster_map: HashMap<String, (bool, HashSet<String>)> = HashMap::new();
        for r in rows.flatten() {
            let (cid, conf, s_json, _) = r;
            let sources: Vec<String> = serde_json::from_str(&s_json).unwrap_or_default();
            let entry = cluster_map.entry(cid).or_insert((conf, HashSet::new()));
            if conf {
                entry.0 = true;
            }
            for s in sources {
                entry.1.insert(s);
            }
        }

        // 2. Отримуємо середні геодезичні відхилення для кожного джерела
        let dev_query = format!(
            "SELECT source, AVG(deviation_km), COUNT(*)
             FROM member_deviations
             WHERE observed_at >= ?1 {type_clause}
             GROUP BY source"
        );
        let mut dev_map: HashMap<String, (f64, usize)> = HashMap::new();
        if let Ok(mut dev_stmt) = conn.prepare(&dev_query) {
            if let Ok(dev_rows) = dev_stmt.query_map(params![since], |row| {
                let s: String = row.get(0)?;
                let avg_dev: f64 = row.get(1)?;
                let count: i64 = row.get(2)?;
                Ok((s, avg_dev, count as usize))
            }) {
                for dr in dev_rows.flatten() {
                    dev_map.insert(dr.0, (dr.1, dr.2));
                }
            }
        }

        // 3. Отримуємо штрафні інциденти: БПЛА на території України без тривоги
        let viol_query = format!(
            "SELECT source, COUNT(*)
             FROM no_alert_violations
             WHERE observed_at >= ?1 {type_clause}
             GROUP BY source"
        );
        let mut viol_map: HashMap<String, usize> = HashMap::new();
        if let Ok(mut viol_stmt) = conn.prepare(&viol_query) {
            if let Ok(viol_rows) = viol_stmt.query_map(params![since], |row| {
                let s: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((s, count as usize))
            }) {
                for vr in viol_rows.flatten() {
                    viol_map.insert(vr.0, vr.1);
                }
            }
        }

        // 4. Отримуємо непідтверджені статичні фантоми без курсу
        let static_viol_query = format!(
            "SELECT source, COUNT(*)
             FROM static_phantom_violations
             WHERE observed_at >= ?1 {type_clause}
             GROUP BY source"
        );
        let mut static_viol_map: HashMap<String, usize> = HashMap::new();
        if let Ok(mut static_stmt) = conn.prepare(&static_viol_query) {
            if let Ok(static_rows) = static_stmt.query_map(params![since], |row| {
                let s: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((s, count as usize))
            }) {
                for svr in static_rows.flatten() {
                    static_viol_map.insert(svr.0, svr.1);
                }
            }
        }

        let total_confirmed_clusters = cluster_map.values().filter(|(conf, _)| *conf).count();
        // Ідеальні максимально можливі бали за період:
        // Кожен підтверджений кластер дає 100 балів + до 20 балів за ідеальну точність (120 балів максимум)
        let max_possible_points = (total_confirmed_clusters as i64) * 120;

        // 5. Офіційні сповіщення ПС ЗСУ та перевірка збігів
        let total_official_notices: usize = conn
            .query_row(
                "SELECT COUNT(DISTINCT notice_id) FROM official_source_matches WHERE observed_at >= ?1",
                params![since],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;

        let mut official_matches_map: HashMap<String, (usize, usize)> = HashMap::new();
        if total_official_notices > 0 {
            if let Ok(mut off_stmt) = conn.prepare(
                "SELECT source, COUNT(*), COALESCE(SUM(matched), 0)
                 FROM official_source_matches
                 WHERE observed_at >= ?1
                 GROUP BY source",
            ) {
                if let Ok(rows) = off_stmt.query_map(params![since], |row| {
                    let src: String = row.get(0)?;
                    let total: i64 = row.get(1)?;
                    let matched: i64 = row.get(2)?;
                    Ok((src, (total as usize, matched as usize)))
                }) {
                    for r in rows.flatten() {
                        official_matches_map.insert(r.0, r.1);
                    }
                }
            }
        }

        // 6. Розрахунок метрик для кожного джерела
        let mut source_stats: Vec<SourceAccuracy> = Vec::new();

        for &src in KNOWN_SOURCES {
            let mut detected_confirmed = 0;
            let mut total_reports = 0;

            for (conf, src_set) in cluster_map.values() {
                if src_set.contains(src) {
                    total_reports += 1;
                    if *conf {
                        detected_confirmed += 1;
                    }
                }
            }

            // Якщо спостережень немає взагалі — початкові нейтральні метрики
            let precision = if total_reports > 0 {
                (detected_confirmed as f64 / total_reports as f64) * 100.0
            } else {
                0.0
            };

            let recall = if total_confirmed_clusters > 0 {
                (detected_confirmed as f64 / total_confirmed_clusters as f64) * 100.0
            } else if total_reports > 0 {
                50.0
            } else {
                0.0
            };

            let (avg_dev_km, _) = dev_map.get(src).copied().unwrap_or((8.0, 0));
            // 0 км = 100%, 55 км (макс. радіус злиття шахеда) = 0%
            let spatial_score = (100.0 - (avg_dev_km / 55.0) * 100.0).clamp(0.0, 100.0);

            // Бонус за геодезичну точність (від 0 до 20 балів на кожну виявлену підтверджену ціль)
            let spatial_bonus = (20.0 * (1.0 - (avg_dev_km / 50.0).clamp(0.0, 1.0))).clamp(0.0, 20.0);
            let earned_points = ((detected_confirmed as f64) * (100.0 + spatial_bonus)).round() as i64;

            // Стабільність: базовий рівень 100%, знижується за низької активності
            let stability_score: f64 = if total_reports > 0 { 95.0 } else { 20.0 };

            let no_alert_violations = viol_map.get(src).copied().unwrap_or(0);
            let static_phantom_violations = static_viol_map.get(src).copied().unwrap_or(0);

            // Накопичувальні штрафні бали за добу:
            // -30 балів за кожен випадок БПЛА над мирною Україною без тривоги (дезінформація)
            // -15 балів за кожен непідтверджений БПЛА без курсу (завислий фантом / чутки без швидкості)
            let penalty_points = (no_alert_violations as i64) * 30 + (static_phantom_violations as i64) * 15;
            let net_points = (earned_points - penalty_points).max(0);

            // Підсумковий зважений бал (Score %):
            // Відношення зароблених чистих балів до ідеального максимуму доби
            let base_score = if max_possible_points > 0 {
                ((net_points as f64 / max_possible_points as f64) * 100.0).clamp(0.0, 100.0)
            } else if total_reports > 0 && penalty_points == 0 {
                50.0
            } else if penalty_points > 0 {
                0.0
            } else {
                0.0
            };

            let (off_total, off_matches) = official_matches_map.get(src).copied().unwrap_or((0, 0));
            let official_concordance_pct = if off_total > 0 {
                ((off_matches as f64 / off_total as f64) * 100.0 * 10.0).round() / 10.0
            } else {
                100.0
            };

            // Якщо накопичено щонайменше 5 офіційних сповіщень, враховуємо збіг з ПС ЗСУ як 25% ваги
            let final_score = if off_total >= 5 {
                (base_score * 0.75 + official_concordance_pct * 0.25).clamp(0.0, 100.0)
            } else {
                base_score
            };

            source_stats.push(SourceAccuracy {
                source: src.to_string(),
                rank: 0,
                score: (final_score * 10.0).round() / 10.0,
                earned_points,
                penalty_points,
                net_points,
                max_possible_points,
                precision: (precision * 10.0).round() / 10.0,
                recall: (recall * 10.0).round() / 10.0,
                spatial_score: (spatial_score * 10.0).round() / 10.0,
                avg_spatial_error_km: (avg_dev_km * 10.0).round() / 10.0,
                stability_score: (stability_score * 10.0).round() / 10.0,
                confirmed_detected: detected_confirmed,
                total_reports,
                no_alert_violations,
                static_phantom_violations,
                penalty_score: penalty_points as f64,
                official_matches: off_matches,
                official_total: off_total,
                official_concordance_pct,
            });
        }

        // Сортування за підсумковим балом
        source_stats.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        for (idx, item) in source_stats.iter_mut().enumerate() {
            item.rank = idx + 1;
        }

        let leader = source_stats.first().filter(|s| s.score > 0.0).map(|s| s.source.clone());
        let active_sources = source_stats.iter().filter(|s| s.total_reports > 0).count();

        Ok(AccuracyReport {
            period: period.to_string(),
            threat_type_filter: threat_type.to_string(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            total_confirmed_clusters,
            total_active_sources: active_sources,
            total_official_notices,
            leader,
            sources: source_stats,
        })
    }

    /// Опис методології оцінювання для UI та документації
    pub fn methodology() -> MethodologyDoc {
        MethodologyDoc {
            title: "Оцінка точності радарних агрегаторів TACMAP".to_string(),
            version: "2.0.0 (Points System)".to_string(),
            summary: "Бальна система за добу (24h): за кожен підтверджений дрон нараховується +100 балів (+ до 20 балів за точність координат). За фантоми нараховуються штрафи (-30 балів за БПЛА без тривоги, -15 за фантом без курсу). Рейтинг у відсотках відображає частку чистих балів від ідеалу доби.".to_string(),
            target_types: vec![
                "uav (Ударні Шахеди-136 / Герань-2)".to_string(),
                "uav_jet (Швидкісні реактивні шахеди)".to_string(),
            ],
            periods: vec![
                "1h (Остання 1 година — оперативна обстановка поточного нальоту)".to_string(),
                "24h (Останні 24 години — добовий підсумок балів)".to_string(),
                "7d (Останні 7 днів — тижнева стабільність)".to_string(),
                "30d (Останні 30 днів — місячний звіт)".to_string(),
                "all (Весь час спостережень)".to_string(),
            ],
            consensus_rule: "Ціль вважається 100% справжньою, коли щонайменше 3 різні карти бачать її поруч (до 55 км) зі спільним курсом руху.".to_string(),
            composite_formula: "Відсоток = (Чисті бали / Ідеальні можливі бали доби) × 100%. Чисті бали = Зароблені (+100 за підтверджений дрон + до 20 за точність) − Штрафи (-30 без тривоги, -15 без курсу).".to_string(),
            metrics: vec![
                MethodologyMetric {
                    name: "◈ Накопичені бали за цілі".to_string(),
                    weight_pct: 50,
                    description: "За кожен підтверджений консенсусом дрон у небі карта отримує +100 балів. Карти, які бачать усі дрони, отримують максимум балів.".to_string(),
                    formula: "+100 балів за кожну виявлену підтверджену ціль.".to_string(),
                },
                MethodologyMetric {
                    name: "⊕ Бонус за геодезичну точність".to_string(),
                    weight_pct: 20,
                    description: "Що ближче координати карти до центру консенсусу — то більший бонус (до +20 балів на кожну виявлену ціль).".to_string(),
                    formula: "Бонус = 20 × (1 − Похибка_км / 50 км) балів на ціль.".to_string(),
                },
                MethodologyMetric {
                    name: "⌖ Правдивість (без вигадок)".to_string(),
                    weight_pct: 15,
                    description: "Частка підтверджених дронів серед усіх показаних цілей. Якщо карта домальовує зайві мітки, вони не приносять балів.".to_string(),
                    formula: "Правдивість = (Підтверджені цілі / Всі показані картою) × 100%.".to_string(),
                },
                MethodologyMetric {
                    name: "◉ Уважність (охоплення неба)".to_string(),
                    weight_pct: 15,
                    description: "Відсоток справжніх дронів у небі, які помітила ця карта.".to_string(),
                    formula: "Уважність = (Помічені картою дрони / Всі дрони в небі) × 100%.".to_string(),
                },
                MethodologyMetric {
                    name: "[ ! ] Штраф за цілі без тривоги".to_string(),
                    weight_pct: 0,
                    description: "Якщо карта показує дрон над Україною в зоні, де не оголошено ані жовтий рівень (дронова загроза), ані червоний рівень (повітряна тривога) — це фантомна мітка. Стягується штраф -30 балів за кожен випадок.".to_string(),
                    formula: "Штраф = Кількість БПЛА без тривоги × 30 балів (віднімається від зароблених балів).".to_string(),
                },
                MethodologyMetric {
                    name: "[ ! ] Штраф за фантоми без курсу".to_string(),
                    weight_pct: 0,
                    description: "Шахед є літаковим БПЛА і не може зависати на місці. Якщо карта публікує дрон без курсу/швидкості, і його не бачать інші карти — це завислий фантом. Стягується штраф -15 балів за кожен випадок.".to_string(),
                    formula: "Штраф = Кількість фантомів без курсу × 15 балів (віднімається від зароблених балів).".to_string(),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Threat;

    fn sample_cluster(id: &str, t_type: &str, confirmed: bool, sources: &[&str], lat: f64, lon: f64) -> Cluster {
        let members: Vec<Threat> = sources
            .iter()
            .map(|s| Threat {
                id: format!("{s}:{id}"),
                source: s.to_string(),
                threat_type: t_type.to_string(),
                title: t_type.to_string(),
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
            id: id.to_string(),
            lat,
            lon,
            threat_type: t_type.to_string(),
            title: t_type.to_string(),
            label: t_type.to_string(),
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

    #[test]
    fn test_accuracy_engine_workflow() {
        let temp_dir = std::env::temp_dir().join(format!("test_acc_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let db_path = temp_dir.join("test.db");
        let engine = AccuracyEngine::new(&db_path).unwrap();

        let now = 100_000.0;
        let c1 = sample_cluster("c1", "uav", true, &["dimap", "radarua", "neptun"], 50.0, 30.0);
        let c2 = sample_cluster("c2", "uav_jet", true, &["dimap", "radarua"], 50.1, 30.1);
        // c3 знаходиться всередині України (lat 49.0, lon 24.5 - Івано-Франківщина), але там тривоги немає!
        let c3 = sample_cluster("c3", "uav", false, &["shahmap"], 49.0, 24.5);
        // c4 знаходиться в зоні тривоги (Київ), але neptun видав його без курсу (heading: None) і не підтверджено іншими
        let mut c4 = sample_cluster("c4", "uav", false, &["neptun"], 50.2, 30.2);
        c4.heading = None;
        for m in &mut c4.members {
            m.heading = None;
        }

        // Область з тривогою навколо Києва [30.0, 50.0]
        let alerts = crate::parsers::alerts::Response {
            raions: vec![],
            oblasts: vec![
                crate::parsers::alerts::AlertEntry {
                    key: Some("київська".to_string()),
                    level: Some("red".to_string()),
                    ..Default::default()
                }
            ],
        };
        let bounds = crate::parsers::alerts::Bounds {
            raions: HashMap::new(),
            oblasts: [(
                "київська".to_string(),
                serde_json::json!({
                    "type": "Polygon",
                    "coordinates": [[[29.0, 49.0], [32.0, 49.0], [32.0, 52.0], [29.0, 52.0], [29.0, 49.0]]]
                }),
            )]
            .into_iter()
            .collect(),
            loaded_at: 100.0,
        };

        engine.record_clusters(&[c1, c2, c3, c4], &alerts, &bounds, now);

        let report_1h = engine.calculate_report("1h", "all", now).unwrap();
        assert_eq!(report_1h.period, "1h");
        assert_eq!(report_1h.total_confirmed_clusters, 2);
        assert!(!report_1h.sources.is_empty());

        // dimap та radarua брали участь у підтверджених кластерах у зоні тривоги
        let dimap = report_1h.sources.iter().find(|s| s.source == "dimap").unwrap();
        assert!(dimap.score > 70.0);
        assert_eq!(dimap.confirmed_detected, 2);
        assert_eq!(dimap.no_alert_violations, 0);
        assert_eq!(dimap.static_phantom_violations, 0);
        assert_eq!(dimap.penalty_score, 0.0);

        // shahmap опублікував БПЛА над Україною у спокійному регіоні без тривоги -> отримав штраф 30 балів!
        let shahmap = report_1h.sources.iter().find(|s| s.source == "shahmap").unwrap();
        assert_eq!(shahmap.no_alert_violations, 1);
        assert_eq!(shahmap.penalty_score, 30.0);
        assert_eq!(shahmap.penalty_points, 30);

        // neptun видав непідтверджений БПЛА без курсу -> отримав штраф 15 балів за статичний фантом!
        let neptun = report_1h.sources.iter().find(|s| s.source == "neptun").unwrap();
        assert_eq!(neptun.static_phantom_violations, 1);
        assert_eq!(neptun.penalty_score, 15.0);
        assert_eq!(neptun.penalty_points, 15);

        // Перевірка збереження та крос-верифікації офіційних сповіщень ПС ЗСУ
        let notice = crate::parsers::official::Notice {
            id: "kpszsu/101".to_string(),
            channel: "kpszsu".to_string(),
            post_url: "https://t.me/kpszsu/101".to_string(),
            text: "БпЛА над Київщиною".to_string(),
            observed_at: chrono::DateTime::from_timestamp(now as i64, 0).unwrap(),
            kind: "uav".to_string(),
            oblast: Some("Київська".to_string()),
            lat: Some(50.3),
            lon: Some(30.5),
        };
        let threat_dimap = sample_cluster("t1", "uav", true, &["dimap"], 50.3, 30.5).members.remove(0);
        engine.record_official_notices(&[notice], &[threat_dimap], now);

        let report_after = engine.calculate_report("1h", "all", now).unwrap();
        assert_eq!(report_after.total_official_notices, 1);
        let dimap = report_after.sources.iter().find(|s| s.source == "dimap").unwrap();
        assert_eq!(dimap.official_matches, 1);
        assert_eq!(dimap.official_total, 1);
        assert_eq!(dimap.official_concordance_pct, 100.0);

        // Перевірка документації методології
        let doc = AccuracyEngine::methodology();
        assert_eq!(doc.metrics.len(), 6);
    }

    #[test]
    fn test_early_warning_amnesty_and_confirmed_immunity() {
        let temp_dir = std::env::temp_dir().join(format!("test_acc_amnesty_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let db_path = temp_dir.join("test.db");
        let engine = AccuracyEngine::new(&db_path).unwrap();

        let now = 100_000.0;
        let empty_alerts = crate::parsers::alerts::Response {
            raions: vec![],
            oblasts: vec![],
        };
        // 1. Консенсусна ціль (confirmed: true, 3 джерела) на спокійній території
        // Повинна мати імунітет: 0 штрафів навіть за повної відсутності тривоги в районі!
        let c_confirmed = sample_cluster("c_conf", "uav", true, &["dimap", "radarua", "neptun"], 48.0, 33.0);
        
        // 2. Одиночна засічка від shahmap на спокійній території
        let c_single = sample_cluster("c_single", "uav", false, &["shahmap"], 47.5, 33.5);

        // Фіксуємо кластери
        let bounds_with_geom = crate::parsers::alerts::Bounds {
            raions: [(
                "бериславський".to_string(),
                serde_json::json!({
                    "type": "Polygon",
                    "coordinates": [[[33.0, 47.0], [34.0, 47.0], [34.0, 48.0], [33.0, 48.0], [33.0, 47.0]]]
                }),
            )].into_iter().collect(),
            oblasts: HashMap::new(),
            loaded_at: 100.0,
        };

        engine.record_clusters(&[c_confirmed, c_single.clone()], &empty_alerts, &bounds_with_geom, now);

        let report1 = engine.calculate_report("1h", "all", now).unwrap();
        let dimap = report1.sources.iter().find(|s| s.source == "dimap").unwrap();
        assert_eq!(dimap.no_alert_violations, 0, "Консенсусна ціль не повинна штрафуватися!");

        let shahmap = report1.sources.iter().find(|s| s.source == "shahmap").unwrap();
        assert_eq!(shahmap.no_alert_violations, 1, "Одиночна ціль отримала попередній штраф");

        // 3. Через 2 хвилини з'являється жовта або червона тривога в районі точки shahmap (47.5, 33.5)
        let alert_appeared = crate::parsers::alerts::Response {
            raions: vec![
                crate::parsers::alerts::AlertEntry {
                    key: Some("бериславський".to_string()),
                    level: Some("yellow".to_string()),
                    ..Default::default()
                }
            ],
            oblasts: vec![],
        };

        // Запускаємо наступний тік через 120 секунд (у межах 10-хвилинного кулдауну)
        engine.record_clusters(&[], &alert_appeared, &bounds_with_geom, now + 120.0);

        let report2 = engine.calculate_report("1h", "all", now + 120.0).unwrap();
        let shahmap2 = report2.sources.iter().find(|s| s.source == "shahmap").unwrap();
        assert_eq!(shahmap2.no_alert_violations, 0, "Штраф повинен бути амністований після появи тривоги!");
        assert_eq!(shahmap2.penalty_points, 0);
    }
}

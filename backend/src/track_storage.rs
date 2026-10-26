//! Модуль реляційного зберігання треків, засічок, сирих спостережень та тривог (SQLite / PostgreSQL).
//!
//! Забезпечує накопичення повноцінного датасету для навчання моделей машинного навчання
//! та нейромереж (прогнозування траєкторій, класифікація цілей, мультисенсорний ф'южн).
//!
//! Таблиці:
//! 1. `tracks` — сесії польоту цілей (ідентифікатор, час, статус, тип, швидкісний профіль, точки входу/виходу).
//! 2. `track_points` — часопросторові точки треку (координати, висота, швидкість, курс, джерела, розкид).
//! 3. `raw_threat_reports` — первинні спостереження від кожного окремого радару/джерела перед кластеризацією.
//! 4. `alert_events` — хронологія оголошення та відбоїв повітряних тривог у районах та областях.

use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};

/// Статус сесії польоту цілі
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackStatus {
    /// Активний борт у повітряному просторі
    Active,
    /// Збитий силами ППО / мобільними вогневими групами
    Downed,
    /// Втрачений під дією засобів радіоелектронної боротьби (РЕБ) / локаційно втрачений
    LostEw,
    /// Вийшов із повітряного простору України (наприклад, у бік Молдови, Білорусі, Румунії чи ТОТ)
    ExitedAirspace,
}

impl TrackStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "активний",
            Self::Downed => "збитий",
            Self::LostEw => "втрачений РЕБ",
            Self::ExitedAirspace => "вийшов із простору",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "активний" | "active" => Some(Self::Active),
            "збитий" | "downed" | "shot_down" => Some(Self::Downed),
            "втрачений реб" | "lost_ew" | "ew" => Some(Self::LostEw),
            "вийшов із простору" | "exited" | "exited_airspace" => Some(Self::ExitedAirspace),
            _ => None,
        }
    }
}

/// Визначення людиночитаного та навчального типу заліза на основі типу цілі та швидкісного профілю
pub fn determine_estimated_type(threat_type: &str, avg_speed: Option<f64>, max_speed: Option<f64>) -> String {
    match threat_type {
        "uav" => {
            if let Some(spd) = max_speed.or(avg_speed) {
                if spd < 130.0 {
                    "БпЛА-імітатор (Гербера / Пародія)".to_string()
                } else {
                    "Shahed-136".to_string()
                }
            } else {
                "Shahed-136".to_string()
            }
        }
        "uav_jet" => "Shahed-238 (реактивний)".to_string(),
        "missile" => "Крилата ракета (Х-101 / Калібр)".to_string(),
        "ballistic" => "Балістична ракета (Іскандер-М / Кинджал)".to_string(),
        "kab" => "КАБ (керована авіабомба)".to_string(),
        "recon" => "Розвідувальний БпЛА (Zala / Supercam / Орлан)".to_string(),
        "fpv" => "FPV-дрон".to_string(),
        "tactical" => "Тактична авіація (Су-34 / Су-35)".to_string(),
        "mig31k" => "МіГ-31К (носій Кинджалу)".to_string(),
        _ => "Повітряна ціль".to_string(),
    }
}

/// Модель сесії польоту цілі (tracks)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    /// Унікальний UUID або внутрішній числовий/текстовий ідентифікатор треку (наприклад, "uav_001")
    pub id: String,
    /// Час першої фіксації цілі (UTC timestamp у секундах)
    pub first_seen: i64,
    /// Час останнього оновлення або зникнення з радарів (UTC timestamp у секундах)
    pub last_seen: i64,
    /// Статус польоту: активний, збитий, втрачений РЕБ, вийшов із простору
    pub status: TrackStatus,
    /// Базовий код типу: uav, uav_jet, missile, ballistic, kab, recon, tactical, fpv
    pub threat_type: String,
    /// Оціночний тип цілі для розмітки датасету
    pub estimated_type: String,
    /// Чи підтверджено консенсусом кількох джерел або офіційним радаром
    pub confirmed: bool,
    /// Перелік усіх джерел, що фіксували цей трек (через кому)
    pub sources: String,
    /// Загальна кількість засічок у навчальній послідовності
    pub points_count: i64,
    /// Загальна пройдена дистанція треку, км
    pub total_dist_km: f64,
    /// Середня швидкість польоту, км/год
    pub avg_speed: Option<f64>,
    /// Максимальна зафіксована швидкість, км/год
    pub max_speed: Option<f64>,
    /// Координати точки першої фіксації (входу) у форматі "lat,lon"
    pub entry_point: Option<String>,
    /// Координати точки останньої фіксації (виходу/збиття) у форматі "lat,lon"
    pub exit_point: Option<String>,
}

/// Модель просторової точки треку (track_points)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackPoint {
    pub id: Option<i64>,
    /// Зовнішній ключ до таблиці tracks
    pub track_id: String,
    /// Точний час засічки (UTC timestamp у секундах)
    pub timestamp: i64,
    /// Географічна широта (latitude, WGS-84)
    pub latitude: f64,
    /// Географічна довгота (longitude, WGS-84)
    pub longitude: f64,
    /// Висота польоту в метрах (якщо є в потоці даних, інакше None)
    pub altitude: Option<f64>,
    /// Розрахункова швидкість на цьому відрізку (км/год)
    pub speed: Option<f64>,
    /// Азимут/курс у градусах (0–360°)
    pub heading: Option<f64>,
    /// Кількість джерел, що підтвердили точку у цьому циклі
    pub source_count: i64,
    /// Список джерел цієї точки
    pub sources: String,
    /// Підтверджено консенсусом
    pub confirmed: bool,
    /// Похибка вимірювання / розкид кластера, км
    pub spread_km: Option<f64>,
    /// Область знаходження цілі
    pub region: Option<String>,
}

/// Сире спостереження від окремого джерела/радару (raw_threat_reports)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawThreatReport {
    pub id: Option<i64>,
    pub observed_at: i64,
    pub source: String,
    pub source_target_id: Option<String>,
    pub threat_type: String,
    pub lat: f64,
    pub lon: f64,
    pub heading: Option<f64>,
    pub region: Option<String>,
    pub district: Option<String>,
    pub locality: Option<String>,
    pub confidence: Option<String>,
    pub target_count: i64,
    pub matched_track_id: Option<String>,
}

/// Подія зміни повітряної тривоги (alert_events)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEventRecord {
    pub id: Option<i64>,
    pub observed_at: i64,
    pub region_id: String,
    pub region_name: String,
    pub region_type: String,
    pub alert_level: String,
    pub is_active: bool,
    pub since: Option<String>,
}

fn ensure_column_exists(conn: &Connection, table: &str, column: &str, col_def: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let mut rows = stmt.query([])?;
    let mut exists = false;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name.eq_ignore_ascii_case(column) {
            exists = true;
            break;
        }
    }
    if !exists {
        conn.execute(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, col_def), [])?;
    }
    Ok(())
}

/// Ініціалізація схеми реляційних таблиць та безпечна міграція
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS tracks (
            id TEXT PRIMARY KEY,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'активний' CHECK (status IN ('активний', 'збитий', 'втрачений РЕБ', 'вийшов із простору')),
            threat_type TEXT NOT NULL DEFAULT 'uav',
            estimated_type TEXT NOT NULL DEFAULT 'Shahed-136',
            confirmed INTEGER NOT NULL DEFAULT 0,
            sources TEXT NOT NULL DEFAULT '',
            points_count INTEGER NOT NULL DEFAULT 0,
            total_dist_km REAL NOT NULL DEFAULT 0.0,
            avg_speed REAL,
            max_speed REAL,
            entry_point TEXT,
            exit_point TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_tracks_first_seen ON tracks(first_seen);
        CREATE INDEX IF NOT EXISTS idx_tracks_last_seen ON tracks(last_seen);
        CREATE INDEX IF NOT EXISTS idx_tracks_status ON tracks(status);

        CREATE TABLE IF NOT EXISTS track_points (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            track_id TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            timestamp INTEGER NOT NULL,
            latitude REAL NOT NULL,
            longitude REAL NOT NULL,
            altitude REAL,
            speed REAL,
            heading REAL CHECK (heading IS NULL OR (heading >= 0.0 AND heading <= 360.0)),
            source_count INTEGER NOT NULL DEFAULT 1,
            sources TEXT NOT NULL DEFAULT '',
            confirmed INTEGER NOT NULL DEFAULT 0,
            spread_km REAL,
            region TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_track_points_track_id ON track_points(track_id);
        CREATE INDEX IF NOT EXISTS idx_track_points_timestamp ON track_points(timestamp);
        CREATE INDEX IF NOT EXISTS idx_track_points_coords ON track_points(latitude, longitude);

        CREATE TABLE IF NOT EXISTS raw_threat_reports (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            observed_at INTEGER NOT NULL,
            source TEXT NOT NULL,
            source_target_id TEXT,
            threat_type TEXT NOT NULL,
            lat REAL NOT NULL,
            lon REAL NOT NULL,
            heading REAL,
            region TEXT,
            district TEXT,
            locality TEXT,
            confidence TEXT,
            target_count INTEGER NOT NULL DEFAULT 1,
            matched_track_id TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_raw_reports_time ON raw_threat_reports(observed_at);
        CREATE INDEX IF NOT EXISTS idx_raw_reports_source ON raw_threat_reports(source);
        CREATE INDEX IF NOT EXISTS idx_raw_reports_track ON raw_threat_reports(matched_track_id);

        CREATE TABLE IF NOT EXISTS alert_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            observed_at INTEGER NOT NULL,
            region_id TEXT NOT NULL,
            region_name TEXT NOT NULL,
            region_type TEXT NOT NULL,
            alert_level TEXT NOT NULL,
            is_active INTEGER NOT NULL,
            since TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_alert_events_time ON alert_events(observed_at);
        CREATE INDEX IF NOT EXISTS idx_alert_events_region ON alert_events(region_id);

        CREATE TABLE IF NOT EXISTS user_consents (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            consent_id TEXT NOT NULL UNIQUE,
            public_key TEXT NOT NULL,
            signature TEXT NOT NULL,
            statement TEXT NOT NULL,
            user_agent TEXT,
            ip_address TEXT,
            accepted_at INTEGER NOT NULL,
            version TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_user_consents_pubkey ON user_consents(public_key);
        CREATE INDEX IF NOT EXISTS idx_user_consents_time ON user_consents(accepted_at);",
    )?;

    // Безпечна міграція існуючих таблиць для підтримки нових атрибутів ML
    ensure_column_exists(conn, "tracks", "threat_type", "TEXT NOT NULL DEFAULT 'uav'")?;
    ensure_column_exists(conn, "tracks", "confirmed", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column_exists(conn, "tracks", "sources", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column_exists(conn, "tracks", "points_count", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column_exists(conn, "tracks", "total_dist_km", "REAL NOT NULL DEFAULT 0.0")?;
    ensure_column_exists(conn, "tracks", "avg_speed", "REAL")?;
    ensure_column_exists(conn, "tracks", "max_speed", "REAL")?;
    ensure_column_exists(conn, "tracks", "entry_point", "TEXT")?;
    ensure_column_exists(conn, "tracks", "exit_point", "TEXT")?;

    ensure_column_exists(conn, "track_points", "source_count", "INTEGER NOT NULL DEFAULT 1")?;
    ensure_column_exists(conn, "track_points", "sources", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column_exists(conn, "track_points", "confirmed", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column_exists(conn, "track_points", "spread_km", "REAL")?;
    ensure_column_exists(conn, "track_points", "region", "TEXT")?;

    conn.execute("CREATE INDEX IF NOT EXISTS idx_tracks_threat_type ON tracks(threat_type);", [])?;

    Ok(())
}

/// Збереження або оновлення сесії польоту (tracks)
pub fn upsert_track(conn: &Connection, track: &Track) -> Result<()> {
    conn.execute(
        "INSERT INTO tracks (
            id, first_seen, last_seen, status, threat_type, estimated_type,
            confirmed, sources, points_count, total_dist_km, avg_speed, max_speed,
            entry_point, exit_point
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT(id) DO UPDATE SET
            last_seen = excluded.last_seen,
            status = excluded.status,
            threat_type = excluded.threat_type,
            estimated_type = excluded.estimated_type,
            confirmed = excluded.confirmed,
            sources = excluded.sources,
            points_count = excluded.points_count,
            total_dist_km = excluded.total_dist_km,
            avg_speed = excluded.avg_speed,
            max_speed = excluded.max_speed,
            exit_point = excluded.exit_point;",
        params![
            track.id,
            track.first_seen,
            track.last_seen,
            track.status.as_str(),
            track.threat_type,
            track.estimated_type,
            if track.confirmed { 1 } else { 0 },
            track.sources,
            track.points_count,
            track.total_dist_km,
            track.avg_speed,
            track.max_speed,
            track.entry_point,
            track.exit_point,
        ],
    )?;
    Ok(())
}

/// Додавання нової просторової точки до треку (track_points)
pub fn insert_track_point(conn: &Connection, point: &TrackPoint) -> Result<i64> {
    conn.execute(
        "INSERT INTO track_points (
            track_id, timestamp, latitude, longitude, altitude, speed, heading,
            source_count, sources, confirmed, spread_km, region
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
        params![
            point.track_id,
            point.timestamp,
            point.latitude,
            point.longitude,
            point.altitude,
            point.speed,
            point.heading,
            point.source_count,
            point.sources,
            if point.confirmed { 1 } else { 0 },
            point.spread_km,
            point.region,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Додавання пакету сирих спостережень окремих джерел
pub fn insert_raw_reports(conn: &Connection, reports: &[RawThreatReport]) -> Result<()> {
    if reports.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "INSERT INTO raw_threat_reports (
            observed_at, source, source_target_id, threat_type,
            lat, lon, heading, region, district, locality,
            confidence, target_count, matched_track_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13);",
    )?;
    for r in reports {
        stmt.execute(params![
            r.observed_at,
            r.source,
            r.source_target_id,
            r.threat_type,
            r.lat,
            r.lon,
            r.heading,
            r.region,
            r.district,
            r.locality,
            r.confidence,
            r.target_count,
            r.matched_track_id,
        ])?;
    }
    Ok(())
}

/// Запис події тривоги (початок / зміна / відбій)
pub fn insert_alert_event(conn: &Connection, event: &AlertEventRecord) -> Result<i64> {
    conn.execute(
        "INSERT INTO alert_events (
            observed_at, region_id, region_name, region_type, alert_level, is_active, since
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
        params![
            event.observed_at,
            event.region_id,
            event.region_name,
            event.region_type,
            event.alert_level,
            if event.is_active { 1 } else { 0 },
            event.since,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Оновлення статусу та часу останнього оновлення треку
pub fn update_track_status(conn: &Connection, track_id: &str, status: TrackStatus, last_seen: i64) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET status = ?1, last_seen = ?2 WHERE id = ?3;",
        params![status.as_str(), last_seen, track_id],
    )?;
    Ok(())
}

/// Отримання сесії треку за його ID
pub fn get_track(conn: &Connection, track_id: &str) -> Result<Option<Track>> {
    let mut stmt = conn.prepare(
        "SELECT id, first_seen, last_seen, status, threat_type, estimated_type,
                confirmed, sources, points_count, total_dist_km, avg_speed, max_speed,
                entry_point, exit_point
         FROM tracks WHERE id = ?1;",
    )?;
    let mut rows = stmt.query(params![track_id])?;

    if let Some(row) = rows.next()? {
        let status_str: String = row.get(3)?;
        let status = TrackStatus::from_str(&status_str).unwrap_or(TrackStatus::Active);
        let confirmed_int: i64 = row.get(6).unwrap_or(0);
        Ok(Some(Track {
            id: row.get(0)?,
            first_seen: row.get(1)?,
            last_seen: row.get(2)?,
            status,
            threat_type: row.get(4).unwrap_or_else(|_| "uav".to_string()),
            estimated_type: row.get(5)?,
            confirmed: confirmed_int != 0,
            sources: row.get(7).unwrap_or_default(),
            points_count: row.get(8).unwrap_or(0),
            total_dist_km: row.get(9).unwrap_or(0.0),
            avg_speed: row.get(10).ok(),
            max_speed: row.get(11).ok(),
            entry_point: row.get(12).ok(),
            exit_point: row.get(13).ok(),
        }))
    } else {
        Ok(None)
    }
}

/// Отримання всіх просторових точок для конкретного треку у хронологічному порядку
pub fn get_track_points(conn: &Connection, track_id: &str) -> Result<Vec<TrackPoint>> {
    let mut stmt = conn.prepare(
        "SELECT id, track_id, timestamp, latitude, longitude, altitude, speed, heading,
                source_count, sources, confirmed, spread_km, region
        FROM track_points
        WHERE track_id = ?1
        ORDER BY timestamp ASC, id ASC;",
    )?;

    let rows = stmt.query_map(params![track_id], |row| {
        let confirmed_int: i64 = row.get(10).unwrap_or(0);
        Ok(TrackPoint {
            id: Some(row.get(0)?),
            track_id: row.get(1)?,
            timestamp: row.get(2)?,
            latitude: row.get(3)?,
            longitude: row.get(4)?,
            altitude: row.get(5)?,
            speed: row.get(6)?,
            heading: row.get(7)?,
            source_count: row.get(8).unwrap_or(1),
            sources: row.get(9).unwrap_or_default(),
            confirmed: confirmed_int != 0,
            spread_km: row.get(11).ok(),
            region: row.get(12).ok(),
        })
    })?;

    let mut points = Vec::new();
    for p in rows {
        points.push(p?);
    }
    Ok(points)
}

/// Експорт датасету траєкторій для тренування нейромереж (PyTorch / TensorFlow / Pandas)
pub fn export_ml_dataset(
    conn: &Connection,
    since_ts: Option<i64>,
    threat_type_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<serde_json::Value> {
    let limit_val = limit.unwrap_or(500).min(5000);
    let since_val = since_ts.unwrap_or(0);

    let (query, has_type) = if let Some(_) = threat_type_filter {
        (
            "SELECT id, first_seen, last_seen, status, threat_type, estimated_type,
                    confirmed, sources, points_count, total_dist_km, avg_speed, max_speed,
                    entry_point, exit_point
             FROM tracks
             WHERE last_seen >= ?1 AND threat_type = ?2 AND points_count >= 2
             ORDER BY first_seen DESC
             LIMIT ?3;",
            true,
        )
    } else {
        (
            "SELECT id, first_seen, last_seen, status, threat_type, estimated_type,
                    confirmed, sources, points_count, total_dist_km, avg_speed, max_speed,
                    entry_point, exit_point
             FROM tracks
             WHERE last_seen >= ?1 AND points_count >= 2
             ORDER BY first_seen DESC
             LIMIT ?2;",
            false,
        )
    };

    let mut stmt = conn.prepare(query)?;
    let track_rows: Vec<Track> = if has_type {
        let tt = threat_type_filter.unwrap();
        let rows = stmt.query_map(params![since_val, tt, limit_val as i64], |row| {
            let status_str: String = row.get(3)?;
            let status = TrackStatus::from_str(&status_str).unwrap_or(TrackStatus::Active);
            let confirmed_int: i64 = row.get(6).unwrap_or(0);
            Ok(Track {
                id: row.get(0)?,
                first_seen: row.get(1)?,
                last_seen: row.get(2)?,
                status,
                threat_type: row.get(4).unwrap_or_else(|_| "uav".to_string()),
                estimated_type: row.get(5)?,
                confirmed: confirmed_int != 0,
                sources: row.get(7).unwrap_or_default(),
                points_count: row.get(8).unwrap_or(0),
                total_dist_km: row.get(9).unwrap_or(0.0),
                avg_speed: row.get(10).ok(),
                max_speed: row.get(11).ok(),
                entry_point: row.get(12).ok(),
                exit_point: row.get(13).ok(),
            })
        })?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        let rows = stmt.query_map(params![since_val, limit_val as i64], |row| {
            let status_str: String = row.get(3)?;
            let status = TrackStatus::from_str(&status_str).unwrap_or(TrackStatus::Active);
            let confirmed_int: i64 = row.get(6).unwrap_or(0);
            Ok(Track {
                id: row.get(0)?,
                first_seen: row.get(1)?,
                last_seen: row.get(2)?,
                status,
                threat_type: row.get(4).unwrap_or_else(|_| "uav".to_string()),
                estimated_type: row.get(5)?,
                confirmed: confirmed_int != 0,
                sources: row.get(7).unwrap_or_default(),
                points_count: row.get(8).unwrap_or(0),
                total_dist_km: row.get(9).unwrap_or(0.0),
                avg_speed: row.get(10).ok(),
                max_speed: row.get(11).ok(),
                entry_point: row.get(12).ok(),
                exit_point: row.get(13).ok(),
            })
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut tracks_json = Vec::new();
    for t in track_rows {
        let points = get_track_points(conn, &t.id)?;
        let points_json: Vec<serde_json::Value> = points
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "t": p.timestamp,
                    "lat": p.latitude,
                    "lon": p.longitude,
                    "alt": p.altitude,
                    "speed": p.speed,
                    "heading": p.heading,
                    "source_count": p.source_count,
                    "sources": p.sources.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>(),
                    "confirmed": p.confirmed,
                    "spread_km": p.spread_km,
                    "region": p.region,
                })
            })
            .collect();

        tracks_json.push(serde_json::json!({
            "id": t.id,
            "threat_type": t.threat_type,
            "estimated_type": t.estimated_type,
            "status": t.status.as_str(),
            "first_seen": t.first_seen,
            "last_seen": t.last_seen,
            "duration_sec": t.last_seen - t.first_seen,
            "points_count": t.points_count,
            "total_dist_km": t.total_dist_km,
            "avg_speed": t.avg_speed,
            "max_speed": t.max_speed,
            "confirmed": t.confirmed,
            "sources": t.sources.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>(),
            "entry_point": t.entry_point,
            "exit_point": t.exit_point,
            "points": points_json,
        }));
    }

    Ok(serde_json::json!({
        "dataset_name": "tacmap_trajectories",
        "format_version": "1.0",
        "description": "Historical flight trajectories for trajectory prediction, threat classification, and neural sensor fusion",
        "tracks_count": tracks_json.len(),
        "tracks": tracks_json,
    }))
}

/// Статистика готовності та об'єму датасету для навчання моделей ШІ
pub fn get_dataset_stats(conn: &Connection) -> Result<serde_json::Value> {
    let total_tracks: i64 = conn.query_row("SELECT count(*) FROM tracks;", [], |r| r.get(0)).unwrap_or(0);
    let total_points: i64 = conn.query_row("SELECT count(*) FROM track_points;", [], |r| r.get(0)).unwrap_or(0);
    let total_raw_reports: i64 = conn.query_row("SELECT count(*) FROM raw_threat_reports;", [], |r| r.get(0)).unwrap_or(0);
    let total_alert_events: i64 = conn.query_row("SELECT count(*) FROM alert_events;", [], |r| r.get(0)).unwrap_or(0);

    let mut stmt = conn.prepare("SELECT threat_type, count(*) FROM tracks GROUP BY threat_type ORDER BY count(*) DESC;")?;
    let type_counts: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            let tt: String = r.get(0)?;
            let cnt: i64 = r.get(1)?;
            Ok(serde_json::json!({ "type": tt, "count": cnt }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut stmt_status = conn.prepare("SELECT status, count(*) FROM tracks GROUP BY status ORDER BY count(*) DESC;")?;
    let status_counts: Vec<serde_json::Value> = stmt_status
        .query_map([], |r| {
            let st: String = r.get(0)?;
            let cnt: i64 = r.get(1)?;
            Ok(serde_json::json!({ "status": st, "count": cnt }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::json!({
        "total_tracks": total_tracks,
        "total_points": total_points,
        "total_raw_reports": total_raw_reports,
        "total_alert_events": total_alert_events,
        "by_threat_type": type_counts,
        "by_status": status_counts,
        "ml_ready": total_tracks > 0 && total_points > 0,
    }))
}

/// Запис криптографічної згоди користувача з умовами використання сервісу
pub fn record_user_consent(
    conn: &Connection,
    consent_id: &str,
    public_key: &str,
    signature: &str,
    statement: &str,
    user_agent: Option<&str>,
    ip_address: Option<&str>,
    accepted_at: i64,
    version: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO user_consents (
            consent_id, public_key, signature, statement, user_agent, ip_address, accepted_at, version
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(consent_id) DO UPDATE SET
            public_key = excluded.public_key,
            signature = excluded.signature,
            statement = excluded.statement,
            user_agent = excluded.user_agent,
            ip_address = excluded.ip_address,
            accepted_at = excluded.accepted_at,
            version = excluded.version",
        params![
            consent_id,
            public_key,
            signature,
            statement,
            user_agent,
            ip_address,
            accepted_at,
            version,
        ],
    )?;
    Ok(())
}

/// Перевірка наявності згоди за її ID
pub fn check_user_consent(conn: &Connection, consent_id: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM user_consents WHERE consent_id = ?1 LIMIT 1")?;
    let exists = stmt.exists(params![consent_id])?;
    Ok(exists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_and_points_lifecycle() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let track = Track {
            id: "uav_001".to_string(),
            first_seen: 1792479500,
            last_seen: 1792479600,
            status: TrackStatus::Active,
            threat_type: "uav".to_string(),
            estimated_type: "Shahed-136 (Герань-2)".to_string(),
            confirmed: true,
            sources: "ukrainealarm,radarua".to_string(),
            points_count: 2,
            total_dist_km: 12.5,
            avg_speed: Some(183.5),
            max_speed: Some(185.0),
            entry_point: Some("47.1234,31.5678".to_string()),
            exit_point: Some("47.2234,31.6678".to_string()),
        };

        upsert_track(&conn, &track).unwrap();

        let point1 = TrackPoint {
            id: None,
            track_id: "uav_001".to_string(),
            timestamp: 1792479500,
            latitude: 47.1234,
            longitude: 31.5678,
            altitude: Some(150.0),
            speed: Some(185.0),
            heading: Some(240.0),
            source_count: 2,
            sources: "ukrainealarm,radarua".to_string(),
            confirmed: true,
            spread_km: Some(1.2),
            region: Some("Миколаївська".to_string()),
        };
        let point2 = TrackPoint {
            id: None,
            track_id: "uav_001".to_string(),
            timestamp: 1792479600,
            latitude: 47.2234,
            longitude: 31.6678,
            altitude: None,
            speed: Some(182.0),
            heading: Some(242.0),
            source_count: 1,
            sources: "ukrainealarm".to_string(),
            confirmed: true,
            spread_km: Some(0.8),
            region: Some("Миколаївська".to_string()),
        };

        insert_track_point(&conn, &point1).unwrap();
        insert_track_point(&conn, &point2).unwrap();

        let loaded_track = get_track(&conn, "uav_001").unwrap().unwrap();
        assert_eq!(loaded_track.id, "uav_001");
        assert_eq!(loaded_track.status, TrackStatus::Active);
        assert_eq!(loaded_track.threat_type, "uav");
        assert_eq!(loaded_track.confirmed, true);

        let points = get_track_points(&conn, "uav_001").unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].altitude, Some(150.0));
        assert_eq!(points[0].sources, "ukrainealarm,radarua");
        assert_eq!(points[1].altitude, None);

        // Перевірка експорту датасету для ML
        let dataset = export_ml_dataset(&conn, None, None, None).unwrap();
        assert_eq!(dataset["tracks_count"], 1);
        let track_json = &dataset["tracks"][0];
        assert_eq!(track_json["id"], "uav_001");
        assert_eq!(track_json["points_count"], 2);

        // Перевірка статистики
        let stats = get_dataset_stats(&conn).unwrap();
        assert_eq!(stats["total_tracks"], 1);
        assert_eq!(stats["total_points"], 2);
        assert_eq!(stats["ml_ready"], true);

        // Перевірка оновлення статусу
        update_track_status(&conn, "uav_001", TrackStatus::Downed, 1792479700).unwrap();
        let updated = get_track(&conn, "uav_001").unwrap().unwrap();
        assert_eq!(updated.status, TrackStatus::Downed);
        assert_eq!(updated.last_seen, 1792479700);

        // Перевірка каскадного видалення
        conn.execute("DELETE FROM tracks WHERE id = 'uav_001';", []).unwrap();
        let remaining_points = get_track_points(&conn, "uav_001").unwrap();
        assert_eq!(remaining_points.len(), 0);
    }

    #[test]
    fn test_raw_reports_and_alert_events() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let raw = vec![
            RawThreatReport {
                id: None,
                observed_at: 1000,
                source: "radarua".to_string(),
                source_target_id: Some("rad_12".to_string()),
                threat_type: "uav".to_string(),
                lat: 48.1,
                lon: 31.2,
                heading: Some(190.0),
                region: Some("Кіровоградська".to_string()),
                district: None,
                locality: None,
                confidence: Some("high".to_string()),
                target_count: 1,
                matched_track_id: Some("uav_001".to_string()),
            }
        ];
        insert_raw_reports(&conn, &raw).unwrap();

        let alert = AlertEventRecord {
            id: None,
            observed_at: 1000,
            region_id: "mykolaiv".to_string(),
            region_name: "Миколаївська область".to_string(),
            region_type: "oblast".to_string(),
            alert_level: "red".to_string(),
            is_active: true,
            since: Some("2026-10-20T08:00:00Z".to_string()),
        };
        insert_alert_event(&conn, &alert).unwrap();

        let stats = get_dataset_stats(&conn).unwrap();
        assert_eq!(stats["total_raw_reports"], 1);
        assert_eq!(stats["total_alert_events"], 1);
    }

    #[test]
    fn test_status_check_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let res = conn.execute(
            "INSERT INTO tracks (id, first_seen, last_seen, status, estimated_type) VALUES ('bad', 1, 2, 'невідомий', 'Shahed-136');",
            [],
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_heading_check_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let track = Track {
            id: "t1".to_string(),
            first_seen: 1,
            last_seen: 2,
            status: TrackStatus::Active,
            threat_type: "uav".to_string(),
            estimated_type: "Shahed-136".to_string(),
            confirmed: false,
            sources: "".to_string(),
            points_count: 0,
            total_dist_km: 0.0,
            avg_speed: None,
            max_speed: None,
            entry_point: None,
            exit_point: None,
        };
        upsert_track(&conn, &track).unwrap();

        let res = conn.execute(
            "INSERT INTO track_points (track_id, timestamp, latitude, longitude, heading) VALUES ('t1', 1, 47.0, 31.0, 400.0);",
            [],
        );
        assert!(res.is_err());

        let res_neg = conn.execute(
            "INSERT INTO track_points (track_id, timestamp, latitude, longitude, heading) VALUES ('t1', 1, 47.0, 31.0, -10.0);",
            [],
        );
        assert!(res_neg.is_err());

        let res_ok = conn.execute(
            "INSERT INTO track_points (track_id, timestamp, latitude, longitude, heading) VALUES ('t1', 1, 47.0, 31.0, 180.0);",
            [],
        );
        assert!(res_ok.is_ok());
    }

    #[test]
    fn test_user_consent_cryptographic_record() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        assert!(!check_user_consent(&conn, "consent_test_123").unwrap());

        record_user_consent(
            &conn,
            "consent_test_123",
            "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEtestpublickeybase64==",
            "MEQCIQDb3sampletestsignaturebase64==",
            "Я беру на себе повну особисту відповідальність за використання сервісу.",
            Some("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)"),
            Some("192.168.1.100"),
            1729422500,
            "1.0",
        ).unwrap();

        assert!(check_user_consent(&conn, "consent_test_123").unwrap());
    }
}

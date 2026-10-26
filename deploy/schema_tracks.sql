-- ==============================================================================
-- TacMap - Схема реляційних таблиць сесій польоту та просторових точок
-- ==============================================================================
-- Таблиця 1: tracks (сесії польоту цілей)
--   id: унікальний UUID або внутрішній числовий/текстовий ідентифікатор треку
--   first_seen: час першої фіксації цілі (UTC timestamp, Unix epoch seconds)
--   last_seen: час останнього оновлення або зникнення з радарів (UTC timestamp)
--   status: 'активний', 'збитий', 'втрачений РЕБ', 'вийшов із простору'
--   estimated_type: тип БПЛА (за замовчуванням 'Shahed-136' або імітатор)
--
-- Таблиця 2: track_points (просторові часові засічки)
--   track_id: зовнішній ключ до tracks(id) з каскадним видаленням (ON DELETE CASCADE)
--   timestamp: точний час засічки (UTC timestamp, Unix epoch seconds)
--   latitude: географічна широта (WGS-84)
--   longitude: географічна довгота (WGS-84)
--   altitude: висота в метрах (якщо є в потоці, інакше NULL)
--   speed: розрахункова швидкість на цьому відрізку (км/год)
--   heading: азимут/курс у градусах (0.0–360.0°, NULL якщо невизначений)
-- ==============================================================================

-- ------------------------------------------------------------------------------
-- 1. Схема для SQLite (вбудована в tacmap / accuracy.db)
-- ------------------------------------------------------------------------------
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS tracks (
    id TEXT PRIMARY KEY,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'активний' CHECK (status IN ('активний', 'збитий', 'втрачений РЕБ', 'вийшов із простору')),
    estimated_type TEXT NOT NULL DEFAULT 'Shahed-136'
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
    heading REAL CHECK (heading IS NULL OR (heading >= 0.0 AND heading <= 360.0))
);

CREATE INDEX IF NOT EXISTS idx_track_points_track_id ON track_points(track_id);
CREATE INDEX IF NOT EXISTS idx_track_points_timestamp ON track_points(timestamp);
CREATE INDEX IF NOT EXISTS idx_track_points_coords ON track_points(latitude, longitude);

-- ------------------------------------------------------------------------------
-- 2. Схема для PostgreSQL / TimescaleDB (для зовнішніх аналітичних сховищ)
-- ------------------------------------------------------------------------------
/*
CREATE TABLE IF NOT EXISTS tracks (
    id VARCHAR(64) PRIMARY KEY,
    first_seen BIGINT NOT NULL,
    last_seen BIGINT NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'активний' CHECK (status IN ('активний', 'збитий', 'втрачений РЕБ', 'вийшов із простору')),
    estimated_type VARCHAR(64) NOT NULL DEFAULT 'Shahed-136'
);

CREATE INDEX IF NOT EXISTS idx_pg_tracks_first_seen ON tracks(first_seen);
CREATE INDEX IF NOT EXISTS idx_pg_tracks_last_seen ON tracks(last_seen);
CREATE INDEX IF NOT EXISTS idx_pg_tracks_status ON tracks(status);

CREATE TABLE IF NOT EXISTS track_points (
    id BIGSERIAL PRIMARY KEY,
    track_id VARCHAR(64) NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    timestamp BIGINT NOT NULL,
    latitude DOUBLE PRECISION NOT NULL,
    longitude DOUBLE PRECISION NOT NULL,
    altitude DOUBLE PRECISION,
    speed DOUBLE PRECISION,
    heading DOUBLE PRECISION CHECK (heading IS NULL OR (heading >= 0.0 AND heading <= 360.0))
);

CREATE INDEX IF NOT EXISTS idx_pg_track_points_track_id ON track_points(track_id);
CREATE INDEX IF NOT EXISTS idx_pg_track_points_timestamp ON track_points(timestamp);
CREATE INDEX IF NOT EXISTS idx_pg_track_points_coords ON track_points(latitude, longitude);
*/

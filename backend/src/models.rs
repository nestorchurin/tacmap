use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Внутрішня модель треку. Зовні (карта/API) позиції бортів НЕ віддаються:
// проєкт показує тільки пуски. Поля лишилися для трекера й детектора.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Threat {
    pub id: String,
    pub source: String, // "neptun" | "dimap"
    #[serde(rename = "type")]
    pub threat_type: String,
    pub title: String,
    pub lat: f64,
    pub lon: f64,
    pub heading: Option<f64>,
    pub region: String,
    pub district: String,
    pub locality: String,
    pub confidence: String,
    pub source_count: i32,
    #[serde(default = "default_target_count")]
    pub target_count: i32,
    pub updated_at: Option<DateTime<Utc>>,
    /// true = точки немає, координати = центроїд області
    pub area_only: bool,
    pub advisory: bool,
    /// Історія треку [lat, lon], старі → нові, без поточної точки
    #[serde(default)]
    pub trail: Vec<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// Театр дій: Україна + зони пусків. Поза ним — географічне сміття.
// ---------------------------------------------------------------------------

pub const THEATER_LAT_MIN: f64 = 42.0;
pub const THEATER_LAT_MAX: f64 = 56.0;
pub const THEATER_LON_MIN: f64 = 20.0;
pub const THEATER_LON_MAX: f64 = 46.0;

pub fn in_theater(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (THEATER_LAT_MIN..=THEATER_LAT_MAX).contains(&lat)
        && (THEATER_LON_MIN..=THEATER_LON_MAX).contains(&lon)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(rename = "type")]
    pub threat_type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub locality: String,
    #[serde(default)]
    pub heading: Option<f64>,
    #[serde(default)]
    pub trail: Vec<[f64; 2]>,
    pub members: Vec<Threat>,
    pub sources: Vec<String>,
    pub agreement: String,
    #[serde(default)]
    pub spread_km: f64,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default = "default_target_count")]
    pub target_count: i32,
}

pub fn default_target_count() -> i32 {
    1
}

/// Витягує кількість цілей у засічці/групі з явного значення або аналізу заголовку.
pub fn extract_target_count(title: &str, explicit: Option<i32>) -> i32 {
    if let Some(c) = explicit {
        if c >= 1 {
            return c;
        }
    }
    let low = title.to_lowercase();
    if let Some(idx) = low.find("група") {
        let rest = &low[idx + "група".len()..];
        for word in rest.split_whitespace() {
            let cleaned = word.trim_matches(|c: char| !c.is_numeric());
            if let Ok(n) = cleaned.parse::<i32>() {
                if (2..=50).contains(&n) {
                    return n;
                }
            }
        }
    }
    if let Some(idx) = low.find("шт") {
        let before = &low[..idx];
        if let Some(last_word) = before.split_whitespace().last() {
            let cleaned = last_word.trim_matches(|c: char| !c.is_numeric());
            if let Ok(n) = cleaned.parse::<i32>() {
                if (2..=50).contains(&n) {
                    return n;
                }
            }
        }
    }
    for part in low.split(|c: char| c == ' ' || c == '(' || c == ')' || c == '[' || c == ']' || c == ',' || c == '/' || c == ':') {
        let p = part.trim();
        if let Some(num_str) = p.strip_suffix('х').or_else(|| p.strip_suffix('x')) {
            if let Ok(n) = num_str.parse::<i32>() {
                if (2..=50).contains(&n) {
                    return n;
                }
            }
        }
        if let Some(num_str) = p.strip_prefix('×').or_else(|| p.strip_prefix('x')) {
            if let Ok(n) = num_str.parse::<i32>() {
                if (2..=50).contains(&n) {
                    return n;
                }
            }
        }
        if let Some(num_str) = p.strip_suffix("шт") {
            if let Ok(n) = num_str.parse::<i32>() {
                if (2..=50).contains(&n) {
                    return n;
                }
            }
        }
    }
    1
}

// ---------------------------------------------------------------------------
// Українські назви типів для показу (сирі коди лишаються в title).
// ---------------------------------------------------------------------------

pub fn ua_label(threat_type: &str, raw_title: &str) -> String {
    if threat_type == "uav_jet" || raw_title == "REACTIVE_SHAHED" {
        return "Реактивний шахед".to_string();
    }
    let low = raw_title.to_lowercase();
    if low.contains("реактив") || low.contains("reactive") || low.contains("jet") {
        return "Реактивний шахед".to_string();
    }
    if low.contains("banderol") || low.contains("бандероль") {
        return "Шахед".to_string();
    }
    if raw_title.starts_with("GERAN") {
        return "Герань".to_string();
    }
    if raw_title == "UNKNOWN_UAV" {
        return "Невідомий БПЛА".to_string();
    }
    if low.contains("decoy") || low.contains("імітатор") || low.contains("пародія") {
        return "Дрон-імітатор".to_string();
    }
    if low.contains("kalibr") || low.contains("калібр") {
        return "Калібр".to_string();
    }
    if low.contains("x101") || low.contains("х-101") {
        return "Х-101".to_string();
    }
    if low.contains("x59") || low.contains("х-59") {
        return "Х-59".to_string();
    }
    if low.contains("kinzhal") || low.contains("кинджал") {
        return "Х-47М2 Кинджал".to_string();
    }
    if low.contains("iskander-m") || low.contains("іскандер-м") {
        return "Іскандер-М".to_string();
    }
    if low.contains("iskander-k") || low.contains("іскандер-к") {
        return "Іскандер-К".to_string();
    }
    if low.contains("iskander") || low.contains("іскандер") {
        return "Іскандер".to_string();
    }
    if low.contains("orlan") || low.contains("орлан") {
        return "Орлан-10".to_string();
    }
    if low.contains("zala") || low.contains("зала") {
        return "ZALA 421".to_string();
    }
    if low.contains("supercam") || low.contains("суперкам") {
        return "Supercam S350".to_string();
    }
    if low.contains("gerbera") || low.contains("гербера") {
        return "Гербера".to_string();
    }
    if low.contains("lancet") || low.contains("ланцет") {
        return "Ланцет".to_string();
    }
    if low.contains("fpv") {
        return "FPV-дрон".to_string();
    }
    if low.contains("su-34") || low.contains("су-34") {
        return "Су-34".to_string();
    }
    if low.contains("su-35") || low.contains("су-35") {
        return "Су-35".to_string();
    }
    if low.contains("su-30") || low.contains("су-30") {
        return "Су-30".to_string();
    }
    if low.contains("su-25") || low.contains("су-25") {
        return "Су-25".to_string();
    }
    if low.contains("su-24") || low.contains("су-24") {
        return "Су-24".to_string();
    }
    // Власна українська назва джерела (напр. "Тактична авіація" без маппинга):
    // довіряємо їй більше ніж заглушці "Невідома ціль"
    if raw_title.chars().any(|c| ('А'..='я').contains(&c) || c == 'Є' || c == 'І' || c == 'Ї' || c == 'Ґ') {
        return raw_title.to_string();
    }
    match threat_type {
        "uav" | "banderol" => "Шахед / БПЛА".to_string(),
        "uav_jet" => "Реактивний шахед".to_string(),
        "tactical" => "Тактична авіація".to_string(),
        "fpv" => "FPV-дрон".to_string(),
        "recon" => "БпЛА-розвідник".to_string(),
        "missile" => "Крилата ракета".to_string(),
        "ballistic" => "Балістична ракета".to_string(),
        "kab" => "КАБ (авіабомба)".to_string(),
        "mig31k" => "МіГ-31К (Кинджал)".to_string(),
        _ => "Невідома ціль".to_string(),
    }
}

// ---------------------------------------------------------------------------
// API-відповіді
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SourceInfo {
    pub name: String,
    pub color: String,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub delay_sec: u64,
    pub buffer_len: usize,
    pub last_fetch_ok: bool,
    pub last_error: Option<String>,
    pub filtered_out: usize,
    pub deduped: usize,
    pub tracks: usize,
    pub anomalies: u64,
    #[serde(default)]
    pub active_launches: usize,
    #[serde(default)]
    pub official_notices: usize,
    #[serde(default)]
    pub online_users: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theater_bounds_sane() {
        assert!(in_theater(50.45, 30.52), "Київ у театрі");
        assert!(in_theater(51.75, 36.29), "Курськ у театрі (зона пусків)");
        assert!(!in_theater(52.52, 13.40), "Берлін поза театром");
        assert!(!in_theater(68.15, 33.46), "Оленья поза театром");
        assert!(!in_theater(f64::NAN, 30.0));
    }

    #[test]
    fn labels_are_ukrainian() {
        assert_eq!(ua_label("uav", "SHAHED"), "Шахед / БПЛА");
        assert_eq!(ua_label("uav", "REACTIVE_SHAHED"), "Реактивний шахед");
        assert_eq!(ua_label("uav", "GERAN3"), "Герань");
        assert_eq!(ua_label("uav", "UNKNOWN_UAV"), "Невідомий БПЛА");
        assert_eq!(ua_label("uav", "Ціль [banderol]"), "Шахед");
        assert_eq!(ua_label("missile", "Ціль [kalibr]"), "Калібр");
        assert_eq!(ua_label("unknown", "Тактична авіація"), "Тактична авіація");
        assert_eq!(ua_label("tactical", "TACTICAL"), "Тактична авіація");
        assert_eq!(ua_label("tactical", "SU-34"), "Су-34");
        assert_eq!(ua_label("unknown", "unknown"), "Невідома ціль");
        assert_eq!(ua_label("fpv", "FPV"), "FPV-дрон");
        assert_eq!(ua_label("weird", "x"), "Невідома ціль");
    }

    #[test]
    fn parses_target_count() {
        assert_eq!(extract_target_count("Шахед (група ×3)", None), 3);
        assert_eq!(extract_target_count("БпЛА 2х", None), 2);
        assert_eq!(extract_target_count("Шахед (2 шт)", None), 2);
        assert_eq!(extract_target_count("БпЛА", Some(4)), 4);
        assert_eq!(extract_target_count("БпЛА", Some(1)), 1);
        assert_eq!(extract_target_count("Одиночний Шахед", None), 1);
    }
}

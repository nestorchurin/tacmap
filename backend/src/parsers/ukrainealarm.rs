//! Парсер офіційного джерела «Повітряна тривога» (map.ukrainealarm.com)
//! Експериментальний сокетний трекінг повітряних цілей вимкнено та деактивовано.
//! Проєкт TACMAP відображає лише загальну ситуаційну інформацію та офіційні пуски/тривоги.

#![allow(dead_code)]

use crate::models::Threat;

const VIEWBOX_WIDTH: f64 = 260.0;
const VIEWBOX_HEIGHT: f64 = 175.0;
const WEST: f64 = 22.137059;
const EAST: f64 = 40.2275801;
const SOUTH: f64 = 44.184598;
const NORTH: f64 = 52.3791473;

fn to_radians(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

fn to_degrees(rad: f64) -> f64 {
    rad * 180.0 / std::f64::consts::PI
}

fn mercator_y(lat: f64) -> f64 {
    (std::f64::consts::PI / 4.0 + to_radians(lat) / 2.0).tan().ln()
}

pub fn normalized_to_lng_lat(x: f64, y: f64, offset_x: Option<f64>, offset_y: Option<f64>) -> (f64, f64) {
    let px = x + offset_x.unwrap_or(0.0);
    let py = y + offset_y.unwrap_or(0.0);
    let projected_width = to_radians(EAST - WEST);
    let projected_height = mercator_y(NORTH) - mercator_y(SOUTH);
    let projection_scale = (VIEWBOX_WIDTH / projected_width).min(VIEWBOX_HEIGHT / projected_height);
    let projection_offset_x = (VIEWBOX_WIDTH - projected_width * projection_scale) / 2.0;
    let projection_offset_y = (VIEWBOX_HEIGHT - projected_height * projection_scale) / 2.0;

    let map_x = px * VIEWBOX_WIDTH;
    let map_y = py * VIEWBOX_HEIGHT;
    let lng = WEST + to_degrees((map_x - projection_offset_x) / projection_scale);
    let projected_lat = mercator_y(NORTH) - (map_y - projection_offset_y) / projection_scale;
    let lat = to_degrees(2.0 * projected_lat.exp().atan() - std::f64::consts::PI / 2.0);
    (lng, lat)
}

#[allow(dead_code)]
pub fn map_type(detected: Option<&str>, formular: Option<&str>, icon: Option<&str>) -> String {
    let s = format!(
        "{} {} {}",
        detected.unwrap_or(""),
        formular.unwrap_or(""),
        icon.unwrap_or("")
    ).to_lowercase();

    if s.contains("shahedjet") || s.contains("реактив") || s.contains("jet") {
        return "uav_jet".to_string();
    }
    if s.contains("shahed") || s.contains("шахед") || s.contains("геран") || s.contains("banderol") {
        return "uav".to_string();
    }
    if s.contains("ballistic") || s.contains("баліст") || s.contains("іскандер") || s.contains("кинжал") {
        return "ballistic".to_string();
    }
    if s.contains("missile") || s.contains("ракета") || s.contains("кр") || s.contains("калібр") || s.contains("х-101") {
        return "missile".to_string();
    }
    if s.contains("airbomb") || s.contains("каб") || s.contains("фаб") {
        return "kab".to_string();
    }
    if s.contains("fpv") || s.contains("recon") || s.contains("розвід") || s.contains("orlan") || s.contains("zala") || s.contains("drone") {
        return "recon_fpv".to_string();
    }
    if s.contains("plane") || s.contains("авіа") || s.contains("су-") || s.contains("міг") || s.contains("тактич") {
        return "tactical".to_string();
    }
    "uav".to_string()
}

pub fn decode_track_id(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("~~") {
        return stripped.to_string();
    }
    if !value.starts_with('~') {
        return value.to_string();
    }
    let b64_str = &value[1..];
    let mut s = b64_str.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    use base64::Engine;
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(s) {
        if bytes.len() >= 16 {
            let idx_map = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
            let mut hex_id = String::with_capacity(32);
            for &idx in &idx_map {
                use std::fmt::Write;
                let _ = write!(&mut hex_id, "{:02x}", bytes[idx]);
            }
            return hex_id;
        }
    }
    value.to_string()
}

/// Виклик fetch вимкнено для запобігання несанкціонованому відстеженню радарних координат
pub async fn fetch(_base_url: &str, _enabled: bool) -> Result<Vec<Threat>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinate_conversion() {
        let (lon, lat) = normalized_to_lng_lat(0.551101, 0.11136, None, None);
        assert!((lon - 32.12).abs() < 0.5, "Expected longitude around 32.12, got {lon}");
        assert!((lat - 51.53).abs() < 0.5, "Expected latitude around 51.53, got {lat}");
    }

    #[test]
    fn test_track_id_decoding() {
        let raw = "~yyFwCfqhck20I1ECwSpg_w";
        let decoded = decode_track_id(raw);
        assert_eq!(decoded, "097021cba1fa4d72b4235102c12a60ff");
    }
}

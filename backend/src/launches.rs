//! Точки пусків: статичний довідковий шар (не треки з джерел).
//! Відомі OSINT-райони пусків БПЛА / КР / балістики / Кинджалів / Калібрів.
//! Координати — орієнтовні (аеродром / полігон / акваторія), точність ~5-10 км.
//! Шар НЕ проходить трекер / delay-буфер: це інфраструктура, а не live-цілі.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSite {
    pub id: String,
    pub name: String,
    /// uav | missile | mig31k | ballistic | naval
    pub kind: String,
    pub lat: f64,
    pub lon: f64,
    pub note: String,
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "uav" => "Пуск БПЛА / Шахед",
        "missile" => "Стратег. авіація / КР",
        "mig31k" => "МіГ-31К / Кинджал",
        "ballistic" => "Балістика / Іскандер",
        "naval" => "Флот / Калібр",
        _ => "Пуск",
    }
}

pub fn kind_color(_kind: &str) -> &'static str {
    "#64748b"
}

/// Статичний список. Джерела координат: відкриті дані аеродромів / OSINT-огляди
/// пусків 2022-2026. Тримати коротко (~22 точки): тільки те, звідки реально летіло.
pub fn all_sites() -> Vec<LaunchSite> {
    let raw: &[(&str, &str, &str, f64, f64, &str)] = &[
        // --- БПЛА / Шахеди ---
        ("primorsko-akhtarsk", "Приморсько-Ахтарськ", "uav", 46.059, 38.172, "Аеродром, осн. район пусків Шахедів з півдня"),
        ("yeysk", "Єйськ", "uav", 46.682, 38.205, "Аеродром, пуски Шахедів / такт. авіація"),
        ("kursk-khalino", "Курськ (Халіно)", "uav", 51.751, 36.293, "Аеродром, пуски Шахедів з півночі"),
        ("bryansk-seshcha", "Брянськ (Сеща)", "uav", 53.715, 33.338, "Аеродром Сеща, пуски БПЛА"),
        ("oryol", "Орел", "uav", 52.970, 36.064, "Район пусків Шахедів"),
        ("shatalovo", "Шаталово", "uav", 54.344, 32.472, "Аеродром, пуски БПЛА"),
        ("millerovo", "Міллерово", "uav", 48.951, 40.297, "Аеродром, пуски БПЛА / такт. авіація"),
        ("chauda", "Мис Чауда (Крим)", "uav", 45.012, 35.835, "Полігон Чауда, пуски Шахедів"),
        ("hvardijske", "Гвардійське (Крим)", "uav", 45.115, 33.979, "Аеродром, пуски БПЛА"),
        // --- Крилаті ракети / стратег. авіація ---
        ("engels-2", "Енгельс-2", "missile", 51.481, 46.211, "Ту-95МС / Ту-160, пуски Х-101/Х-555"),
        ("shaykovka", "Шайковка", "missile", 54.227, 34.591, "Ту-22М3, пуски Х-22/Х-32"),
        ("dyagilevo", "Дягілєво (Рязань)", "missile", 54.641, 39.575, "Ту-95МС / Ту-22М3, пуски КР"),
        ("olenya", "Оленья", "missile", 68.151, 33.467, "Ту-95МС, пуски Х-101 (далекий рубіж)"),
        // --- МіГ-31К / Кинджал ---
        ("savasleyka", "Саваслейка", "mig31k", 55.322, 42.312, "Осн. база МіГ-31К, пуски Кинджалів"),
        ("mozdok", "Моздок", "mig31k", 43.788, 44.606, "Аеродром, МіГ-31К / Ту"),
        // --- Балістика ---
        ("dzhankoy", "Джанкой (Крим)", "ballistic", 45.708, 34.392, "Район пусків балістики / Іскандер"),
        ("belgorod", "Бєлгород", "ballistic", 50.595, 36.587, "Район пусків балістики / Іскандер / С-300"),
        ("kursk", "Курськ", "ballistic", 51.730, 36.193, "Район пусків балістики / Іскандер"),
        ("voronezh", "Воронеж", "ballistic", 51.672, 39.184, "Район пусків балістики / Іскандер"),
        ("taganrog", "Таганрог / Ростов", "ballistic", 47.236, 38.896, "Район пусків балістики / Іскандер"),
        // --- Флот / Калібри ---
        ("novorossiysk", "Новоросійськ", "naval", 44.718, 37.769, "ЧФ, носії Калібрів"),
        ("sevastopol", "Севастополь", "naval", 44.611, 33.493, "ЧФ, носії Калібрів"),
        ("caspian", "Каспій (акваторія)", "naval", 42.600, 50.800, "Каспійська флотилія / пуски КР"),
    ];
    raw.iter()
        .map(|(id, name, kind, lat, lon, note)| LaunchSite {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            lat: *lat,
            lon: *lon,
            note: note.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_core_launch_kinds() {
        let sites = all_sites();
        assert!(sites.len() >= 15, "має бути ядро точок, є {}", sites.len());
        for k in ["uav", "missile", "mig31k", "ballistic", "naval"] {
            assert!(sites.iter().any(|s| s.kind == k), "немає виду {k}");
        }
    }

    #[test]
    fn coords_are_finite_and_ordered() {
        for s in all_sites() {
            assert!(s.lat.is_finite() && s.lon.is_finite(), "{}", s.id);
            assert!((30.0..=75.0).contains(&s.lat), "lat {}", s.id);
            assert!((20.0..=60.0).contains(&s.lon), "lon {}", s.id);
        }
    }

    #[test]
    fn kind_meta_covered() {
        for s in all_sites() {
            assert_ne!(kind_label(&s.kind), "Пуск", "невідомий kind {}", s.kind);
        }
    }
}

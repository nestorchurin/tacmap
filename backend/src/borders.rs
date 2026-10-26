//! Кордон України (спрощений полігон, WGS84) + геоматематика для ліній підльоту.
//! Полігон — міжнародно визнаний кордон включно з Кримом, точність ~10-20 км:
//! достатньо для «часу до кордону», НЕ для юридичних меж.
//! Єдине місце геоматематики: dest_point / bearing / haversine живуть тут.

/// Спрощений кордон України за годинниковою стрілкою, [lat, lon].
/// ~40 вершин: північ (Білорусь) → схід (РФ) → південь (Азов/Чорне/Крим) →
/// південний захід → Молдова → захід (Румунія/Угорщина/Словаччина/Польща).
pub const UA_BORDER: &[[f64; 2]] = &[
    [51.62, 23.65],
    [51.90, 25.00],
    [51.80, 26.80],
    [51.55, 28.30],
    [51.30, 30.20],
    [52.11, 31.79],
    [51.95, 33.60],
    [51.20, 34.40],
    [50.35, 35.75],
    [49.80, 37.50],
    [49.00, 38.90],
    [48.40, 39.80],
    [47.90, 39.30],
    [47.30, 38.70],
    [46.95, 37.60],
    [46.55, 36.85],
    [45.90, 36.70],
    [45.30, 36.45],
    [44.98, 36.00],
    [44.40, 34.20],
    [44.35, 33.00],
    [45.30, 32.50],
    [46.10, 31.95],
    [46.55, 31.30],
    [46.40, 30.75],
    [46.48, 29.90],
    [46.00, 29.60],
    [45.45, 28.95],
    [45.75, 28.20],
    [46.50, 29.00],
    [47.30, 29.40],
    [48.00, 28.60],
    [48.55, 27.80],
    [48.60, 26.90],
    [48.30, 26.00],
    [47.90, 25.30],
    [48.10, 24.30],
    [48.43, 22.20],
    [48.62, 22.15],
    [49.00, 22.85],
    [49.80, 23.10],
    [50.40, 23.90],
    [51.00, 23.90],
];

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

/// Точка призначення: рух dist_km за курсом bearing_deg. Повертає (lat, lon).
pub fn dest_point_ll(lat: f64, lon: f64, bearing_deg: f64, dist_km: f64) -> [f64; 2] {
    let r = 6371.0;
    let br = bearing_deg.to_radians();
    let la1 = lat.to_radians();
    let lo1 = lon.to_radians();
    let d = dist_km / r;
    let la2 = (la1.sin() * d.cos() + la1.cos() * d.sin() * br.cos()).asin();
    let lo2 = lo1 + (br.sin() * d.sin() * la1.cos()).atan2(d.cos() - la1.sin() * la2.sin());
    [la2.to_degrees(), lo2.to_degrees()]
}

/// Початковий курс від (lat1,lon1) до (lat2,lon2), градуси 0..360.
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let la1 = lat1.to_radians();
    let la2 = lat2.to_radians();
    let dlo = (lon2 - lon1).to_radians();
    let y = dlo.sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * dlo.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Чи всередині полігона України (ray casting, x=lon, y=lat).
pub fn inside_ukraine(lat: f64, lon: f64) -> bool {
    let n = UA_BORDER.len();
    let mut inside = false;
    let mut j = n - 1;
    for (i, p) in UA_BORDER.iter().enumerate() {
        let yi = p[0];
        let xi = p[1];
        let yj = UA_BORDER[j][0];
        let xj = UA_BORDER[j][1];
        if (yi > lat) != (yj > lat) {
            let xhit = (xj - xi) * (lat - yi) / (yj - yi) + xi;
            if lon < xhit {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Найближча точка кордону: (lat, lon, dist_km). Дискретизація ребер.
pub fn nearest_border_point(lat: f64, lon: f64) -> [f64; 3] {
    let n = UA_BORDER.len();
    let mut best = [UA_BORDER[0][0], UA_BORDER[0][1], f64::MAX];
    for i in 0..n {
        let a = UA_BORDER[i];
        let b = UA_BORDER[(i + 1) % n];
        // 12 підточок на ребро — достатньо для точності ~5 км
        for k in 0..=12 {
            let t = k as f64 / 12.0;
            let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let d = haversine_km(lat, lon, p[0], p[1]);
            if d < best[2] {
                best = [p[0], p[1], d];
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kyiv_inside_moscow_outside() {
        assert!(inside_ukraine(50.45, 30.52), "Київ всередині");
        assert!(!inside_ukraine(55.75, 37.61), "Москва зовні");
        assert!(!inside_ukraine(52.23, 21.01), "Варшава зовні");
        assert!(!inside_ukraine(46.06, 38.17), "Приморсько-Ахтарськ зовні");
        // міста України — всі всередині
        for (name, lat, lon) in [
            ("Львів", 49.84, 24.02),
            ("Одеса", 46.48, 30.72),
            ("Харків", 49.99, 36.23),
            ("Дніпро", 48.47, 35.04),
            ("Запоріжжя", 47.83, 35.17),
            ("Херсон", 46.63, 32.61),
            ("Миколаїв", 46.97, 31.99),
            ("Суми", 50.91, 34.80),
            ("Чернігів", 51.50, 31.30),
            ("Чернівці", 48.29, 25.93),
            ("Ужгород", 48.62, 22.30),
            ("Чауда", 45.01, 35.83),
            ("Джанкой", 45.71, 34.39),
            ("Севастополь", 44.61, 33.49),
        ] {
            assert!(inside_ukraine(lat, lon), "{name} всередині");
        }
    }

    #[test]
    fn nearest_border_to_kyiv_reasonable() {
        let n = nearest_border_point(50.45, 30.52);
        // до білоруського кордону ~100-150 км
        assert!(n[2] > 40.0 && n[2] < 350.0, "dist {}", n[2]);
    }

    #[test]
    fn dest_and_bearing_roundtrip() {
        // на широті 50°: 1° lon ≈ 71.5 км, тож 111.19 км ≈ +1.556°
        let p = dest_point_ll(50.0, 31.0, 90.0, 111.19);
        assert!((p[1] - 32.556).abs() < 0.05, "{p:?}");
        let b = bearing_deg(50.0, 31.0, 50.0, 32.0);
        assert!((b - 90.0).abs() < 2.0, "{b}");
    }
}

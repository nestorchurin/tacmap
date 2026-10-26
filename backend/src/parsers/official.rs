//! Офіційні сповіщення про пуски: публічні превʼю Telegram-каналів (t.me/s/...).
//! Без ключів і авторизації: t.me/s віддається незалогіненим.
//! За замовчуванням — `kpszsu` (Повітряні Сили ЗСУ): пости про зліт МіГ-31К,
//! пуски КАБ/КР/балістики, рух ударних БпЛА з напрямками.
//! Евристичний монітор лишається фолбеком, якщо превʼю недоступне.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::client;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notice {
    pub id: String,
    pub channel: String,
    #[serde(default)]
    pub post_url: String,
    pub text: String,
    pub observed_at: DateTime<Utc>,
    /// mig31k | ballistic | kab | missile | uav | unknown
    pub kind: String,
    pub oblast: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "mig31k" => "МіГ-31К (офіційно)",
        "tactical" => "Тактична авіація (офіційно)",
        "ballistic" => "Балістика (офіційно)",
        "kab" => "КАБи (офіційно)",
        "missile" => "Ракети (офіційно)",
        "uav" => "БПЛА (офіційно)",
        _ => "Пуск (офіційно)",
    }
}

pub fn kind_color(kind: &str) -> &'static str {
    match kind {
        "uav" => "#ff7a00",
        "missile" | "mig31k" => "#ff3b30",
        "tactical" => "#818cf8",
        "ballistic" => "#e879f9",
        "kab" => "#f5a623",
        _ => "#d6c48a",
    }
}

// ---------------------------------------------------------------------------
// Області: стем → (назва, центр). Центри орієнтовні, для маркера «десь тут».
// ---------------------------------------------------------------------------

const OBLASTS: &[(&str, &str, f64, f64)] = &[
    ("вінницьк", "Вінницька", 49.23, 28.47),
    ("вінниччин", "Вінницька", 49.23, 28.47),
    ("волинськ", "Волинська", 51.00, 24.70),
    ("дніпропетровськ", "Дніпропетровська", 48.45, 34.90),
    ("донецьк", "Донецька", 48.30, 37.50),
    ("донеччин", "Донецька", 48.30, 37.50),
    ("житомирськ", "Житомирська", 50.25, 28.65),
    ("житомирщин", "Житомирська", 50.25, 28.65),
    ("закарпат", "Закарпатська", 48.60, 23.30),
    ("запорізьк", "Запорізька", 47.40, 35.30),
    ("запоріжж", "Запорізька", 47.40, 35.30),
    ("франківськ", "Івано-Франківська", 48.90, 24.70),
    ("київськ", "Київська", 50.30, 30.50),
    ("київ", "Київ", 50.45, 30.52),
    ("столиц", "Київ", 50.45, 30.52),
    ("кіровоградськ", "Кіровоградська", 48.50, 32.25),
    ("кіровоградщин", "Кіровоградська", 48.50, 32.25),
    ("луганськ", "Луганська", 49.00, 38.50),
    ("луганщин", "Луганська", 49.00, 38.50),
    ("львівськ", "Львівська", 49.80, 24.00),
    ("миколаївськ", "Миколаївська", 47.40, 32.00),
    ("миколаївщин", "Миколаївська", 47.40, 32.00),
    ("одеськ", "Одеська", 46.60, 30.00),
    ("одещин", "Одеська", 46.60, 30.00),
    ("полтавськ", "Полтавська", 49.60, 34.50),
    ("полтавщин", "Полтавська", 49.60, 34.50),
    ("рівненськ", "Рівненська", 50.60, 26.30),
    ("рівненщин", "Рівненська", 50.60, 26.30),
    ("сумськ", "Сумська", 51.00, 34.00),
    ("сумщин", "Сумська", 51.00, 34.00),
    ("тернопільськ", "Тернопільська", 49.55, 25.60),
    ("тернопільщин", "Тернопільська", 49.55, 25.60),
    ("харківськ", "Харківська", 49.70, 36.60),
    ("харківщин", "Харківська", 49.70, 36.60),
    ("херсонськ", "Херсонська", 46.50, 33.30),
    ("херсонщин", "Херсонська", 46.50, 33.30),
    ("хмельницьк", "Хмельницька", 49.40, 26.90),
    ("хмельниччин", "Хмельницька", 49.40, 26.90),
    ("черкаськ", "Черкаська", 49.10, 32.00),
    ("черкащин", "Черкаська", 49.10, 32.00),
    ("чернігівськ", "Чернігівська", 51.30, 32.00),
    ("чернігівщин", "Чернігівська", 51.30, 32.00),
    ("чернівецьк", "Чернівецька", 48.40, 25.90),
    ("крим", "Крим", 45.30, 34.30),
    // Ключові міста та райцентри
    ("васильків", "Київська", 50.18, 30.32),
    ("біла церква", "Київська", 49.80, 30.12),
    ("бровар", "Київська", 50.51, 30.79),
    ("бориспіль", "Київська", 50.35, 30.95),
    ("житомир", "Житомирська", 50.25, 28.65),
    ("суми", "Сумська", 50.91, 34.80),
    ("кривий ріг", "Дніпропетровська", 47.91, 33.39),
    ("дніпр", "Дніпропетровська", 48.45, 34.90),
    ("кременчук", "Полтавська", 49.07, 33.42),
    ("уман", "Черкаська", 48.75, 30.22),
    ("луцьк", "Волинська", 50.75, 25.34),
    ("рівне", "Рівненська", 50.62, 26.25),
    ("чернівц", "Чернівецька", 48.29, 25.93),
    ("ужгород", "Закарпатська", 48.62, 22.30),
];

pub fn match_oblast(lower_text: &str) -> Option<(String, f64, f64)> {
    for (stem, name, lat, lon) in OBLASTS {
        if lower_text.contains(stem) {
            return Some((name.to_string(), *lat, *lon));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Класифікація: тільки пускові/рухові пости, «ракетна небезпека» без руху — ні.
// ---------------------------------------------------------------------------

fn has_any(hay: &str, words: &[&str]) -> bool {
    words.iter().any(|w| hay.contains(w))
}

const ALL_CLEAR: &[&str] = &["відбій", "отбой", "загроза минула", "небезпека минула"];
const MIG: &[&str] = &["міг-31", "міг 31", "міг31", "миг-31", "миг31", "кинджал", "кинжал"];
const TACTICAL: &[&str] = &[
    "тактичн",
    "тактичної",
    "тактической",
    "винищувач",
    "истребител",
    "су-34",
    "су-35",
    "су-30",
    "су-24",
    "су-25",
    "су-57",
];
const BALLISTIC: &[&str] = &["балісти", "іскандер", "искандер"];
const MISSILE: &[&str] = &[
    "крилат", "ракет", "калібр", "калибр", "х-101", "х-555", "х101", "х555", "швидкісн", "скоростн",
];
const UAV: &[&str] = &[
    "шахед", "шахід", "шахид", "геран", "бпла", "дрон", "ударн",
];
const CONTEXT: &[&str] = &[
    "пуск", "зліт", "злет", "виліт", "вилет", "взлет", "старт", "рух", "зафікс",
    "напрям", "курс", "летить", "летит", "проліт", "проліт", "активність",
];

fn kab_token(lower: &str) -> bool {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|t| matches!(t, "каб" | "каби" | "кабів" | "фаб" | "фаби"))
}

/// Вид пуску або None (не пусковий пост / відбій).
pub fn classify(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if has_any(&lower, ALL_CLEAR) {
        return None;
    }
    if has_any(&lower, MIG) {
        return Some("mig31k".to_string());
    }
    if has_any(&lower, TACTICAL) {
        return Some("tactical".to_string());
    }
    if has_any(&lower, BALLISTIC) {
        return Some("ballistic".to_string());
    }
    if kab_token(&lower) {
        return Some("kab".to_string());
    }
    // «ракетна небезпека» без руху/пусків — це тривога, а не пуск
    if has_any(&lower, MISSILE) {
        if has_any(&lower, CONTEXT) {
            return Some("missile".to_string());
        }
        return None;
    }
    if has_any(&lower, UAV) {
        return Some("uav".to_string());
    }
    if has_any(&lower, &["пуск", "зліт", "злет", "виліт", "вилет", "взлет"]) {
        return Some("unknown".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Носій → точка з launches.rs. ТІЛЬКИ кораблі/літаки/аеродроми на ворожих територіях.
// Пост без впізнаного носія (рух над областю, КАБи без бази, «невідомо») — None:
// це сповіщення-маркер, а не подія пуску зі стрілкою.
// ---------------------------------------------------------------------------

/// id сайта-носія за видом і текстом (текст уже в lowercase) або None.
pub fn carrier_site(kind: &str, lower_text: &str) -> Option<&'static str> {
    let t = lower_text;
    match kind {
        // зліт МіГ-31К: за замовчуванням Саваслейка, Моздок — якщо названо
        "mig31k" => {
            if t.contains("моздок") {
                Some("mozdok")
            } else {
                Some("savasleyka")
            }
        }
        "tactical" | "kab" => {
            if t.contains("єйськ") || t.contains("ейск") {
                Some("yeysk")
            } else if t.contains("міллеров") || t.contains("миллеров") {
                Some("millerovo")
            } else if t.contains("халіно") || t.contains("халино") {
                Some("kursk-khalino")
            } else if t.contains("курськ") || t.contains("курск") || t.contains("курщ") {
                Some("kursk")
            } else if t.contains("бєлгород") || t.contains("белгород") {
                Some("belgorod")
            } else if t.contains("сещ") || t.contains("брянськ") || t.contains("брянск") {
                Some("bryansk-seshcha")
            } else if t.contains("воронеж") {
                Some("voronezh")
            } else {
                None
            }
        }
        // крилаті: стратегічні борти або кораблі; прикордонні пускові райони
        "missile" => {
            if has_any(t, &["ту-95", "ту-160", "енгельс"]) {
                Some("engels-2")
            } else if has_any(t, &["ту-22", "шайковк"]) {
                Some("shaykovka")
            } else if t.contains("олен") {
                Some("olenya")
            } else if has_any(t, &["дягіл", "дягил", "рязан"]) {
                Some("dyagilevo")
            } else if t.contains("каспі") || t.contains("каспи") {
                Some("caspian")
            } else if t.contains("севастопол") {
                Some("sevastopol")
            } else if has_any(
                t,
                &[
                    "калібр", "калибр", "корабл", "фрегат", "підводн", "подводн", "флот",
                    "акваторі", "чорн",
                ],
            ) {
                // канонічний naval-носій Чорного моря
                Some("novorossiysk")
            } else if t.contains("курськ") || t.contains("курск") || t.contains("курщ") {
                Some("kursk")
            } else if t.contains("воронеж") {
                Some("voronezh")
            } else if t.contains("бєлгород") || t.contains("белгород") {
                Some("belgorod")
            } else {
                None
            }
        }
        // балістика: точку ставимо за названим районом пуску на ворожій території
        "ballistic" => {
            if t.contains("джанк") || t.contains("крим") {
                Some("dzhankoy")
            } else if t.contains("бєлгород") || t.contains("белгород") {
                Some("belgorod")
            } else if t.contains("воронеж") {
                Some("voronezh")
            } else if t.contains("курськ") || t.contains("курск") || t.contains("курщ") {
                Some("kursk")
            } else if t.contains("таганрог") || t.contains("ростов") {
                Some("taganrog")
            } else if t.contains("брянськ") || t.contains("брянск") {
                Some("bryansk-seshcha")
            } else {
                None
            }
        }
        // БПЛА: аеродром вильоту або ворожий прикордонний пункт запуску
        "uav" => {
            if t.contains("приморськ") {
                Some("primorsko-akhtarsk")
            } else if t.contains("єйськ") || t.contains("ейск") {
                Some("yeysk")
            } else if t.contains("халіно") || t.contains("халино") || t.contains("курськ") || t.contains("курск") || t.contains("курщ") {
                Some("kursk-khalino")
            } else if t.contains("сещ") || t.contains("брянськ") || t.contains("брянск") {
                Some("bryansk-seshcha")
            } else if has_any(t, &[" з орла", "орел", "орёл", "орловского", "орлівського"]) {
                Some("oryol")
            } else if t.contains("воронеж") {
                Some("voronezh")
            } else if t.contains("бєлгород") || t.contains("белгород") {
                Some("belgorod")
            } else if t.contains("шаталов") {
                Some("shatalovo")
            } else if t.contains("міллеров") || t.contains("миллеров") {
                Some("millerovo")
            } else if t.contains("чауд") {
                Some("chauda")
            } else if t.contains("гвардійськ") || t.contains("гвардейск") {
                Some("hvardijske")
            } else {
                None
            }
        }
        _ => {
            if t.contains("курськ") || t.contains("курск") || t.contains("курщ") {
                Some("kursk")
            } else if t.contains("воронеж") {
                Some("voronezh")
            } else if t.contains("бєлгород") || t.contains("белгород") {
                Some("belgorod")
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Парсинг t.me/s/<channel>: пости з data-post, часом і текстом.
// ---------------------------------------------------------------------------

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    let mut out = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // числові десяткові &#123; або шістнадцяткові &#x1F680;
    while let Some(start) = out.find("&#") {
        let rest = &out[start + 2..];
        let end = rest.find(';').map(|i| start + 2 + i);
        let Some(end) = end else { break };
        let content = &out[start + 2..end];
        let code_point = if let Some(hex) = content.strip_prefix('x').or_else(|| content.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            content.parse::<u32>().ok()
        };
        let replacement = code_point.and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default();
        out.replace_range(start..=end, &replacement);
    }
    out
}

fn clean_text(html: &str) -> String {
    let spaced = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let text = decode_entities(&strip_tags(&spaced));
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 500;
    if collapsed.chars().count() > MAX {
        collapsed.chars().take(MAX - 1).collect::<String>() + "…"
    } else {
        collapsed
    }
}

fn parse_preview(channel: &str, html: &str) -> Vec<Notice> {
    let mut out = Vec::new();
    for chunk in html.split("data-post=\"").skip(1) {
        let Some(q) = chunk.find('"') else { continue };
        let head = &chunk[..q];
        let num = head.split('/').next_back().unwrap_or("").to_string();
        if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let body = &chunk[q..];
        // час поста
        let dt = (|| {
            let i = body.find("datetime=\"")? + "datetime=\"".len();
            let j = body[i..].find('"')? + i;
            body[i..j]
                .parse::<DateTime<chrono::FixedOffset>>()
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })();
        let Some(dt) = dt else { continue };
        // текст поста
        let text = (|| {
            let i = body.find("js-message_text")?;
            let gt = body[i..].find('>')? + i + 1;
            let end = body[gt..].find("</div>")? + gt;
            Some(clean_text(&body[gt..end]))
        })();
        let Some(text) = text else { continue };
        if text.is_empty() {
            continue;
        }
        let Some(kind) = classify(&text) else { continue };
        let (oblast, lat, lon) = match match_oblast(&text.to_lowercase()) {
            Some((n, la, lo)) => (Some(n), Some(la), Some(lo)),
            None => (None, None, None),
        };
        // МіГ без області — канонічна точка Саваслейки (для стрілки коридору)
        let (lat, lon) = if kind == "mig31k" && lat.is_none() {
            (Some(55.322), Some(42.312))
        } else {
            (lat, lon)
        };
        out.push(Notice {
            id: format!("{channel}:{num}"),
            channel: channel.to_string(),
            post_url: format!("https://t.me/{channel}/{num}"),
            text,
            observed_at: dt,
            kind,
            oblast,
            lat,
            lon,
        });
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.observed_at));
    out.truncate(30);
    out
}

pub async fn fetch(channels: &[String]) -> Result<Vec<Notice>, String> {
    let c = client()?;
    let mut all = Vec::new();
    for ch in channels {
        let ch = ch.trim().trim_start_matches('@');
        if ch.is_empty() {
            continue;
        }
        let url = format!("https://t.me/s/{ch}");
        let resp = c
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AlertsMapParser")
            .send()
            .await
            .map_err(|e| format!("official {ch} conn: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("official {ch} http {}", resp.status()));
        }
        let html = resp.text().await.map_err(|e| format!("official {ch} body: {e}"))?;
        all.extend(parse_preview(ch, &html));
    }
    all.sort_by_key(|b| std::cmp::Reverse(b.observed_at));
    all.truncate(60);
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="kpszsu/44101">
      <div class="tgme_widget_message_info"><time class="time" datetime="2026-10-11T18:05:00+03:00">21:05</time></div>
      <div class="tgme_widget_message_text js-message_text" dir="auto">Зафіксовано зліт МіГ-31К з аеродрому Саваслейка!</div>
    </div></div>
    <div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="kpszsu/44102">
      <div class="tgme_widget_message_info"><time class="time" datetime="2026-10-11T18:20:00+03:00">21:20</time></div>
      <div class="tgme_widget_message_text js-message_text" dir="auto">Рух ударних БпЛА на Сумщині у напрямку Чернігівщини!</div>
    </div></div>
    <div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="kpszsu/44103">
      <div class="tgme_widget_message_info"><time class="time" datetime="2026-10-11T18:30:00+03:00">21:30</time></div>
      <div class="tgme_widget_message_text js-message_text" dir="auto">🚨 Ракетна небезпека по всій території України!</div>
    </div></div>
    <div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="kpszsu/44104">
      <div class="tgme_widget_message_info"><time class="time" datetime="2026-10-11T18:40:00+03:00">21:40</time></div>
      <div class="tgme_widget_message_text js-message_text" dir="auto">Відбій загрози ударних БпЛА.</div>
    </div></div>"#;

    #[test]
    fn parses_and_filters_posts() {
        let out = parse_preview("kpszsu", FIXTURE);
        // МіГ + шахеди; «ракетна небезпека» без руху і відбій — відсіяні
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].id, "kpszsu:44102");
        assert_eq!(out[0].kind, "uav");
        assert_eq!(out[0].oblast.as_deref(), Some("Сумська"));
        assert_eq!(out[1].kind, "mig31k");
        assert_eq!(out[1].post_url, "https://t.me/kpszsu/44101");
        // час — київський, приведений до UTC
        assert_eq!(out[1].observed_at.to_rfc3339(), "2026-10-11T15:05:00+00:00");
    }

    #[test]
    fn mig_gets_savasleyka_point() {
        let out = parse_preview("kpszsu", FIXTURE);
        let mig = out.iter().find(|n| n.kind == "mig31k").unwrap();
        assert_eq!((mig.lat.unwrap(), mig.lon.unwrap()), (55.322, 42.312));
    }

    #[test]
    fn classify_kinds() {
        assert_eq!(classify("Пуски КАБ на Сумщині!").as_deref(), Some("kab"));
        assert_eq!(
            classify("Активність ворожої тактичної авіації на східному напрямку!").as_deref(),
            Some("tactical")
        );
        assert_eq!(classify("Виліт Су-34").as_deref(), Some("tactical"));
        assert_eq!(
            classify("Зафіксовано пуск балістики зі сходу").as_deref(),
            Some("ballistic")
        );
        assert_eq!(
            classify("Пуски крилатих ракет з Ту-95").as_deref(),
            Some("missile")
        );
        assert_eq!(classify("Ракетна небезпека!").as_deref(), None);
        assert_eq!(classify("Відбій тривоги.").as_deref(), None);
        assert_eq!(classify("Доброго ранку, Україно!").as_deref(), None);
        assert_eq!(
            classify("Зафіксовано виліт, тип зʼясовується").as_deref(),
            Some("unknown")
        );
    }

    #[test]
    fn oblast_matching() {
        assert_eq!(
            match_oblast("бпла на харківщині"),
            Some(("Харківська".to_string(), 49.70, 36.60))
        );
        // живі форми з постів ПС ЗСУ
        assert_eq!(
            match_oblast("реактивний бпла курсом на городню чернігівщина:").map(|(n, _, _)| n),
            Some("Чернігівська".to_string())
        );
        assert_eq!(
            match_oblast("бпла на черкащині, повз звенигородку").map(|(n, _, _)| n),
            Some("Черкаська".to_string())
        );
        assert_eq!(
            match_oblast("бпла з чорного моря на миколаївщину").map(|(n, _, _)| n),
            Some("Миколаївська".to_string())
        );
        assert_eq!(match_oblast("тиха ніч"), None);
    }

    #[test]
    fn cleans_html() {
        let t = clean_text("Рух БпЛА<br>у напрямку <a href=\"x\">Києва</a> &amp; області &#x1F680;");
        assert_eq!(t, "Рух БпЛА у напрямку Києва & області 🚀");
    }

    #[test]
    fn carrier_planes_ships_airfields() {
        let c = |kind: &str, text: &str| carrier_site(kind, &text.to_lowercase());
        // літаки
        assert_eq!(c("mig31k", "Зліт МіГ-31К з Саваслейки"), Some("savasleyka"));
        assert_eq!(c("mig31k", "Зліт МіГ-31К"), Some("savasleyka"));
        assert_eq!(c("mig31k", "Зліт МіГ з Моздока"), Some("mozdok"));
        assert_eq!(c("missile", "Пуски Х-101 з Ту-95"), Some("engels-2"));
        assert_eq!(c("missile", "Пуски Х-22 з Ту-22"), Some("shaykovka"));
        // кораблі
        assert_eq!(
            c("missile", "Пуски Калібрів з акваторії Чорного моря"),
            Some("novorossiysk")
        );
        assert_eq!(c("missile", "Кораблі вийшли на пускові рубежі"), Some("novorossiysk"));
        assert_eq!(c("missile", "Пуски з Каспію"), Some("caspian"));
        // аеродроми БПЛА
        assert_eq!(c("uav", "Виліт Шахедів з Приморська"), Some("primorsko-akhtarsk"));
        assert_eq!(c("uav", "Пуски з Єйська"), Some("yeysk"));
        // балістика лише за названим районом
        assert_eq!(c("ballistic", "Пуск Іскандера з Криму"), Some("dzhankoy"));
        assert_eq!(c("ballistic", "Пуск балістики з Бєлгородської області"), Some("belgorod"));
        assert_eq!(c("ballistic", "Пуск Іскандера з Воронежа"), Some("voronezh"));
        assert_eq!(c("ballistic", "Пуск балістики з Курська"), Some("kursk"));
        assert_eq!(c("ballistic", "Пуск балістики з Курської області"), Some("kursk"));
        assert_eq!(c("uav", "Пуски БпЛА з Курська"), Some("kursk-khalino"));
        assert_eq!(c("tactical", "Пуски КАБ з Бєлгородщини"), Some("belgorod"));
        assert_eq!(c("tactical", "Пуски КАБ з Курської області"), Some("kursk"));
        assert_eq!(c("ballistic", "Пуск балістики зі сходу"), None);
        // рух над областю — не пуск (не плутати ціль з аеродромом)
        assert_eq!(c("uav", "Рух БпЛА на Сумщині"), None);
        assert_eq!(c("uav", "Шахеди в Орловській області"), None);
        assert_eq!(c("missile", "Ракети курсом на Київ"), None);
        assert_eq!(c("kab", "Пуски КАБ на Сумщині"), None);
        assert_eq!(c("unknown", "Зафіксовано виліт"), None);
    }
}

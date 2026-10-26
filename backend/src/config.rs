use std::env;

/// Конфіг з env. Головні ручки: затримка видачі та увімкнення DIMAP.
#[derive(Debug, Clone)]
pub struct Config {
    pub neptun_base_url: String,
    pub dimap_base_url: String,
    pub shahmap_base_url: String,
    pub radarua_base_url: String,
    pub aviacontrol_base_url: String,
    pub kupol_base_url: String,
    pub eradar_base_url: String,
    pub ukrainealarm_base_url: String,
    /// Не частіше 5с (вимога Neptun, CDN-кеш)
    pub poll_interval_sec: u64,
    /// Публічна затримка (безпека)
    pub public_delay_sec: u64,
    pub enable_dimap: bool,
    pub enable_shahmap: bool,
    pub enable_radarua: bool,
    pub enable_aviacontrol: bool,
    pub enable_kupol: bool,
    pub enable_eradar: bool,
    pub enable_ukrainealarm: bool,
    /// Telegram-канали офіційних сповіщень про пуски (t.me/s/..., через кому)
    pub official_channels: String,
    pub enable_official: bool,
    /// Інтервал перевірки офіційного каналу ПС ЗСУ, с (за замовчуванням 10с)
    pub official_poll_interval_sec: f64,
    /// Евристичний детектор пусків за треками (за замовчуванням ВИМКНЕНО:
    /// пуски беремо лише з офіційних сповіщень із впізнаним носієм)
    pub enable_heuristic: bool,
    pub color_neptun: String,
    pub color_dimap: String,
    pub color_shahmap: String,
    pub color_radarua: String,
    pub color_aviacontrol: String,
    pub color_kupol: String,
    pub color_eradar: String,
    pub color_ukrainealarm: String,
    /// Скільки секунд трек живе без оновлень (внутрішньо для детектора)
    pub track_expire_sec: f64,
    pub host: String,
    pub port: u16,
    pub data_dir: String,
    pub frontend_dir: Option<String>,
    pub enable_debug: bool,
    pub cors_allowed_origin: String,
    #[allow(dead_code)]
    pub api_secret_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            neptun_base_url: get("NEPTUN_BASE_URL", "https://neptun.in.ua"),
            dimap_base_url: get("DIMAP_BASE_URL", "https://api.dimap.live/api"),
            shahmap_base_url: get("SHAHMAP_BASE_URL", "https://shahmap.online"),
            radarua_base_url: get("RADARUA_BASE_URL", "https://radarua.com"),
            aviacontrol_base_url: get("AVIACONTROL_BASE_URL", "https://aviacontrol.com.ua"),
            kupol_base_url: get("KUPOL_BASE_URL", "https://kupol.in.ua/api"),
            eradar_base_url: get("ERADAR_BASE_URL", "https://eradar.app/api"),
            ukrainealarm_base_url: get("UKRAINEALARM_BASE_URL", "https://map.ukrainealarm.com"),
            poll_interval_sec: get("POLL_INTERVAL_SEC", "30").parse().unwrap_or(30),
            public_delay_sec: get("PUBLIC_DELAY_SEC", "5").parse().unwrap_or(5),
            enable_dimap: get("ENABLE_DIMAP", "true").parse().unwrap_or(true),
            enable_shahmap: get("ENABLE_SHAHMAP", "true").parse().unwrap_or(true),
            enable_radarua: get("ENABLE_RADARUA", "true").parse().unwrap_or(true),
            enable_aviacontrol: get("ENABLE_AVIACONTROL", "true").parse().unwrap_or(true),
            enable_kupol: get("ENABLE_KUPOL", "true").parse().unwrap_or(true),
            enable_eradar: get("ENABLE_ERADAR", "true").parse().unwrap_or(true),
            enable_ukrainealarm: get("ENABLE_UKRAINEALARM", "true").parse().unwrap_or(true),
            official_channels: get("OFFICIAL_CHANNELS", "kpszsu"),
            enable_official: get("ENABLE_OFFICIAL", "true").parse().unwrap_or(true),
            official_poll_interval_sec: get("OFFICIAL_POLL_INTERVAL_SEC", "10").parse().unwrap_or(10.0),
            enable_heuristic: get("ENABLE_HEURISTIC", "false").parse().unwrap_or(false),
            color_neptun: get("COLOR_NEPTUN", "#38e1ff"),
            color_dimap: get("COLOR_DIMAP", "#f5a623"),
            color_shahmap: get("COLOR_SHAHMAP", "#c084fc"),
            color_radarua: get("COLOR_RADARUA", "#f472b6"),
            color_aviacontrol: get("COLOR_AVIACONTROL", "#ffea00"),
            color_kupol: get("COLOR_KUPOL", "#00e5ff"),
            color_eradar: get("COLOR_ERADAR", "#fb923c"),
            color_ukrainealarm: get("COLOR_UKRAINEALARM", "#10b981"),
            track_expire_sec: get("TRACK_EXPIRE_SEC", "30").parse().unwrap_or(30.0),
            host: get("HOST", "0.0.0.0"),
            port: get("PORT", "8080").parse().unwrap_or(8080),
            data_dir: get("DATA_DIR", "data"),
            frontend_dir: env::var("FRONTEND_DIR").ok().filter(|s| !s.trim().is_empty()),
            enable_debug: get("ENABLE_DEBUG", "false").parse().unwrap_or(false),
            cors_allowed_origin: get("CORS_ORIGIN", "*"),
            api_secret_token: env::var("API_SECRET_TOKEN").ok().filter(|s| !s.trim().is_empty()),
        }
    }

    pub fn official_channel_list(&self) -> Vec<String> {
        self.official_channels
            .split(',')
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn color_for(&self, source: &str) -> &str {
        match source {
            "dimap" => &self.color_dimap,
            "shahmap" => &self.color_shahmap,
            "radarua" => &self.color_radarua,
            "aviacontrol" => &self.color_aviacontrol,
            "kupol" => &self.color_kupol,
            "eradar" => &self.color_eradar,
            "ukrainealarm" => &self.color_ukrainealarm,
            _ => &self.color_neptun,
        }
    }
}

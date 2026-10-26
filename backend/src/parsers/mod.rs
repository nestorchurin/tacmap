use std::sync::OnceLock;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub(crate) fn client() -> Result<&'static reqwest::Client, String> {
    Ok(CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .user_agent(concat!("AlertsMapParser/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    }))
}

pub mod alerts;
pub mod aviacontrol;
pub mod dimap;
pub mod eradar;
pub mod kupol;
pub mod neptun;
pub mod official;
pub mod radarua;
pub mod shahmap;
pub mod ukrainealarm;


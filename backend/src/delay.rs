//! Буфер публічної затримки: віддаємо снапшот віком >= delay_sec.
//! Рішення власника з міркувань безпеки (не світити борти в реальному часі).

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

use crate::models::{Cluster, Threat};
use crate::tracker::now_secs;

pub struct DelayBuffer {
    pub delay_sec: u64,
    inner: RwLock<VecDeque<(f64, Vec<Threat>, Vec<Cluster>)>>,
    maxlen: usize,
}

impl DelayBuffer {
    pub fn new(delay_sec: u64) -> Self {
        Self {
            delay_sec,
            inner: RwLock::new(VecDeque::with_capacity(64)),
            maxlen: 200,
        }
    }

    pub async fn push(&self, threats: Vec<Threat>, clusters: Vec<Cluster>) {
        let mut h = self.inner.write().await;
        h.push_back((now_secs(), threats, clusters));
        while h.len() > self.maxlen {
            h.pop_front();
        }
    }

    /// (threats, clusters, age_sec). Якщо історії мало — найстаріше що є.
    pub async fn get_delayed(&self) -> (Vec<Threat>, Vec<Cluster>, f64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let cutoff = now - self.delay_sec as f64;
        let h = self.inner.read().await;
        let mut best: Option<(f64, Vec<Threat>, Vec<Cluster>)> = None;
        for (ts, threats, clusters) in h.iter() {
            if *ts <= cutoff {
                best = Some((*ts, threats.clone(), clusters.clone()));
            } else {
                break;
            }
        }
        match best.or_else(|| h.front().map(|(ts, t, c)| (*ts, t.clone(), c.clone()))) {
            Some((ts, t, c)) => (t, c, (now - ts).max(0.0)),
            None => (vec![], vec![], 0.0),
        }
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}

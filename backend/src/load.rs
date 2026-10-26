//! Адаптивний кулдаун опитування: чим більше навантаження — тим довша пауза.
//! Працює як TCP congestion control (AIMD):
//! - кожен цикл міряємо скільки він тривав і скільки живих треків;
//! - якщо цикл з'їдає значну частку інтервалу або треків забагато —
//!   інтервал росте (×1.5, до максимуму);
//! - якщо N циклів поспіль спокійно — інтервал повільно вертається до бази.
//!
//! Мінімум ніколи не нижче бази (повага до ліміту Neptun 5с).

/// Треки, при яких тиск = 1.0 (типовий рій ~60-100, масована атака 300+).
#[allow(dead_code)]
pub const THREAT_PRESSURE_NORM: f64 = 200.0;
#[allow(dead_code)]
pub const BUSY_OK_FRACTION: f64 = 0.6;
#[allow(dead_code)]
pub const HEALTHY_STREAK: u32 = 6;

#[allow(dead_code)]
pub struct AdaptiveCooldown {
    base_sec: f64,
    max_sec: f64,
    current_sec: f64,
    streak: u32,
    pub last_score: f64,
}

#[allow(dead_code)]
impl AdaptiveCooldown {
    pub fn new(base_sec: f64, max_sec: f64) -> Self {
        let base = base_sec.max(1.0);
        Self {
            base_sec: base,
            max_sec: max_sec.max(base),
            current_sec: base,
            streak: 0,
            last_score: 0.0,
        }
    }

    pub fn interval_secs(&self) -> f64 {
        self.current_sec
    }

    /// Виклик в кінці кожного циклу.
    pub fn feedback(&mut self, cycle_secs: f64, live_tracks: usize) {
        let busy = if self.current_sec > 0.0 {
            cycle_secs / self.current_sec
        } else {
            1.0
        };
        let pressure = live_tracks as f64 / THREAT_PRESSURE_NORM;
        let score = busy.max(pressure);
        self.last_score = score;
        if score > BUSY_OK_FRACTION {
            self.current_sec = (self.current_sec * 1.5).min(self.max_sec);
            self.streak = 0;
        } else {
            self.streak += 1;
            if self.streak >= HEALTHY_STREAK {
                self.current_sec = (self.current_sec / 1.5).max(self.base_sec);
                self.streak = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_under_load() {
        let mut cd = AdaptiveCooldown::new(5.0, 20.0);
        // масована атака: 400 треків
        cd.feedback(0.5, 400);
        assert!(cd.interval_secs() > 5.0);
        // довгий цикл: 4с роботи при інтервалі 7.5с
        let before = cd.interval_secs();
        cd.feedback(4.0, 10);
        assert!(cd.interval_secs() >= before);
    }

    #[test]
    fn caps_at_max() {
        let mut cd = AdaptiveCooldown::new(5.0, 20.0);
        for _ in 0..20 {
            cd.feedback(5.0, 1000);
        }
        assert_eq!(cd.interval_secs(), 20.0);
    }

    #[test]
    fn recovers_to_base_when_calm() {
        let mut cd = AdaptiveCooldown::new(5.0, 20.0);
        cd.feedback(0.2, 500);
        assert!(cd.interval_secs() > 5.0);
        for _ in 0..30 {
            cd.feedback(0.2, 10);
        }
        assert_eq!(cd.interval_secs(), 5.0);
    }

    #[test]
    fn never_below_base() {
        let mut cd = AdaptiveCooldown::new(5.0, 20.0);
        for _ in 0..50 {
            cd.feedback(0.01, 0);
        }
        assert_eq!(cd.interval_secs(), 5.0);
    }
}

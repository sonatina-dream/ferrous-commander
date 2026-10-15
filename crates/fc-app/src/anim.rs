//! A tiny critically-damped spring for UI motion (the gliding cursor).

/// One animated value chasing a target.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
}

impl Spring {
    const STIFFNESS: f32 = 620.0;
    /// Slightly under-damped so moves feel lively without visible wobble.
    const DAMPING: f32 = 44.0;

    pub fn snap(&mut self, target: f32) {
        self.value = target;
        self.velocity = 0.0;
    }

    /// Advances by `dt` seconds toward `target`. Returns true while still moving.
    pub fn step(&mut self, target: f32, dt: f32) -> bool {
        let mut remaining = dt.clamp(0.0, 0.05);
        while remaining > 0.0 {
            let h = remaining.min(1.0 / 240.0);
            let force = Self::STIFFNESS * (target - self.value) - Self::DAMPING * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if (target - self.value).abs() < 0.002 && self.velocity.abs() < 0.02 {
            self.snap(target);
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target_without_overshooting_much() {
        let mut s = Spring::default();
        let mut peak = 0.0f32;
        let mut frames = 0;
        while s.step(10.0, 1.0 / 60.0) {
            peak = peak.max(s.value);
            frames += 1;
            assert!(frames < 120, "spring did not settle");
        }
        assert_eq!(s.value, 10.0);
        assert!(peak < 10.4, "overshoot {peak}");
        assert!(frames > 5, "moves over several frames");
    }
}

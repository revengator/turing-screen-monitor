// SPDX-License-Identifier: GPL-3.0-or-later
//! Visual levels for this dashboard, not hardware safety limits.

#[derive(Clone, Copy)]
pub struct Thresholds {
    medium: f64,
    high: f64,
    maximum: Option<f64>,
}

pub const LOAD: Thresholds = Thresholds {
    medium: 50.0,
    high: 80.0,
    maximum: Some(100.0),
};
pub const MEMORY: Thresholds = Thresholds {
    medium: 60.0,
    high: 85.0,
    maximum: Some(100.0),
};
pub const DISK: Thresholds = Thresholds {
    medium: 70.0,
    high: 90.0,
    maximum: Some(100.0),
};
pub const CPU_TEMPERATURE: Thresholds = Thresholds {
    medium: 60.0,
    high: 80.0,
    maximum: None,
};
pub const GPU_TEMPERATURE: Thresholds = Thresholds {
    medium: 55.0,
    high: 75.0,
    maximum: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Unavailable,
    Low,
    Medium,
    High,
}

impl Level {
    pub fn classify(value: Option<f64>, thresholds: Thresholds) -> Self {
        let Some(value) = value.filter(|v| {
            v.is_finite() && *v >= 0.0 && thresholds.maximum.is_none_or(|maximum| *v <= maximum)
        }) else {
            return Self::Unavailable;
        };
        // Match the visible number, so 50% cannot look low at a 50% threshold.
        let displayed = format!("{value:.0}").parse::<f64>().unwrap_or(value);
        if displayed >= thresholds.high {
            Self::High
        } else if displayed >= thresholds.medium {
            Self::Medium
        } else {
            Self::Low
        }
    }

    pub fn color(self) -> [u8; 3] {
        match self {
            Self::Unavailable => [128, 145, 160],
            Self::Low => [68, 220, 145],
            Self::Medium => [255, 190, 65],
            Self::High => [255, 90, 100],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "N/A",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_levels_change_at_the_documented_boundaries() {
        for thresholds in [LOAD, MEMORY, DISK, CPU_TEMPERATURE, GPU_TEMPERATURE] {
            for (value, level) in [
                (0.0, Level::Low),
                (thresholds.medium - 0.51, Level::Low),
                (thresholds.medium - 0.49, Level::Medium),
                (thresholds.medium, Level::Medium),
                (thresholds.high - 0.51, Level::Medium),
                (thresholds.high - 0.49, Level::High),
                (thresholds.high, Level::High),
                (100.0, Level::High),
            ] {
                assert_eq!(Level::classify(Some(value), thresholds), level);
            }
            for value in [None, Some(f64::NAN), Some(f64::INFINITY), Some(-1.0)] {
                assert_eq!(Level::classify(value, thresholds), Level::Unavailable);
            }
        }
        assert_eq!(Level::classify(Some(101.0), LOAD), Level::Unavailable);
    }

    #[test]
    fn a_high_individual_metric_is_not_hidden_by_low_or_missing_values() {
        let idle_cpu = Level::classify(Some(2.0), LOAD);
        let hot_cpu = Level::classify(Some(85.0), CPU_TEMPERATURE);
        assert_eq!(idle_cpu.max(hot_cpu), Level::High);
    }
}

use super::classifier::{Assessment, Severity};
use super::config::{AutoModConfig, Sensitivity, capped};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    None,
    Warn,
    Timeout(Duration),
}

pub fn decide(config: &AutoModConfig, assessment: &Assessment) -> Action {
    if !config.enabled {
        return Action::None;
    }
    let (warning, timeout, severe) = match config.sensitivity {
        Sensitivity::Low => (0.95, 0.99, 0.985),
        Sensitivity::Normal => (0.90, 0.97, 0.95),
        Sensitivity::High => (0.84, 0.96, 0.93),
    };
    let can_warn = config.warnings_enabled && assessment.confidence >= warning;
    match assessment.severity {
        Severity::Safe | Severity::Mild => Action::None,
        Severity::Warning => {
            if can_warn {
                Action::Warn
            } else {
                Action::None
            }
        }
        Severity::Timeout if config.timeouts_enabled && assessment.confidence >= timeout => {
            let duration = if assessment.categories.iter().any(|c| c.severe()) {
                config.serious_timeout
            } else {
                config.moderate_timeout
            };
            Action::Timeout(capped(duration))
        }
        Severity::Severe
            if config.timeouts_enabled
                && assessment.confidence >= severe
                && assessment.categories.iter().any(|c| c.severe()) =>
        {
            let duration = if assessment
                .categories
                .iter()
                .any(|c| matches!(c, super::classifier::Category::SelfHarm))
            {
                config.extreme_timeout
            } else {
                config.severe_timeout
            };
            Action::Timeout(capped(duration))
        }
        _ => {
            if can_warn {
                Action::Warn
            } else {
                Action::None
            }
        }
    }
}

/// The first confirmed violation is a warning. A second one within the warning window
/// becomes a timeout, including when both assessments were warning-level.
pub fn with_warning_history(config: &AutoModConfig, classified: Action, warned: bool) -> Action {
    if classified == Action::None {
        return Action::None;
    }
    if !warned {
        return if config.warnings_enabled {
            Action::Warn
        } else {
            Action::None
        };
    }
    if config.timeouts_enabled {
        return match classified {
            Action::Timeout(duration) => Action::Timeout(capped(duration)),
            Action::Warn => Action::Timeout(capped(config.moderate_timeout)),
            Action::None => Action::None,
        };
    }
    if config.warnings_enabled {
        Action::Warn
    } else {
        Action::None
    }
}

pub fn may_timeout(
    is_owner: bool,
    is_bot: bool,
    is_staff: bool,
    can_moderate: bool,
    above_target: bool,
) -> bool {
    !is_owner && !is_bot && !is_staff && can_moderate && above_target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automod::classifier::Category;
    #[test]
    fn conservative_thresholds_and_cap() {
        let mut config = AutoModConfig::default();
        let safe = Assessment {
            severity: Severity::Safe,
            confidence: 1.0,
            categories: vec![],
        };
        assert_eq!(decide(&config, &safe), Action::None);
        let targeted_insult = Assessment {
            severity: Severity::Warning,
            confidence: 0.93,
            categories: vec![Category::Harassment],
        };
        assert_eq!(decide(&config, &targeted_insult), Action::Warn);
        let serious = Assessment {
            severity: Severity::Severe,
            confidence: 0.95,
            categories: vec![Category::SelfHarm],
        };
        config.extreme_timeout = Duration::from_secs(10 * 86400);
        assert_eq!(
            decide(&config, &serious),
            Action::Timeout(super::super::config::MAX_AUTO_TIMEOUT)
        );
        let uncertain = Assessment {
            confidence: 0.5,
            ..serious.clone()
        };
        assert_eq!(decide(&config, &uncertain), Action::None);
        config.enabled = false;
        assert_eq!(decide(&config, &serious), Action::None);
    }
    #[test]
    fn protected_members_and_permissions() {
        for flags in [
            (true, false, false, true, true),
            (false, true, false, true, true),
            (false, false, true, true, true),
            (false, false, false, false, true),
            (false, false, false, true, false),
        ] {
            assert!(!may_timeout(flags.0, flags.1, flags.2, flags.3, flags.4));
        }
    }

    #[test]
    fn one_warning_then_timeout_for_confirmed_repeat() {
        let mut config = AutoModConfig::default();
        assert_eq!(
            with_warning_history(&config, Action::Warn, false),
            Action::Warn
        );
        assert_eq!(
            with_warning_history(&config, Action::Warn, true),
            Action::Timeout(config.moderate_timeout)
        );
        assert_eq!(
            with_warning_history(&config, Action::Timeout(config.extreme_timeout), false),
            Action::Warn
        );
        assert_eq!(
            with_warning_history(&config, Action::Timeout(config.extreme_timeout), true),
            Action::Timeout(config.extreme_timeout)
        );
        assert_eq!(
            with_warning_history(&config, Action::None, true),
            Action::None
        );
        config.timeouts_enabled = false;
        assert_eq!(
            with_warning_history(&config, Action::Warn, true),
            Action::Warn
        );
        config.warnings_enabled = false;
        assert_eq!(
            with_warning_history(&config, Action::Timeout(config.extreme_timeout), false),
            Action::None
        );
    }
}

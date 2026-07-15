//! Evaluation of version-JSON rules against the current OS/arch and the
//! enabled feature set.

use std::collections::HashSet;

use super::model::{Rule, RuleAction};

/// OS name as used in Mojang rules.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Arch as used in Mojang rules ("x86" means 32-bit x86).
pub fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "x86",
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        "arm" => "arm32",
        other => other,
    }
}

fn rule_matches(rule: &Rule, features: &HashSet<String>) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if name != os_name() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            if arch != os_arch() {
                return false;
            }
        }
        // os.version regexes are ignored (treated as matching).
    }
    if let Some(req) = &rule.features {
        for (feature, wanted) in req {
            if features.contains(feature) != *wanted {
                return false;
            }
        }
    }
    true
}

/// Mojang rule semantics: with no rules everything is allowed; otherwise the
/// last matching rule decides, and no match means disallow.
pub fn rules_allow(rules: Option<&[Rule]>, features: &HashSet<String>) -> bool {
    let Some(rules) = rules else { return true };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, features) {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::model::OsRule;

    fn os_rule(action: RuleAction, name: &str) -> Rule {
        Rule {
            action,
            os: Some(OsRule {
                name: Some(name.into()),
                arch: None,
                version: None,
            }),
            features: None,
        }
    }

    #[test]
    fn allow_current_os() {
        let rules = [os_rule(RuleAction::Allow, os_name())];
        assert!(rules_allow(Some(&rules), &HashSet::new()));
    }

    #[test]
    fn allow_all_disallow_other_os() {
        let rules = [
            Rule {
                action: RuleAction::Allow,
                os: None,
                features: None,
            },
            os_rule(RuleAction::Disallow, "beos"),
        ];
        assert!(rules_allow(Some(&rules), &HashSet::new()));
    }

    #[test]
    fn feature_rule_requires_feature() {
        let rules = [Rule {
            action: RuleAction::Allow,
            os: None,
            features: Some([("is_demo_user".to_string(), true)].into_iter().collect()),
        }];
        assert!(!rules_allow(Some(&rules), &HashSet::new()));
        let features: HashSet<String> = ["is_demo_user".to_string()].into_iter().collect();
        assert!(rules_allow(Some(&rules), &features));
    }
}

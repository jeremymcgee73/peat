//! Capability matching for `DeploymentDirective`'s Capability scope (peat#773).
//!
//! `CapabilityMatcher::matches(adv, filter)` evaluates a
//! `CapabilityFilter` against a candidate node's `CapabilityAdvertisement`,
//! returning `true` only if **every** constraint in the filter is satisfied.
//! Replaces the pre-#773 stub in `DeploymentDirective::targets_node` that
//! returned `true` whenever the filter's capability list happened to be
//! empty.
//!
//! ## What the matcher checks
//!
//! 1. **Hardware bounds** (`min_gpu_memory_mb`, `min_memory_mb`,
//!    `min_storage_mb`). Each bound that's `Some(req)` on the filter
//!    must be matched by a `Some(have)` on the advert's
//!    `HardwareSpec` with `have >= req`. Missing hardware info on the
//!    advert is conservatively treated as a non-match — see the
//!    `HardwareSpec` doc-comment for the rationale.
//! 2. **Custom k/v constraints** (`filter.custom`). Each `(k, v)` must
//!    appear in `adv.hardware.custom` with the same value. Same
//!    missing-info posture.
//! 3. **Required capability strings** (`filter.required_capabilities`).
//!    Every string must match an advertised `CapabilityInfo` either by
//!    (a) direct case-insensitive comparison against
//!    `CapabilityInfo.capability_type`, or (b) the SensorType
//!    vocabulary bridge (a filter "ELECTRO_OPTICAL" matches an advert
//!    advertising "EO", since "EO" is `SensorType::ElectroOptical`'s
//!    canonical `code()`).
//!
//! ## Vocabulary
//!
//! The canonical capability-string vocabulary will be formalised by the
//! Capability+Labels ADR (ADR-018 is the closest in-tree precedent and
//! covers the AI-model side). Until that lands, this matcher uses the
//! provisional vocabulary already on the wire:
//!
//! - **Sensor strings:** the `SensorType::code()` short codes —
//!   `EO`, `IR`, `RAD`, `SON`, `ACO`, `SIGINT`, `MAD`. The matcher
//!   accepts both the short code and a small set of common
//!   spelled-out aliases (e.g. `ELECTRO_OPTICAL`, `INFRARED`).
//! - **Compute / AI strings:** free-form SCREAMING_SNAKE_CASE values
//!   already appearing on `CapabilityInfo.capability_type`
//!   (`OBJECT_TRACKING`, `COMPUTE`, `COMMUNICATION`, `CUDA`,
//!   `TENSORRT`, …). The matcher uses case-insensitive direct
//!   comparison for these — no ComputeCapability enum exists yet
//!   (per peat#773 dependencies note).
//!
//! When the Capability+Labels ADR lands, the
//! `normalize_capability_string` helper plus the SensorType-alias
//! table are the two surfaces that need reconciliation.

use crate::cot::types::{CapabilityAdvertisement, CapabilityInfo};
use crate::distribution::directive::CapabilityFilter;
use crate::models::domain::SensorType;

/// Stateless evaluator for capability-based deployment targeting.
///
/// See module-level documentation for what's checked and the
/// provisional canonical-string vocabulary.
pub struct CapabilityMatcher;

impl CapabilityMatcher {
    /// Return `true` iff every constraint in `filter` is satisfied
    /// by `adv`. All-must-match semantics — no constraint defaults
    /// to "pass" except when the filter leaves it unset.
    pub fn matches(adv: &CapabilityAdvertisement, filter: &CapabilityFilter) -> bool {
        // 1. Hardware bounds. Each set bound requires the corresponding
        //    advert field to be present and >= the bound. Missing advert
        //    field is a non-match (conservative — see HardwareSpec doc).
        if let Some(req) = filter.min_gpu_memory_mb {
            if !hardware_bound_satisfied(adv, |hw| hw.gpu_memory_mb, req) {
                return false;
            }
        }
        if let Some(req) = filter.min_memory_mb {
            if !hardware_bound_satisfied(adv, |hw| hw.memory_mb, req) {
                return false;
            }
        }
        if let Some(req) = filter.min_storage_mb {
            if !hardware_bound_satisfied(adv, |hw| hw.storage_mb, req) {
                return false;
            }
        }

        // 2. Custom k/v constraints. Each (k, v) in filter.custom must
        //    appear on adv.hardware.custom with the same value.
        if !filter.custom.is_empty() {
            let adv_custom = adv.hardware.as_ref().map(|hw| &hw.custom);
            for (k, v) in &filter.custom {
                let matches_value = adv_custom
                    .and_then(|c| c.get(k))
                    .map(|av| av == v)
                    .unwrap_or(false);
                if !matches_value {
                    return false;
                }
            }
        }

        // 3. Required capability strings. Each must match an advertised
        //    CapabilityInfo by direct comparison or SensorType bridge.
        for req in &filter.required_capabilities {
            if !capability_matches_any(req, &adv.capabilities) {
                return false;
            }
        }

        true
    }
}

/// Normalise a capability string for comparison: trim whitespace,
/// uppercase ASCII letters. Non-ASCII is left untouched (no Unicode
/// case folding) — the canonical vocabulary is ASCII by construction.
fn normalize_capability_string(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// True if any advertised capability matches the required string,
/// either by direct case-insensitive comparison against
/// `CapabilityInfo.capability_type` or via the SensorType vocabulary
/// bridge.
fn capability_matches_any(required: &str, advertised: &[CapabilityInfo]) -> bool {
    let req_norm = normalize_capability_string(required);

    // (a) Direct case-insensitive match against capability_type.
    if advertised
        .iter()
        .any(|c| normalize_capability_string(&c.capability_type) == req_norm)
    {
        return true;
    }

    // (b) SensorType bridge — if the required string spells out a
    //     SensorType (long or short form), match against the
    //     advert's capability_type using SensorType::code().
    if let Some(sensor) = sensor_type_from_string(&req_norm) {
        let code = sensor.code();
        if advertised
            .iter()
            .any(|c| c.capability_type.eq_ignore_ascii_case(code))
        {
            return true;
        }
    }

    false
}

/// Map an already-normalised (trim + uppercase) capability string to a
/// `SensorType` if one applies. Accepts the canonical short code
/// (`EO`, `IR`, etc.) plus common spelled-out aliases so directives
/// authored against either form match the same nodes.
fn sensor_type_from_string(normalized: &str) -> Option<SensorType> {
    Some(match normalized {
        "EO" | "ELECTRO_OPTICAL" | "ELECTROOPTICAL" | "ELECTRO-OPTICAL" => {
            SensorType::ElectroOptical
        }
        "IR" | "INFRARED" => SensorType::Infrared,
        "RAD" | "RADAR" => SensorType::Radar,
        "SON" | "SONAR" => SensorType::Sonar,
        "ACO" | "ACOUSTIC" => SensorType::Acoustic,
        "SIGINT" => SensorType::Sigint,
        "MAD" => SensorType::Mad,
        _ => return None,
    })
}

/// True if the advert has a `HardwareSpec` whose extracted field is
/// `Some(have)` with `have >= req`. Missing `HardwareSpec` or missing
/// field is a non-match (conservative).
fn hardware_bound_satisfied(
    adv: &CapabilityAdvertisement,
    extract: impl FnOnce(&crate::cot::types::HardwareSpec) -> Option<u64>,
    req: u64,
) -> bool {
    adv.hardware
        .as_ref()
        .and_then(extract)
        .map(|have| have >= req)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cot::types::{
        CapabilityAdvertisement, CapabilityInfo, HardwareSpec, OperationalStatus, Position,
    };
    use std::collections::HashMap;

    fn make_advert(caps: Vec<&str>) -> CapabilityAdvertisement {
        let mut adv = CapabilityAdvertisement::new(
            "p-1".into(),
            "UAV".into(),
            Position::new(0.0, 0.0),
            OperationalStatus::Active,
            1.0,
        );
        for ct in caps {
            adv = adv.with_capability(CapabilityInfo {
                capability_type: ct.to_string(),
                model_name: "test".into(),
                version: "1.0".into(),
                precision: 1.0,
                status: OperationalStatus::Active,
            });
        }
        adv
    }

    fn empty_filter() -> CapabilityFilter {
        CapabilityFilter::default()
    }

    #[test]
    fn empty_filter_matches_any_advert() {
        let adv = make_advert(vec![]);
        assert!(CapabilityMatcher::matches(&adv, &empty_filter()));
    }

    #[test]
    fn direct_capability_string_match_case_insensitive() {
        let adv = make_advert(vec!["OBJECT_TRACKING"]);
        let filter = CapabilityFilter {
            required_capabilities: vec!["object_tracking".into()],
            ..empty_filter()
        };
        assert!(CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn required_capability_missing_fails() {
        let adv = make_advert(vec!["COMPUTE"]);
        let filter = CapabilityFilter {
            required_capabilities: vec!["OBJECT_TRACKING".into()],
            ..empty_filter()
        };
        assert!(!CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn all_required_capabilities_must_match() {
        let adv = make_advert(vec!["COMPUTE", "OBJECT_TRACKING"]);
        let filter_partial_present = CapabilityFilter {
            required_capabilities: vec!["COMPUTE".into(), "MISSING".into()],
            ..empty_filter()
        };
        assert!(!CapabilityMatcher::matches(&adv, &filter_partial_present));

        let filter_all_present = CapabilityFilter {
            required_capabilities: vec!["COMPUTE".into(), "OBJECT_TRACKING".into()],
            ..empty_filter()
        };
        assert!(CapabilityMatcher::matches(&adv, &filter_all_present));
    }

    #[test]
    fn sensor_type_bridge_matches_short_code_advert_via_long_name_filter() {
        // Advert exposes the short code "EO" (what SensorType::ElectroOptical
        // emits via .code()). Filter says "ELECTRO_OPTICAL". Bridge applies.
        let adv = make_advert(vec!["EO"]);
        let filter = CapabilityFilter {
            required_capabilities: vec!["ELECTRO_OPTICAL".into()],
            ..empty_filter()
        };
        assert!(CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn sensor_type_bridge_matches_all_seven_sensors_via_long_aliases() {
        let pairs = [
            ("EO", "electro_optical"),
            ("IR", "infrared"),
            ("RAD", "radar"),
            ("SON", "sonar"),
            ("ACO", "acoustic"),
            ("SIGINT", "sigint"),
            ("MAD", "mad"),
        ];
        for (advert_code, filter_alias) in pairs {
            let adv = make_advert(vec![advert_code]);
            let filter = CapabilityFilter {
                required_capabilities: vec![filter_alias.into()],
                ..empty_filter()
            };
            assert!(
                CapabilityMatcher::matches(&adv, &filter),
                "expected {advert_code} advert to match filter alias '{filter_alias}'"
            );
        }
    }

    #[test]
    fn hardware_bound_missing_advert_field_fails() {
        // Advert has no HardwareSpec at all.
        let adv = make_advert(vec![]);
        let filter = CapabilityFilter {
            min_gpu_memory_mb: Some(8_192),
            ..empty_filter()
        };
        assert!(!CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn hardware_bound_with_present_field_satisfied() {
        let adv = make_advert(vec![]).with_hardware(HardwareSpec {
            gpu_memory_mb: Some(16_384),
            memory_mb: Some(32_768),
            storage_mb: Some(512_000),
            custom: HashMap::new(),
        });
        let filter = CapabilityFilter {
            min_gpu_memory_mb: Some(8_192),
            min_memory_mb: Some(16_384),
            min_storage_mb: Some(100_000),
            ..empty_filter()
        };
        assert!(CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn hardware_bound_below_required_fails() {
        let adv = make_advert(vec![]).with_hardware(HardwareSpec {
            gpu_memory_mb: Some(4_096),
            ..Default::default()
        });
        let filter = CapabilityFilter {
            min_gpu_memory_mb: Some(8_192),
            ..empty_filter()
        };
        assert!(!CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn hardware_bound_at_exactly_required_passes() {
        let adv = make_advert(vec![]).with_hardware(HardwareSpec {
            memory_mb: Some(16_384),
            ..Default::default()
        });
        let filter = CapabilityFilter {
            min_memory_mb: Some(16_384),
            ..empty_filter()
        };
        assert!(CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn custom_field_match_and_mismatch() {
        let mut adv_custom = HashMap::new();
        adv_custom.insert("cpu_arch".to_string(), "aarch64".to_string());
        adv_custom.insert("tensorrt_version".to_string(), "10.0".to_string());
        let adv = make_advert(vec![]).with_hardware(HardwareSpec {
            custom: adv_custom,
            ..Default::default()
        });

        let mut filter_match = empty_filter();
        filter_match
            .custom
            .insert("cpu_arch".into(), "aarch64".into());
        assert!(CapabilityMatcher::matches(&adv, &filter_match));

        let mut filter_value_mismatch = empty_filter();
        filter_value_mismatch
            .custom
            .insert("cpu_arch".into(), "x86_64".into());
        assert!(!CapabilityMatcher::matches(&adv, &filter_value_mismatch));

        let mut filter_key_missing = empty_filter();
        filter_key_missing
            .custom
            .insert("gpu_compute_capability".into(), "8.9".into());
        assert!(!CapabilityMatcher::matches(&adv, &filter_key_missing));
    }

    #[test]
    fn custom_field_advert_has_no_hardware_fails() {
        let adv = make_advert(vec![]);
        let mut filter = empty_filter();
        filter.custom.insert("cpu_arch".into(), "aarch64".into());
        assert!(!CapabilityMatcher::matches(&adv, &filter));
    }

    #[test]
    fn combined_filter_all_constraints_must_pass() {
        let mut adv_custom = HashMap::new();
        adv_custom.insert("cpu_arch".to_string(), "aarch64".to_string());
        let adv = make_advert(vec!["OBJECT_TRACKING", "EO"]).with_hardware(HardwareSpec {
            gpu_memory_mb: Some(16_384),
            memory_mb: Some(32_768),
            custom: adv_custom,
            ..Default::default()
        });

        let mut filter_pass = CapabilityFilter {
            min_gpu_memory_mb: Some(8_192),
            min_memory_mb: Some(16_384),
            required_capabilities: vec!["OBJECT_TRACKING".into(), "ELECTRO_OPTICAL".into()],
            ..empty_filter()
        };
        filter_pass
            .custom
            .insert("cpu_arch".into(), "aarch64".into());
        assert!(CapabilityMatcher::matches(&adv, &filter_pass));

        // Break one constraint at a time — each must independently
        // cause failure.
        let mut filter_break_gpu = filter_pass.clone();
        filter_break_gpu.min_gpu_memory_mb = Some(32_768);
        assert!(!CapabilityMatcher::matches(&adv, &filter_break_gpu));

        let mut filter_break_cap = filter_pass.clone();
        filter_break_cap
            .required_capabilities
            .push("NONEXISTENT".into());
        assert!(!CapabilityMatcher::matches(&adv, &filter_break_cap));

        let mut filter_break_custom = filter_pass.clone();
        filter_break_custom
            .custom
            .insert("cpu_arch".into(), "x86_64".into());
        assert!(!CapabilityMatcher::matches(&adv, &filter_break_custom));
    }
}

//! Placement algebra: the pure domain-movement decision for staging.
//!
//! [`plan_movement`] is the single named decision point for "how does this
//! input reach that target": same-domain placements are referenced in
//! place, different-domain placements cross through the coordinator's
//! verified staging channel, and unattributed placements fall back to the
//! transport's physical alias/endpoint admission (legacy records predate
//! domain tagging, and the physical checks remain the trust step even for
//! tagged placements).
//!
//! The function is deliberately pure: no IO, no SSH, no clocks. Its
//! contract is property-tested without a cluster.

use crate::value::DomainId;

/// How an input placement reaches an execution target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Movement {
    /// The input already lives in a domain the target accesses in place.
    /// The transport still verifies endpoint identity before trusting it.
    ReferenceInPlace,
    /// The content must cross domains through the coordinator's verified
    /// staging channel (hash-checked upload), producing a new placement
    /// record in the target's domain.
    Stage,
    /// The placement cannot reach the target under the domain rules.
    Reject(&'static str),
}

/// Decide how an input placement reaches an execution target.
///
/// Rules, in order:
/// - A placement with no domain attribute is a legacy record: the decision
///   defers to the transport's physical admission (alias/endpoint/root).
/// - A placement whose domain equals the target's domain is referenced in
///   place — zero copy, identity re-verified at read time.
/// - Anything else crosses domains through staged movement. The
///   coordinator mediates: local→remote is one upload, remote→local one
///   download, and remote→remote reads through the source's read-only
///   mount before uploading.
pub fn plan_movement(input: &DomainId, target: &DomainId) -> Movement {
    if input.as_str().is_empty() {
        return Movement::Reject("input placement carries no domain");
    }
    if target.as_str().is_empty() {
        return Movement::Reject("target declares no accessible domain");
    }
    if input == target {
        return Movement::ReferenceInPlace;
    }
    Movement::Stage
}

/// The staging decision for a concrete artifact against a concrete
/// transport: tagged domains decide authoritatively; untagged (legacy,
/// local-default) placements keep the transport's physical admission.
pub fn placement_decision(input: &DomainId, target: &DomainId) -> Movement {
    if input.is_local() {
        // Legacy record or genuinely local file: the physical alias
        // admission decides reference-vs-stage exactly as before tagging.
        return Movement::ReferenceInPlace;
    }
    plan_movement(input, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(name: &str) -> DomainId {
        DomainId::new(name)
    }

    #[test]
    fn same_domain_references_in_place() {
        assert_eq!(
            placement_decision(&domain("hpc-b"), &domain("hpc-b")),
            Movement::ReferenceInPlace
        );
    }

    #[test]
    fn different_domains_stage_through_the_coordinator() {
        assert_eq!(
            placement_decision(&domain("hpc-a"), &domain("hpc-b")),
            Movement::Stage
        );
        assert_eq!(
            placement_decision(&domain("hpc-a"), &DomainId::local()),
            Movement::Stage
        );
    }

    #[test]
    fn local_inputs_defer_to_physical_admission() {
        // Legacy records (domain defaults to local) must not regress to a
        // forced upload: the transport's alias/endpoint checks decide.
        assert_eq!(
            placement_decision(&DomainId::local(), &domain("hpc-b")),
            Movement::ReferenceInPlace
        );
        assert_eq!(
            placement_decision(&DomainId::local(), &DomainId::local()),
            Movement::ReferenceInPlace
        );
    }

    #[test]
    fn unattributed_placements_reject() {
        assert_eq!(
            plan_movement(&domain(""), &domain("hpc-b")),
            Movement::Reject("input placement carries no domain")
        );
        assert_eq!(
            plan_movement(&domain("hpc-b"), &domain("")),
            Movement::Reject("target declares no accessible domain")
        );
    }

    #[test]
    fn the_decision_is_total_and_deterministic() {
        let domains = ["", "local", "hpc-a", "hpc-b", "object-store"];
        for input in domains {
            for target in domains {
                let decision = placement_decision(&domain(input), &domain(target));
                assert_eq!(
                    decision,
                    placement_decision(&domain(input), &domain(target))
                );
                assert!(
                    matches!(
                        decision,
                        Movement::ReferenceInPlace | Movement::Stage | Movement::Reject(_)
                    ),
                    "every pair must decide, got {decision:?} for {input}->{target}"
                );
            }
        }
    }
}

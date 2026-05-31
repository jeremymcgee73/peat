//! Command routing logic for hierarchical dissemination
//!
//! Implements target resolution and routing decisions based on command policies.
//!
//! # Dissemination model (coordinator-mediated)
//!
//! Peat's command dissemination is **coordinator-mediated** at every tier:
//! - **Cell/Cohort scopes**: a coordinator at that tier emits the command; it
//!   gossips down to cell leaders; cell leaders fan out to members. The
//!   per-cell `CommandRouter` participates because it has the membership
//!   data it needs (`cell_id`, `cell_members`, `cohort_id`).
//! - **Federation/Coalition scopes**: the **coordinator** at the federation or
//!   coalition tier resolves targets *at its own layer* and re-emits the
//!   command as `Scope::Cell` or `Scope::Individual` to specific
//!   cells/nodes. Per-cell routers therefore **should never see** an
//!   inbound `Scope::Federation` or `Scope::Coalition` command — if one
//!   reaches `resolve_target` at the cell tier, that signals a coordinator
//!   bug (or a misrouted gossip frame), not a routing decision we can act on.
//!
//! Consequence: `Scope::Federation` and `Scope::Coalition` resolve to
//! `TargetResolution::NotApplicable` at the cell-tier router, with a
//! `tracing::warn!` so the misrouting is visible to operators. This is
//! intentional per peat#904 PR #957 QA finding #4 and Kit's design call —
//! the alternative ("fan out cell-locally when we don't know what else to
//! do") amplifies one command into thousands of cell-level broadcasts at
//! Coalition scale.

use peat_schema::command::v1::{command_target::Scope, HierarchicalCommand};
use std::collections::HashSet;
use tracing::warn;

/// Resolves command targets to specific node IDs
pub struct CommandRouter {
    /// Current node ID
    node_id: String,

    /// Cell ID (if member of a cell)
    cell_id: Option<String>,

    /// Cell members (if leader)
    cell_members: Vec<String>,

    /// Cohort ID (if member of a cohort)
    cohort_id: Option<String>,
}

/// Result of target resolution
#[derive(Debug, Clone, PartialEq)]
pub enum TargetResolution {
    /// Command targets this node directly
    Self_,

    /// Command targets subordinate nodes (IDs listed)
    Subordinates(Vec<String>),

    /// Command targets all cell members
    AllCellMembers(Vec<String>),

    /// Command does not target this node or subordinates
    NotApplicable,
}

impl CommandRouter {
    /// Create new router for a node
    pub fn new(
        node_id: String,
        cell_id: Option<String>,
        cell_members: Vec<String>,
        cohort_id: Option<String>,
    ) -> Self {
        Self {
            node_id,
            cell_id,
            cell_members,
            cohort_id,
        }
    }

    /// Resolve command target to specific nodes
    pub fn resolve_target(&self, command: &HierarchicalCommand) -> TargetResolution {
        let target = match &command.target {
            Some(t) => t,
            None => return TargetResolution::NotApplicable,
        };

        let scope = Scope::try_from(target.scope).unwrap_or(Scope::Unspecified);

        match scope {
            Scope::Individual => {
                // Target specific individuals
                let target_ids: HashSet<String> = target.target_ids.iter().cloned().collect();

                if target_ids.contains(&self.node_id) {
                    TargetResolution::Self_
                } else {
                    // Check if any subordinates are targeted
                    let subordinate_targets: Vec<String> = self
                        .cell_members
                        .iter()
                        .filter(|m| target_ids.contains(*m))
                        .cloned()
                        .collect();

                    if !subordinate_targets.is_empty() {
                        TargetResolution::Subordinates(subordinate_targets)
                    } else {
                        TargetResolution::NotApplicable
                    }
                }
            }

            Scope::Cell => {
                // Target entire cell(s)
                if let Some(ref my_cell) = self.cell_id {
                    if target.target_ids.contains(my_cell) {
                        // This cell is targeted
                        if !self.cell_members.is_empty() {
                            // This node is cell leader - target all members
                            TargetResolution::AllCellMembers(self.cell_members.clone())
                        } else {
                            // This node is a cell member - target self
                            TargetResolution::Self_
                        }
                    } else {
                        TargetResolution::NotApplicable
                    }
                } else {
                    TargetResolution::NotApplicable
                }
            }

            Scope::Cohort => {
                // Target entire cohort(s)
                if let Some(ref my_cohort) = self.cohort_id {
                    if target.target_ids.contains(my_cohort) {
                        // This cohort is targeted
                        if !self.cell_members.is_empty() {
                            // This node is cell leader - target all members
                            TargetResolution::AllCellMembers(self.cell_members.clone())
                        } else {
                            // This node is a cohort member - target self
                            TargetResolution::Self_
                        }
                    } else {
                        TargetResolution::NotApplicable
                    }
                } else {
                    TargetResolution::NotApplicable
                }
            }

            // Federation / Coalition targets are coordinator-resolved at the
            // federation/coalition tier and re-emitted as Cell- or Individual-
            // scope commands to specific cells/nodes (see module doc). The
            // per-cell router has no membership data for tier 3/4 and should
            // never see these scopes at this layer. If we get here, a
            // coordinator emitted a Federation/Coalition-scope command
            // without resolving targets, or a gossip frame was misrouted.
            // Either way, this router is not the right place to act on it.
            Scope::Federation | Scope::Coalition => {
                warn!(
                    node_id = %self.node_id,
                    command_id = %command.command_id,
                    scope = ?scope,
                    "Federation/Coalition-scope command reached the cell-tier \
                     router; should have been resolved at the coordinator layer. \
                     Treating as NotApplicable (see peat-protocol::command::routing \
                     module doc)."
                );
                TargetResolution::NotApplicable
            }

            Scope::Broadcast => {
                // Broadcast to all nodes
                if !self.cell_members.is_empty() {
                    // Cell leader - target all members
                    TargetResolution::AllCellMembers(self.cell_members.clone())
                } else {
                    // Regular node - target self
                    TargetResolution::Self_
                }
            }

            Scope::Unspecified => TargetResolution::NotApplicable,
        }
    }

    /// Check if this node should route the command downward
    pub fn should_route(&self, resolution: &TargetResolution) -> bool {
        matches!(
            resolution,
            TargetResolution::Subordinates(_) | TargetResolution::AllCellMembers(_)
        )
    }

    /// Get list of nodes to route command to
    pub fn get_routing_targets(&self, resolution: &TargetResolution) -> Vec<String> {
        match resolution {
            TargetResolution::Subordinates(nodes) => nodes.clone(),
            TargetResolution::AllCellMembers(nodes) => nodes.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peat_schema::command::v1::CommandTarget;

    #[test]
    fn test_resolve_individual_self() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec!["node-1".to_string(), "node-2".to_string()],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-1".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Individual as i32,
                target_ids: vec!["node-1".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(resolution, TargetResolution::Self_);
    }

    #[test]
    fn test_resolve_individual_subordinate() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec!["node-1".to_string(), "node-2".to_string()],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-1".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Individual as i32,
                target_ids: vec!["node-2".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(
            resolution,
            TargetResolution::Subordinates(vec!["node-2".to_string()])
        );
    }

    #[test]
    fn test_resolve_cell() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec![
                "node-1".to_string(),
                "node-2".to_string(),
                "node-3".to_string(),
            ],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-1".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Cell as i32,
                target_ids: vec!["cell-alpha".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        if let TargetResolution::AllCellMembers(members) = resolution {
            assert_eq!(members.len(), 3);
        } else {
            panic!("Expected AllCellMembers resolution");
        }
    }

    // ========================================================================
    // Federation / Coalition routing tests
    //
    // These pin the coordinator-mediated dissemination model documented in
    // this module's doc-comment: Federation/Coalition scopes are resolved at
    // the federation/coalition coordinator tier and re-emitted as Cell- or
    // Individual-scope commands. A per-cell router should NEVER see these
    // scopes inbound; if it does, that's a coordinator bug or a misrouted
    // gossip frame. The router responds with NotApplicable (and emits a
    // tracing::warn).
    //
    // The previously-considered alternative — fan out cell-locally when the
    // router has no higher-tier membership signal — was explicitly rejected
    // because it amplifies one command into thousands of cell broadcasts at
    // Coalition scale (peat#904 PR #957, QA finding #4 + Kit's design call).
    // ========================================================================

    #[test]
    fn test_resolve_federation_at_cell_router_is_not_applicable_even_as_leader() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec![
                "node-1".to_string(),
                "node-2".to_string(),
                "node-3".to_string(),
            ],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-fed-1".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Federation as i32,
                target_ids: vec!["fed-1".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(
            resolution,
            TargetResolution::NotApplicable,
            "Federation-scope command at a cell-tier router must not fan out, \
             even for a cell leader — the federation coordinator is responsible \
             for resolving targets and re-emitting as Cell/Individual scope."
        );
    }

    #[test]
    fn test_resolve_federation_at_cell_router_is_not_applicable_for_non_leader() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec![],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-fed-2".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Federation as i32,
                target_ids: vec!["fed-1".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(
            resolution,
            TargetResolution::NotApplicable,
            "Federation-scope command at a cell-tier non-leader router is \
             NotApplicable; coordinator-mediated dissemination model."
        );
    }

    #[test]
    fn test_resolve_coalition_at_cell_router_is_not_applicable_even_as_leader() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec!["node-1".to_string(), "node-2".to_string()],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-coa-1".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Coalition as i32,
                target_ids: vec!["coa-1".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(
            resolution,
            TargetResolution::NotApplicable,
            "Coalition-scope command at a cell-tier router must not fan out, \
             even for a cell leader — the coalition coordinator is responsible \
             for resolving targets and re-emitting as Cell/Individual scope."
        );
    }

    #[test]
    fn test_resolve_coalition_at_cell_router_is_not_applicable_for_non_leader() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec![],
            None,
        );

        let command = HierarchicalCommand {
            command_id: "cmd-coa-2".to_string(),
            target: Some(CommandTarget {
                scope: Scope::Coalition as i32,
                target_ids: vec!["coa-1".to_string()],
            }),
            ..Default::default()
        };

        let resolution = router.resolve_target(&command);
        assert_eq!(
            resolution,
            TargetResolution::NotApplicable,
            "Coalition-scope command at a cell-tier non-leader router is \
             NotApplicable; coordinator-mediated dissemination model."
        );
    }

    #[test]
    fn test_should_route() {
        let router = CommandRouter::new(
            "node-1".to_string(),
            Some("cell-alpha".to_string()),
            vec!["node-1".to_string(), "node-2".to_string()],
            None,
        );

        assert!(router.should_route(&TargetResolution::Subordinates(vec!["node-2".to_string()])));
        assert!(!router.should_route(&TargetResolution::Self_));
        assert!(!router.should_route(&TargetResolution::NotApplicable));
    }
}

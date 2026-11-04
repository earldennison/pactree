use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::trace::BoundaryKind;

pub const EXTERNAL_MUTATION_REQUIREMENT_SCHEMA: &str = "pact.external_mutation_requirement.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ExternalMutationRequirement {
    #[serde(default = "default_requirement_schema")]
    pub schema: String,

    pub requirement_id: String,

    pub idempotency_key: String,

    pub kind: ExternalMutationKind,

    pub boundary: BoundaryKind,

    pub branch_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,

    #[serde(default)]
    pub payload: BTreeMap<String, Value>,

    #[serde(default)]
    pub aux: BTreeMap<String, Value>,
}

fn default_requirement_schema() -> String {
    EXTERNAL_MUTATION_REQUIREMENT_SCHEMA.to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMutationKind {
    RenderComponent,

    ExecuteTool,

    EstimateTokens,

    ReadExternalContent,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryOutcome {
    Committed,

    WaitingExternal,

    Deferred,

    Failed,

    ProjectedReadonly,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BoundaryPolicy {
    pub allow_commit_with_pending_external: bool,

    pub treat_unmet_as_deferred: bool,

    pub block_on_external_failure: bool,
}

impl BoundaryPolicy {
    pub fn for_kind(kind: BoundaryKind) -> Self {
        use BoundaryKind::*;
        match kind {
            Authored => Self {
                allow_commit_with_pending_external: true,
                treat_unmet_as_deferred: false,
                block_on_external_failure: false,
            },

            PreRequest | PostRequestAlteration => Self {
                allow_commit_with_pending_external: false,
                treat_unmet_as_deferred: false,
                block_on_external_failure: true,
            },

            Request => Self {
                allow_commit_with_pending_external: true,
                treat_unmet_as_deferred: false,
                block_on_external_failure: false,
            },

            Response => Self {
                allow_commit_with_pending_external: true,
                treat_unmet_as_deferred: false,
                block_on_external_failure: false,
            },

            PostResponseAlteration => Self {
                allow_commit_with_pending_external: false,
                treat_unmet_as_deferred: false,
                block_on_external_failure: true,
            },

            PostResponse => Self {
                allow_commit_with_pending_external: true,
                treat_unmet_as_deferred: false,
                block_on_external_failure: false,
            },

            Adhoc => Self {
                allow_commit_with_pending_external: true,
                treat_unmet_as_deferred: true,
                block_on_external_failure: false,
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PendingExternalMutations {
    by_key: BTreeMap<String, ExternalMutationRequirement>,

    failures: BTreeMap<String, String>,
}

impl PendingExternalMutations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, requirement: ExternalMutationRequirement) {
        self.by_key
            .insert(requirement.idempotency_key.clone(), requirement);
    }

    pub fn fulfill(&mut self, idempotency_key: &str) -> Option<ExternalMutationRequirement> {
        self.failures.remove(idempotency_key);
        self.by_key.remove(idempotency_key)
    }

    pub fn record_failure(&mut self, idempotency_key: &str, reason: String) {
        self.failures.insert(idempotency_key.to_string(), reason);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ExternalMutationRequirement> {
        self.by_key.values()
    }

    pub fn pending_count(&self) -> usize {
        self.by_key.len()
    }

    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    pub fn failures(&self) -> &BTreeMap<String, String> {
        &self.failures
    }

    pub fn evaluate(&self, policy: &BoundaryPolicy) -> BoundaryOutcome {
        if self.failure_count() > 0 && policy.block_on_external_failure {
            return BoundaryOutcome::Failed;
        }
        if self.pending_count() == 0 {
            return BoundaryOutcome::Committed;
        }
        if policy.treat_unmet_as_deferred {
            return BoundaryOutcome::Deferred;
        }
        if policy.allow_commit_with_pending_external {
            return BoundaryOutcome::Committed;
        }
        BoundaryOutcome::WaitingExternal
    }
}

#[cfg(test)]
mod tests;

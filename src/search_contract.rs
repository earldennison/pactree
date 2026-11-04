use serde::{Deserialize, Serialize};

use crate::error::{PactError, PactResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchSurface {
    Head,

    Branch(String),
}

impl SearchSurface {
    pub fn parse(value: &str) -> PactResult<Self> {
        match value {
            "head" => Ok(Self::Head),
            _ => {
                if let Some(branch) = value.strip_prefix("branch:") {
                    if branch.is_empty() {
                        return Err(PactError::Invalid(format!(
                            "invalid search surface {value:?}: `branch:` requires a \
                             non-empty branch id"
                        )));
                    }
                    return Ok(Self::Branch(branch.to_string()));
                }
                let hint = if value == "composed"
                    || value.starts_with("composed")
                    || value.starts_with("revision:")
                    || value == "history"
                    || value.starts_with("history:")
                    || value == "ledger"
                    || value.starts_with("ledger")
                {
                    " (composed overlays, historical revisions, and ledger-wide \
                     history are explicitly unsupported in search v0.1)"
                } else {
                    ""
                };
                Err(PactError::Invalid(format!(
                    "invalid search surface {value:?}: v0.1 supports `head` and \
                     `branch:<branch-id>` only{hint}; unsupported surfaces are \
                     rejected and never fall back to head"
                )))
            }
        }
    }

    pub fn as_token(&self) -> String {
        match self {
            Self::Head => "head".to_string(),
            Self::Branch(branch) => format!("branch:{branch}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchMode {
    Exact,
    Regex,
    Lexical,
    Dense,
    Hybrid,
}

impl SearchMode {
    pub fn parse(value: &str) -> PactResult<Self> {
        match value {
            "exact" => Ok(Self::Exact),
            "regex" => Ok(Self::Regex),
            "lexical" => Ok(Self::Lexical),
            "dense" => Ok(Self::Dense),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(PactError::Invalid(format!(
                "invalid search mode {value:?}: v0.1 modes are `exact`, `regex`, \
                 `lexical`, `dense`, and `hybrid`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Regex => "regex",
            Self::Lexical => "lexical",
            Self::Dense => "dense",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum SearchFreshnessPolicy {
    #[default]
    RequireCurrent,

    AllowStale,
}

impl SearchFreshnessPolicy {
    pub fn parse(value: &str) -> PactResult<Self> {
        match value {
            "require_current" => Ok(Self::RequireCurrent),
            "allow_stale" => Ok(Self::AllowStale),
            _ => Err(PactError::Invalid(format!(
                "invalid search freshness policy {value:?}: v0.1 policies are \
                 `require_current` and `allow_stale`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequireCurrent => "require_current",
            Self::AllowStale => "allow_stale",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchChangeCursor {
    pub source_file_identity: String,
    pub root_id: String,
    pub branch_id: String,
    pub revision: u64,
    pub ordinal: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchSnapshotInfo {
    pub source_file_identity: String,
    pub root_id: String,
    pub branch_id: String,
    pub head_revision: u64,
    pub search_cursor: PactSearchChangeCursor,
    pub physical_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PactSearchSelectorEligibility {
    pub total_components: u64,
    pub eligible_components: u64,
    pub eligible_ordinals: Vec<u64>,
    pub execution_plan: String,
}

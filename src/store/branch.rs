use super::model::PactStore;
use super::projection::build_projection;
use crate::error::PactResult;
use serde_json::Value;
use std::collections::BTreeMap;

impl PactStore {
    pub(super) fn project_value(&self, revision: Option<u64>) -> PactResult<Value> {
        let _target_revision = revision.unwrap_or(self.head_revision);
        Ok(self.base.clone())
    }

    pub(super) fn first_seen_by_replay(
        &self,
        revision: Option<u64>,
        blob_inline_limit: usize,
    ) -> PactResult<BTreeMap<String, u64>> {
        let _target_revision = revision.unwrap_or(self.head_revision);
        let mut first_seen = BTreeMap::new();
        let base_projection = build_projection(&self.base, &BTreeMap::new(), blob_inline_limit)?;
        for component_id in base_projection.component_ids() {
            first_seen.insert(component_id.clone(), 0);
        }

        Ok(first_seen)
    }
}

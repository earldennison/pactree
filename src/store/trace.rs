use super::model::{
    PactRef, PactRefSpec, PactTransactionTrace, PactTransactionTraceBuildRequest,
    TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA, TRANSACTION_TRACE_SCHEMA,
};
use crate::error::{PactError, PactResult};

impl PactRef {
    fn from_spec(
        root_id: &str,
        branch_id: &str,
        transaction_id: &str,
        label: &str,
        spec: PactRefSpec,
    ) -> PactResult<Self> {
        if spec.revision.is_none() && spec.component_id.is_none() && spec.content_ref.is_none() {
            return Err(PactError::invalid(format!(
                "{label} requires at least one of revision, component_id, or content_ref"
            )));
        }
        Ok(Self {
            root_id: root_id.to_string(),
            branch_id: branch_id.to_string(),
            revision: spec.revision,
            transaction_id: Some(transaction_id.to_string()),
            component_id: spec.component_id,
            content_ref: spec.content_ref,
        })
    }
}

impl PactTransactionTraceBuildRequest {
    fn validate(&self) -> PactResult<()> {
        if self.schema != TRANSACTION_TRACE_BUILD_REQUEST_SCHEMA {
            return Err(PactError::invalid(format!(
                "unsupported transaction trace build request schema: {}",
                self.schema
            )));
        }
        if self.root_id.is_empty() {
            return Err(PactError::invalid(
                "transaction trace build request requires root_id",
            ));
        }
        if self.branch_id.is_empty() {
            return Err(PactError::invalid(
                "transaction trace build request requires branch_id",
            ));
        }
        if self.transaction_id.is_empty() {
            return Err(PactError::invalid(
                "transaction trace build request requires transaction_id",
            ));
        }
        Ok(())
    }
}

impl PactTransactionTrace {
    pub fn build(request: PactTransactionTraceBuildRequest) -> PactResult<Self> {
        request.validate()?;
        let root_id = request.root_id;
        let branch_id = request.branch_id;
        let transaction_id = request.transaction_id;
        let trace = Self {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            authored_ref: PactRef::from_spec(
                &root_id,
                &branch_id,
                &transaction_id,
                "authored_ref",
                request.authored_ref,
            )?,
            pre_request_ref: PactRef::from_spec(
                &root_id,
                &branch_id,
                &transaction_id,
                "pre_request_ref",
                request.pre_request_ref,
            )?,
            post_request_alteration_ref: PactRef::from_spec(
                &root_id,
                &branch_id,
                &transaction_id,
                "post_request_alteration_ref",
                request.post_request_alteration_ref,
            )?,
            request_ref: optional_ref(
                &root_id,
                &branch_id,
                &transaction_id,
                "request_ref",
                request.request_ref,
            )?,
            response_ref: optional_ref(
                &root_id,
                &branch_id,
                &transaction_id,
                "response_ref",
                request.response_ref,
            )?,
            post_response_alteration_ref: optional_ref(
                &root_id,
                &branch_id,
                &transaction_id,
                "post_response_alteration_ref",
                request.post_response_alteration_ref,
            )?,
            post_response_ref: optional_ref(
                &root_id,
                &branch_id,
                &transaction_id,
                "post_response_ref",
                request.post_response_ref,
            )?,
            root_id,
            branch_id,
            transaction_id,
            alteration_ids: request.alteration_ids,
            commit: request.commit,
            replay_trace: request.replay_trace,
            aux: request.aux,
        };
        trace.validate()?;
        Ok(trace)
    }

    pub fn validate(&self) -> PactResult<()> {
        if self.schema != TRANSACTION_TRACE_SCHEMA {
            return Err(PactError::invalid(format!(
                "unsupported transaction trace schema: {}",
                self.schema
            )));
        }
        if self.root_id.is_empty() {
            return Err(PactError::invalid("transaction trace requires root_id"));
        }
        if self.branch_id.is_empty() {
            return Err(PactError::invalid("transaction trace requires branch_id"));
        }
        if self.transaction_id.is_empty() {
            return Err(PactError::invalid(
                "transaction trace requires transaction_id",
            ));
        }
        for (label, pact_ref) in [
            ("authored_ref", &self.authored_ref),
            ("pre_request_ref", &self.pre_request_ref),
            (
                "post_request_alteration_ref",
                &self.post_request_alteration_ref,
            ),
        ] {
            self.validate_ref(label, pact_ref)?;
        }
        for (label, pact_ref) in [
            ("request_ref", self.request_ref.as_ref()),
            ("response_ref", self.response_ref.as_ref()),
            (
                "post_response_alteration_ref",
                self.post_response_alteration_ref.as_ref(),
            ),
            ("post_response_ref", self.post_response_ref.as_ref()),
        ] {
            if let Some(pact_ref) = pact_ref {
                self.validate_ref(label, pact_ref)?;
            }
        }
        Ok(())
    }

    fn validate_ref(&self, label: &str, pact_ref: &PactRef) -> PactResult<()> {
        if pact_ref.root_id != self.root_id {
            return Err(PactError::invalid(format!(
                "{label} root_id does not match trace root_id"
            )));
        }
        if pact_ref.branch_id != self.branch_id {
            return Err(PactError::invalid(format!(
                "{label} branch_id does not match trace branch_id"
            )));
        }
        Ok(())
    }

    pub(super) fn fork_revision(&self) -> Option<u64> {
        [
            self.post_response_ref.as_ref(),
            self.post_response_alteration_ref.as_ref(),
            self.response_ref.as_ref(),
            self.request_ref.as_ref(),
            Some(&self.post_request_alteration_ref),
            Some(&self.pre_request_ref),
            Some(&self.authored_ref),
        ]
        .into_iter()
        .flatten()
        .find_map(|pact_ref| pact_ref.revision)
    }
}

fn optional_ref(
    root_id: &str,
    branch_id: &str,
    transaction_id: &str,
    label: &str,
    spec: Option<PactRefSpec>,
) -> PactResult<Option<PactRef>> {
    spec.map(|spec| PactRef::from_spec(root_id, branch_id, transaction_id, label, spec))
        .transpose()
}

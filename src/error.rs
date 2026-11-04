use crate::components::ComponentValidationError;
use crate::data_structures::PactArrayError;
use crate::mutation::MutationValidationError;
use crate::provider_thread::ProviderThreadError;
use crate::view::ViewError;

pub type PactResult<T> = Result<T, PactError>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PactError {
    #[error("{0}")]
    Invalid(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Codec(String),

    #[error("{0}")]
    CorruptFile(String),

    #[error("{0}")]
    Storage(String),

    #[error("{0}")]
    Resolver(String),

    #[error("{0}")]
    Selector(String),

    #[error("{0}")]
    Provider(String),

    #[error("{0}")]
    Internal(String),

    #[error("{0}")]
    UnresolvedParent(String),

    #[error("{0}")]
    ParentMismatch(String),

    #[error("{0}")]
    Unsupported(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Array(#[from] PactArrayError),

    #[error(transparent)]
    MutationValidation(#[from] MutationValidationError),

    #[error(transparent)]
    NodeValidation(#[from] ComponentValidationError),

    #[error(transparent)]
    ProviderThread(#[from] ProviderThreadError),

    #[error(transparent)]
    View(#[from] ViewError),
}

impl PactError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn codec(msg: impl Into<String>) -> Self {
        Self::Codec(msg.into())
    }

    pub fn corrupt(msg: impl Into<String>) -> Self {
        Self::CorruptFile(msg.into())
    }

    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage(msg.into())
    }

    pub fn resolver(msg: impl Into<String>) -> Self {
        Self::Resolver(msg.into())
    }

    pub fn selector(msg: impl Into<String>) -> Self {
        Self::Selector(msg.into())
    }

    pub fn provider(msg: impl Into<String>) -> Self {
        Self::Provider(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn unresolved_parent(msg: impl Into<String>) -> Self {
        Self::UnresolvedParent(msg.into())
    }

    pub fn parent_mismatch(msg: impl Into<String>) -> Self {
        Self::ParentMismatch(msg.into())
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    pub fn kind(&self) -> PactErrorKind {
        match self {
            Self::Invalid(_) => PactErrorKind::Invalid,
            Self::NotFound(_) => PactErrorKind::NotFound,
            Self::Codec(_) => PactErrorKind::Codec,
            Self::CorruptFile(_) => PactErrorKind::CorruptFile,
            Self::Storage(_) => PactErrorKind::Storage,
            Self::Resolver(_) => PactErrorKind::Resolver,
            Self::Selector(_) => PactErrorKind::Selector,
            Self::Provider(_) | Self::ProviderThread(_) => PactErrorKind::Provider,
            Self::Internal(_) => PactErrorKind::Internal,
            Self::UnresolvedParent(_) => PactErrorKind::UnresolvedParent,
            Self::ParentMismatch(_) => PactErrorKind::ParentMismatch,
            Self::Unsupported(_) => PactErrorKind::Unsupported,
            Self::Io(_) => PactErrorKind::Io,

            Self::Array(
                PactArrayError::MissingDepth { .. }
                | PactArrayError::MissingOffset { .. }
                | PactArrayError::MissingCore,
            ) => PactErrorKind::NotFound,
            Self::Array(_) => PactErrorKind::Invalid,
            Self::MutationValidation(_) => PactErrorKind::Invalid,
            Self::NodeValidation(_) => PactErrorKind::Invalid,

            Self::View(ViewError::SourceBranchNotFound(_)) => PactErrorKind::NotFound,
            Self::View(ViewError::ConsumerBranchAlreadyExists(_))
            | Self::View(ViewError::InvalidPinnedRevision { .. }) => PactErrorKind::Invalid,
            Self::View(ViewError::ConsumerBranchCloneFailed(_)) => PactErrorKind::Internal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PactErrorKind {
    Invalid,
    NotFound,
    Codec,
    CorruptFile,
    Storage,
    Resolver,
    Selector,
    Provider,
    Component,
    Internal,
    UnresolvedParent,
    ParentMismatch,
    Unsupported,
    Io,
}

impl PactErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Codec => "codec",
            Self::CorruptFile => "corrupt_file",
            Self::Storage => "storage",
            Self::Resolver => "resolver",
            Self::Selector => "selector",
            Self::Provider => "provider",
            Self::Component => "component",
            Self::Internal => "internal",
            Self::UnresolvedParent => "unresolved_parent",
            Self::ParentMismatch => "parent_mismatch",
            Self::Unsupported => "unsupported",
            Self::Io => "io",
        }
    }
}

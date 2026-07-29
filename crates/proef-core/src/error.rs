//! Fault taxonomy and stable exit codes (ADR-0009).
//!
//! Errors are categorized by *who is at fault*: the user's input ([`CoreError::User`]),
//! the system under test ([`CoreError::TestFailure`]), or the environment / proef
//! itself ([`CoreError::System`]). The mapping to process exit codes is **total and a
//! public contract**, pinned by the CLI integration test suite.
//!
//! Engines report through the separate [`EngineError`] layer — engines cannot know
//! exit codes, and the core cannot know engine internals; [`EngineErrorClass`] folds
//! into the core taxonomy at the seam.

use std::error::Error;
use std::fmt;

/// Process exit codes — a public contract (`0` ok · `1` test failure · `2` user
/// error · `3` system error), pinned by `assert_cmd` integration tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExitCode {
    /// Everything ran and every test passed.
    Success = 0,
    /// The suite executed; at least one test failed.
    TestFailure = 1,
    /// The user's input is at fault (bad flags, unbound steps, invalid packs, …).
    UserError = 2,
    /// The environment or proef itself is at fault.
    SystemError = 3,
}

impl ExitCode {
    /// The numeric process exit code.
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// Core error taxonomy, categorized by fault (ADR-0009).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The user's input is at fault → exit code 2.
    #[error("{0}")]
    User(String),
    /// A test genuinely failed → exit code 1.
    #[error("{0}")]
    TestFailure(String),
    /// The environment or proef itself is at fault → exit code 3.
    #[error("{message}")]
    System {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying cause, when one exists.
        #[source]
        source: Option<Box<dyn Error + Send + Sync>>,
    },
}

impl CoreError {
    /// A user-fault error (exit 2).
    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }

    /// A test-failure error (exit 1).
    pub fn test_failure(message: impl Into<String>) -> Self {
        Self::TestFailure(message.into())
    }

    /// A system-fault error (exit 3) without an underlying cause.
    pub fn system(message: impl Into<String>) -> Self {
        Self::System {
            message: message.into(),
            source: None,
        }
    }

    /// A system-fault error (exit 3) with an underlying cause.
    pub fn system_with(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::System {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// The stable exit code this error maps to. Total by construction.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::User(_) => ExitCode::UserError,
            Self::TestFailure(_) => ExitCode::TestFailure,
            Self::System { .. } => ExitCode::SystemError,
        }
    }
}

impl From<&CoreError> for ExitCode {
    fn from(err: &CoreError) -> Self {
        err.exit_code()
    }
}

/// How an engine failure folds into the core taxonomy (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorClass {
    /// Infrastructure trouble (connection, native library, …) → [`CoreError::System`].
    Infra,
    /// An assertion in the system under test failed → [`CoreError::TestFailure`].
    AssertFailed,
    /// The engine could not be set up or configured → [`CoreError::System`].
    Setup,
}

/// An error reported by an engine across the seam (ADR-0002 / ADR-0009).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EngineError {
    /// Fault classification, folded into [`CoreError`] by the dispatcher.
    pub class: EngineErrorClass,
    /// Human-readable description of the failure.
    pub message: String,
    /// Underlying cause, when one exists.
    #[source]
    pub source: Option<Box<dyn Error + Send + Sync>>,
}

impl EngineError {
    /// An infrastructure failure ([`EngineErrorClass::Infra`]).
    pub fn infra(message: impl Into<String>) -> Self {
        Self {
            class: EngineErrorClass::Infra,
            message: message.into(),
            source: None,
        }
    }

    /// An assertion failure ([`EngineErrorClass::AssertFailed`]).
    pub fn assert_failed(message: impl Into<String>) -> Self {
        Self {
            class: EngineErrorClass::AssertFailed,
            message: message.into(),
            source: None,
        }
    }

    /// A setup/configuration failure ([`EngineErrorClass::Setup`]).
    pub fn setup(message: impl Into<String>) -> Self {
        Self {
            class: EngineErrorClass::Setup,
            message: message.into(),
            source: None,
        }
    }

    /// Attach an underlying cause.
    #[must_use]
    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }
}

impl From<EngineError> for CoreError {
    fn from(err: EngineError) -> Self {
        match err.class {
            EngineErrorClass::AssertFailed => Self::TestFailure(err.message),
            EngineErrorClass::Infra | EngineErrorClass::Setup => Self::System {
                message: err.message,
                source: err.source,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exit-code mapping is total and pinned.
    #[test]
    fn core_error_exit_codes_are_stable() {
        assert_eq!(CoreError::user("bad flag").exit_code().code(), 2);
        assert_eq!(
            CoreError::test_failure("assert failed").exit_code().code(),
            1
        );
        assert_eq!(CoreError::system("no network").exit_code().code(), 3);
        assert_eq!(ExitCode::Success.code(), 0);
    }

    #[test]
    fn engine_errors_fold_into_the_core_taxonomy() {
        let assert_failed: CoreError = EngineError::assert_failed("status != 200").into();
        assert_eq!(assert_failed.exit_code(), ExitCode::TestFailure);

        let infra: CoreError = EngineError::infra("connection refused").into();
        assert_eq!(infra.exit_code(), ExitCode::SystemError);

        let setup: CoreError = EngineError::setup("libcurl missing").into();
        assert_eq!(setup.exit_code(), ExitCode::SystemError);
    }

    #[test]
    fn engine_error_sources_survive_the_fold() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let core: CoreError = EngineError::infra("connect failed").with_source(io).into();
        let CoreError::System { source, .. } = &core else {
            panic!("expected System variant");
        };
        assert!(source.is_some());
    }
}

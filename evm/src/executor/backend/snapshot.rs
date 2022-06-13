use revm::SubRoutine;

/// Represents a snapshot taken during evm execution
#[derive(Clone, Debug)]
pub struct BackendSnapshot<T> {
    pub db: T,
    /// The subroutine state at a specific point
    pub subroutine: SubRoutine,
}

// === impl BackendSnapshot ===

impl<T> BackendSnapshot<T> {
    pub fn new(db: T, subroutine: SubRoutine) -> Self {
        Self { db, subroutine }
    }
}

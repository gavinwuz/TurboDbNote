//! Shared bounds for connection establishment and individual database operations.
pub const DEFAULT_EXECUTION_SECONDS: u64 = 300;
pub const MAX_EXECUTION_SECONDS: u64 = 3600;
pub fn execution_seconds(value: Option<u64>) -> u64 {
    value
        .unwrap_or(DEFAULT_EXECUTION_SECONDS)
        .clamp(1, MAX_EXECUTION_SECONDS)
}

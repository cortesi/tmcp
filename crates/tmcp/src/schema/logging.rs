use serde::{Deserialize, Serialize};

/// The severity of a log message, mirroring syslog levels (RFC 5424).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum LoggingLevel {
    /// Detailed debugging information.
    Debug,
    /// Normal operational messages.
    Info,
    /// Normal but significant events.
    Notice,
    /// Warning conditions.
    Warning,
    /// Error conditions.
    Error,
    /// Critical conditions.
    Critical,
    /// Action must be taken immediately.
    Alert,
    /// The system is unusable.
    Emergency,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_level_ordering() {
        assert!(LoggingLevel::Emergency > LoggingLevel::Debug);
        assert!(LoggingLevel::Alert > LoggingLevel::Debug);
        assert!(LoggingLevel::Critical > LoggingLevel::Info);
        assert!(LoggingLevel::Error > LoggingLevel::Warning);
        assert!(LoggingLevel::Warning > LoggingLevel::Notice);
        assert!(LoggingLevel::Notice > LoggingLevel::Info);
        assert!(LoggingLevel::Info > LoggingLevel::Debug);

        assert!(LoggingLevel::Debug < LoggingLevel::Emergency);
        assert!(LoggingLevel::Debug < LoggingLevel::Alert);
        assert!(LoggingLevel::Debug < LoggingLevel::Critical);
        assert!(LoggingLevel::Debug < LoggingLevel::Error);
        assert!(LoggingLevel::Debug < LoggingLevel::Warning);
        assert!(LoggingLevel::Debug < LoggingLevel::Notice);
        assert!(LoggingLevel::Debug < LoggingLevel::Info);
    }
}

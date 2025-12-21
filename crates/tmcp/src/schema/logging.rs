use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum LoggingLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
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

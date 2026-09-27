use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A single provenance record capturing who did what, to which service, and
/// with which metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub service: String,
    pub operation: String,
    pub metadata: BTreeMap<String, String>,
}

impl ProvenanceRecord {
    pub fn new(service: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            operation: operation.into(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Format the record into a single, unambiguous log line.
    ///
    /// Values are escaped so that newlines or delimiter characters embedded in
    /// service, operation, or metadata values cannot forge additional log lines
    /// or key/value pairs. Each original value remains recoverable as exactly
    /// one field.
    pub fn log_fields(&self) -> String {
        let mut out = String::new();
        out.push_str("service=");
        out.push_str(&escape_field(&self.service));
        out.push_str(" operation=");
        out.push_str(&escape_field(&self.operation));
        for (key, value) in &self.metadata {
            out.push(' ');
            out.push_str(&escape_field(key));
            out.push('=');
            out.push_str(&escape_field(value));
        }
        out
    }
}

/// Escape a value so it cannot break out of its field.
///
/// Backslash, the `=` key/value delimiter, the space field delimiter, and
/// newline/carriage-return characters are all escaped. Escaping is reversible:
/// `\\` -> `\\\\`, `=` -> `\\=`, ` ` -> `\\ `, `\n` -> `\\n`, `\r` -> `\\r`.
fn escape_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '=' => escaped.push_str("\\="),
            ' ' => escaped.push_str("\\ "),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_fields_escapes_newlines_and_delimiters() {
        let record = ProvenanceRecord::new("svc\noperation=forged", "op=evil")
            .with_metadata("key with space", "value=with=equals\nand newline");

        let line = record.log_fields();

        // No raw newline may survive, so the record stays on one log line.
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));

        // The injected `operation=forged` must not appear as a real field.
        assert!(!line.contains(" operation=forged"));

        // Each original value is recoverable as exactly one field.
        assert!(line.contains("service=svc\\noperation\\=forged"));
        assert!(line.contains("operation=op\\=evil"));
        assert!(line.contains("key\\ with\\ space=value\\=with\\=equals\\nand\\ newline"));
    }

    #[test]
    fn log_fields_round_trips_plain_values() {
        let record = ProvenanceRecord::new("billing", "charge").with_metadata("amount", "42");
        assert_eq!(record.log_fields(), "service=billing operation=charge amount=42");
    }
}

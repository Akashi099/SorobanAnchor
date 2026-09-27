use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A record of a request's provenance: which service handled it, what
/// operation was performed, and any associated metadata.
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

    /// Format the record as a single, delimited log line.
    ///
    /// Values are escaped so that newlines or delimiter characters inside a
    /// value cannot forge additional log lines or key/value pairs. Each
    /// original value remains recoverable as exactly one field.
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
/// Backslash, the field delimiter (`=`), the pair delimiter (space), and
/// newline characters are all escaped. Escaping is reversible: a backslash
/// always introduces an escape sequence, so the original value can be
/// recovered unambiguously.
fn escape_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '=' => escaped.push_str("\\="),
            ' ' => escaped.push_str("\\s"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_fields_formats_plain_values() {
        let record = ProvenanceRecord::new("billing", "charge")
            .with_metadata("region", "us-east-1");
        assert_eq!(
            record.log_fields(),
            "service=billing operation=charge region=us-east-1"
        );
    }

    #[test]
    fn log_fields_escapes_newlines_and_delimiters() {
        let record = ProvenanceRecord::new("billing\nservice=evil", "charge op=forged")
            .with_metadata("note", "line1\nline2")
            .with_metadata("key=injected", "value with spaces");

        let line = record.log_fields();

        // No raw newline may survive, so no extra log line can be forged.
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));

        // The forged key/value pair must not appear as a real pair.
        assert!(!line.contains(" service=evil"));
        assert!(!line.contains(" op=forged"));

        // Each original value is still recoverable as exactly one field.
        assert!(line.contains("service=billing\\nservice\\=evil"));
        assert!(line.contains("operation=charge\\sop\\=forged"));
        assert!(line.contains("note=line1\\nline2"));
        assert!(line.contains("key\\=injected=value\\swith\\sspaces"));
    }

    #[test]
    fn escape_field_is_reversible() {
        let original = "a=b c\nd\\e";
        let escaped = escape_field(original);
        assert_eq!(unescape_field(&escaped), original);
    }

    fn unescape_field(value: &str) -> String {
        let mut out = String::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('\\') => out.push('\\'),
                    Some('=') => out.push('='),
                    Some('s') => out.push(' '),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }
        out
    }
}

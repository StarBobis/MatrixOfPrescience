use serde_json::Value;

/// Utility constants and methods for string/JSON operations.
pub(crate) const TRACE_STEP_TEXT_LIMIT: usize = 280;

pub(crate) struct StrUtils;

impl StrUtils {
    pub(crate) fn trim_non_empty(value: &str) -> Option<String> {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    pub(crate) fn json_str(value: Option<&Value>, key: &str) -> Option<String> {
        value
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .and_then(Self::trim_non_empty)
    }

    pub(crate) fn toml_str(value: Option<&toml::Value>, key: &str) -> Option<String> {
        value
            .and_then(|value| value.get(key))
            .and_then(toml::Value::as_str)
            .and_then(Self::trim_non_empty)
    }

    pub(crate) fn extract_value_text(value: &Value) -> String {
        match value {
            Value::String(content) => content.trim().to_string(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .or_else(|| part.get("content"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string(),
            _ => String::new(),
        }
    }

    pub(crate) fn message_content_text(message: &Value) -> String {
        match message.get("content") {
            Some(Value::String(content)) => content.trim().to_string(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string(),
            _ => String::new(),
        }
    }

    /// Short preview for UI traces and inline snippets: cuts at `max_chars`
    /// and appends an ellipsis instead of a "[Content truncated]" banner.
    pub(crate) fn ellipsis_text(text: String, max_chars: usize) -> String {
        if text.chars().count() <= max_chars {
            return text;
        }

        let mut truncated: String = text.chars().take(max_chars).collect();
        truncated.push('…');
        truncated
    }

    pub(crate) fn strip_ansi_escape_sequences(text: &str) -> String {
        let mut cleaned = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\x1b' && matches!(chars.peek(), Some('[')) {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }

            cleaned.push(ch);
        }

        cleaned
    }

    pub(crate) fn normalize_json_smart_quotes(text: &str) -> String {
        let mut normalized = String::with_capacity(text.len());
        let mut in_ascii_string = false;
        let mut escaped = false;

        for ch in text.chars() {
            if in_ascii_string {
                normalized.push(ch);

                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_ascii_string = false;
                }
                continue;
            }

            match ch {
                '"' => {
                    in_ascii_string = true;
                    normalized.push(ch);
                }
                '\u{201c}' | '\u{201d}' => normalized.push('"'),
                _ => normalized.push(ch),
            }
        }

        normalized
    }
}

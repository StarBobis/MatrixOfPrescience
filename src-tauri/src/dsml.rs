use serde_json::{json, Map, Value};

use crate::message_content_text;

#[derive(Debug)]
struct DsmlTag {
    start: usize,
    end: usize,
    closing: bool,
    name: String,
    attrs: Vec<(String, String)>,
}

fn split_first_token(text: &str) -> (&str, &str) {
    let trimmed = text.trim();

    if let Some((index, _)) = trimmed.char_indices().find(|(_, ch)| ch.is_whitespace()) {
        (&trimmed[..index], trimmed[index..].trim())
    } else {
        (trimmed, "")
    }
}

fn is_dsml_quote(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}')
}

fn dsml_closing_quote(opening: char) -> char {
    match opening {
        '\u{201C}' => '\u{201D}',
        '\u{2018}' => '\u{2019}',
        other => other,
    }
}

fn parse_dsml_attrs(text: &str) -> Vec<(String, String)> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut attrs = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }

        let key_start = index;
        while index < chars.len() && !chars[index].is_whitespace() && chars[index] != '=' {
            index += 1;
        }

        if key_start == index {
            index += 1;
            continue;
        }

        let key = chars[key_start..index]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();

        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }

        let mut value = String::new();
        if index < chars.len() && chars[index] == '=' {
            index += 1;

            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }

            if index < chars.len() && is_dsml_quote(chars[index]) {
                let closing_quote = dsml_closing_quote(chars[index]);
                index += 1;
                let value_start = index;

                while index < chars.len() && chars[index] != closing_quote {
                    index += 1;
                }

                value = chars[value_start..index].iter().collect();

                if index < chars.len() {
                    index += 1;
                }
            } else {
                let value_start = index;
                while index < chars.len() && !chars[index].is_whitespace() {
                    index += 1;
                }
                value = chars[value_start..index].iter().collect();
            }
        }

        attrs.push((key, value));
    }

    attrs
}

fn dsml_attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn dsml_attr_bool(attrs: &[(String, String)], key: &str) -> bool {
    dsml_attr(attrs, key)
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
}

/// Try to find a DSML tag starting at `from` in `content`.
///
/// Recognises both `<DSML | ` (ASCII pipe) and `<DSML \u{FF5C} ` (fullwidth pipe)
/// opening/closing tag prefixes, with optional whitespace around the delimiter.
fn next_dsml_tag(content: &str, from: usize) -> Option<DsmlTag> {
    let mut search_from = from;

    while search_from < content.len() {
        let relative_start = content[search_from..].find('<')?;
        let start = search_from + relative_start;
        let remaining = &content[start..];

        // Detect opening or closing tag: "<DSML" or "</DSML"
        let closing = remaining.starts_with("</DSML");
        if !closing && !remaining.starts_with("<DSML") {
            search_from = start + '<'.len_utf8();
            continue;
        }

        let prefix_len = if closing { "</DSML".len() } else { "<DSML".len() };
        let after_prefix = &content[start + prefix_len..];

        // Consume optional whitespace, then the pipe delimiter (ASCII | or fullwidth \u{FF5C}),
        // then optional whitespace again.  Compute the byte offset past the delimiter.
        let mut chars = after_prefix.chars();
        let mut skip = 0usize;

        // optional whitespace before delimiter
        for ch in chars.by_ref() {
            if !ch.is_whitespace() {
                // put-back: this is the delimiter (or something else)
                let is_pipe = ch == '|' || ch == '\u{FF5C}';
                if !is_pipe {
                    // not a valid DSML tag — advance past '<' and keep searching
                    search_from = start + '<'.len_utf8();
                    break;
                }
                skip += ch.len_utf8(); // count the pipe char
                break;
            }
            skip += ch.len_utf8();
        }

        // If we broke out of the loop early (not a pipe), continue outer while
        if search_from > start {
            continue;
        }

        // optional whitespace after delimiter
        for ch in chars {
            if !ch.is_whitespace() {
                break;
            }
            skip += ch.len_utf8();
        }

        let body_start = start + prefix_len + skip;
        let body_end = body_start + content[body_start..].find('>')?;
        let end = body_end + '>'.len_utf8();
        let body = content[body_start..body_end].trim();
        let (name, attrs) = split_first_token(body);

        return Some(DsmlTag {
            start,
            end,
            closing,
            name: name.trim_start_matches('/').to_ascii_lowercase(),
            attrs: parse_dsml_attrs(attrs),
        });
    }

    None
}

fn collect_dsml_tags(content: &str) -> Vec<DsmlTag> {
    let mut tags = Vec::new();
    let mut index = 0;

    while let Some(tag) = next_dsml_tag(content, index) {
        index = tag.end;
        tags.push(tag);
    }

    tags
}

fn clean_dsml_parameter_text(raw: &str, parameter_name: &str) -> String {
    let without_leading_line = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);

    if parameter_name.eq_ignore_ascii_case("content") {
        without_leading_line
            .trim_end_matches(|ch| ch == '\r' || ch == '\n')
            .to_string()
    } else {
        without_leading_line.trim().to_string()
    }
}

fn dsml_parameter_value(raw: &str, attrs: &[(String, String)], parameter_name: &str) -> Value {
    let text = clean_dsml_parameter_text(raw, parameter_name);

    if dsml_attr_bool(attrs, "string") {
        return Value::String(text);
    }

    serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
}

fn tool_call_value(index: usize, name: String, arguments: Map<String, Value>) -> Value {
    let arguments =
        serde_json::to_string(&Value::Object(arguments)).unwrap_or_else(|_| "{}".into());

    json!({
        "id": format!("dsml-tool-call-{}", index),
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments,
        }
    })
}

fn extract_dsml_tool_calls_from_content(content: &str) -> Vec<Value> {
    let tags = collect_dsml_tags(content);

    if tags.is_empty() {
        return Vec::new();
    }

    let mut calls = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_arguments = Map::new();
    let mut index = 0;

    while index < tags.len() {
        let tag = &tags[index];

        if tag.closing {
            if tag.name == "invoke" {
                if let Some(name) = current_name.take() {
                    calls.push(tool_call_value(calls.len(), name, current_arguments));
                    current_arguments = Map::new();
                }
            }
            index += 1;
            continue;
        }

        match tag.name.as_str() {
            "invoke" => {
                if let Some(name) = current_name.take() {
                    calls.push(tool_call_value(calls.len(), name, current_arguments));
                    current_arguments = Map::new();
                }

                current_name = dsml_attr(&tag.attrs, "name")
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string);
                index += 1;
            }
            "parameter" => {
                let Some(parameter_name) = dsml_attr(&tag.attrs, "name")
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                else {
                    index += 1;
                    continue;
                };

                if current_name.is_none() {
                    index += 1;
                    continue;
                }

                let next_tag = tags.get(index + 1);
                let value_end = next_tag.map(|next| next.start).unwrap_or(content.len());
                let raw_value = &content[tag.end..value_end];
                current_arguments.insert(
                    parameter_name.to_string(),
                    dsml_parameter_value(raw_value, &tag.attrs, parameter_name),
                );

                index += if next_tag
                    .map(|next| next.closing && next.name == "parameter")
                    .unwrap_or(false)
                {
                    2
                } else {
                    1
                };
            }
            _ => {
                index += 1;
            }
        }
    }

    if let Some(name) = current_name {
        calls.push(tool_call_value(calls.len(), name, current_arguments));
    }

    calls
}

pub(crate) fn normalize_dsml_tool_calls_in_message(mut message: Value) -> Value {
    let has_tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| !calls.is_empty())
        .unwrap_or(false);

    if has_tool_calls {
        return message;
    }

    let content = message_content_text(&message);

    if content.is_empty() || !content.contains("DSML") {
        return message;
    }

    let tool_calls = extract_dsml_tool_calls_from_content(&content);

    if !tool_calls.is_empty() {
        message["content"] = json!("");
        message["tool_calls"] = Value::Array(tool_calls);
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ascii_pipe_dsml_tool_call() {
        let content = "\
<DSML | tool_calls>\n\
<DSML | invoke name=\"write_file\">\n\
<DSML | parameter name=\"file\" string=\"true\">include/DX12ShaderDump.hpp\n\
<DSML | parameter name=\"content\" string=\"true\">#pragma once\n\n#include <Windows.h>\n";

        let tool_calls = extract_dsml_tool_calls_from_content(content);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], json!("write_file"));

        let arguments: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap())
                .expect("DSML arguments should be valid JSON");

        assert_eq!(arguments["file"], json!("include/DX12ShaderDump.hpp"));
        assert_eq!(
            arguments["content"],
            json!("#pragma once\n\n#include <Windows.h>")
        );
    }

    #[test]
    fn parses_fullwidth_pipe_dsml_tool_call() {
        let content = format!(
            "\
<DSML {} tool_calls>\n\
<DSML {} invoke name=\"read_file\">\n\
<DSML {} parameter name=\"file\" string=\"true\">src/main.rs\n\
</DSML {} invoke>\n\
</DSML {} tool_calls>",
            '\u{FF5C}', '\u{FF5C}', '\u{FF5C}', '\u{FF5C}', '\u{FF5C}'
        );

        let tool_calls = extract_dsml_tool_calls_from_content(&content);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], json!("read_file"));
        let args: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["file"], json!("src/main.rs"));
    }

    #[test]
    fn parses_multiple_tool_calls_with_different_functions() {
        let content = "\
<DSML | tool_calls>\n\
<DSML | invoke name=\"list_files\">\n\
<DSML | parameter name=\"path\" string=\"true\">src/\n\
<DSML | invoke name=\"write_file\">\n\
<DSML | parameter name=\"file\" string=\"true\">src/lib.rs\n\
<DSML | parameter name=\"content\" string=\"true\">pub fn main() {}\n\
</DSML | invoke>\n\
<DSML | invoke name=\"run_command\">\n\
<DSML | parameter name=\"command\" string=\"true\">cargo\n\
<DSML | parameter name=\"args\" string=\"false\">[\"build\"]\n\
</DSML | invoke>\n\
</DSML | tool_calls>";

        let tool_calls = extract_dsml_tool_calls_from_content(content);

        assert_eq!(tool_calls.len(), 3);

        assert_eq!(tool_calls[0]["function"]["name"], json!("list_files"));
        let args0: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args0["path"], json!("src/"));

        assert_eq!(tool_calls[1]["function"]["name"], json!("write_file"));
        let args1: Value =
            serde_json::from_str(tool_calls[1]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args1["file"], json!("src/lib.rs"));
        assert_eq!(args1["content"], json!("pub fn main() {}"));

        assert_eq!(tool_calls[2]["function"]["name"], json!("run_command"));
        let args2: Value =
            serde_json::from_str(tool_calls[2]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args2["command"], json!("cargo"));
        assert_eq!(args2["args"], json!(["build"]));
    }

    #[test]
    fn parses_tool_calls_without_wrapper() {
        let content = "\
<DSML | invoke name=\"read_file\">\n\
<DSML | parameter name=\"file\" string=\"true\">src/main.rs\n\
</DSML | invoke>";

        let tool_calls = extract_dsml_tool_calls_from_content(content);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], json!("read_file"));
        let args: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["file"], json!("src/main.rs"));
    }

    #[test]
    fn normalizes_deepseek_dsml_content_into_tool_calls() {
        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": "<DSML | tool_calls>\n<DSML | invoke name=\"write_file\">\n<DSML | parameter name=\"content\" string=\"true\">body"
        }));

        assert_eq!(message["content"], json!(""));
        assert_eq!(
            message["tool_calls"][0]["function"]["name"],
            json!("write_file")
        );
    }

    #[test]
    fn normalize_skips_when_already_has_tool_calls() {
        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": "<DSML | tool_calls>...",
            "tool_calls": [{"id": "existing", "type": "function", "function": {"name": "f", "arguments": "{}"}}]
        }));

        assert_eq!(message["content"], json!("<DSML | tool_calls>..."));
        assert_eq!(message["tool_calls"][0]["id"], json!("existing"));
    }

    #[test]
    fn normalize_skips_no_dsml_content() {
        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": "plain text without any special markers"
        }));

        assert_eq!(
            message["content"],
            json!("plain text without any special markers")
        );
        assert!(message.get("tool_calls").is_none());
    }
}

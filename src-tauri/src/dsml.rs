use serde_json::{json, Map, Value};

use crate::StrUtils;

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
    matches!(
        ch,
        '"' | '\'' | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}'
    )
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
/// Recognises legacy `<DSML | ...>` tags and native `<｜｜DSML｜｜...>` tags.
fn next_dsml_tag(content: &str, from: usize) -> Option<DsmlTag> {
    let mut search_from = from;

    while search_from < content.len() {
        let relative_start = content[search_from..].find('<')?;
        let start = search_from + relative_start;
        let remaining = &content[start..];

        let prefix = if remaining.starts_with("</｜｜DSML｜｜") {
            Some((true, "</｜｜DSML｜｜".len(), false))
        } else if remaining.starts_with("<｜｜DSML｜｜") {
            Some((false, "<｜｜DSML｜｜".len(), false))
        } else if remaining.starts_with("</DSML") {
            Some((true, "</DSML".len(), true))
        } else if remaining.starts_with("<DSML") {
            Some((false, "<DSML".len(), true))
        } else {
            None
        };

        let Some((closing, prefix_len, requires_pipe_delimiter)) = prefix else {
            search_from = start + '<'.len_utf8();
            continue;
        };
        let after_prefix = &content[start + prefix_len..];
        let mut skip = 0usize;

        if requires_pipe_delimiter {
            let mut chars = after_prefix.chars();

            for ch in chars.by_ref() {
                if !ch.is_whitespace() {
                    if ch != '|' && ch != '\u{FF5C}' {
                        search_from = start + '<'.len_utf8();
                        break;
                    }
                    skip += ch.len_utf8();
                    break;
                }
                skip += ch.len_utf8();
            }

            if search_from > start {
                continue;
            }

            for ch in chars {
                if !ch.is_whitespace() {
                    break;
                }
                skip += ch.len_utf8();
            }
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

/// Remove DSML tool payloads while preserving prose before and after each payload.
fn strip_dsml_payloads(content: &str) -> String {
    let tags = collect_dsml_tags(content);
    if tags.is_empty() {
        return content.to_string();
    }

    let mut ranges = Vec::new();
    let mut wrapper_start = None;

    for tag in &tags {
        if tag.name != "tool_calls" {
            continue;
        }

        if tag.closing {
            if let Some(start) = wrapper_start.take() {
                ranges.push((start, tag.end));
            }
        } else if wrapper_start.is_none() {
            wrapper_start = Some(tag.start);
        }
    }

    if let Some(start) = wrapper_start {
        ranges.push((start, content.len()));
    }

    if ranges.is_empty() {
        let mut invoke_start = None;
        for tag in &tags {
            if tag.name != "invoke" {
                continue;
            }

            if tag.closing {
                if let Some(start) = invoke_start.take() {
                    ranges.push((start, tag.end));
                }
            } else {
                if let Some(start) = invoke_start.replace(tag.start) {
                    ranges.push((start, tag.start));
                }
            }
        }

        if let Some(start) = invoke_start {
            ranges.push((start, content.len()));
        }
    }

    if ranges.is_empty() {
        return content.to_string();
    }

    let mut result = String::with_capacity(content.len());
    let mut last_end = 0usize;

    for (start, end) in ranges {
        if start > last_end {
            result.push_str(&content[last_end..start]);
        }
        last_end = end;
    }

    if last_end < content.len() {
        result.push_str(&content[last_end..]);
    }

    result
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

    let content = StrUtils::message_content_text(&message);

    if content.is_empty() || !content.contains('<') {
        return message;
    }

    let tool_calls = extract_dsml_tool_calls_from_content(&content);

    if !tool_calls.is_empty() {
        let cleaned = strip_dsml_payloads(&content);
        message["content"] = json!(cleaned);
        if !has_tool_calls {
            message["tool_calls"] = Value::Array(tool_calls);
        }
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
    fn parses_native_deepseek_dsml_with_multiple_tool_calls() {
        let content = "HookedIASetIndexBuffer (915-933) — need to re-check\n\
HookedClearUnorderedAccessViewUint (1036-1052) — still broken per previous read\n\
Re-read these four to determine which still need patching.\n\n\
<｜｜DSML｜｜tool_calls>\n\
<｜｜DSML｜｜invoke name=“read_file”>\n\
<｜｜DSML｜｜parameter name=“file” string=“true”>src/DirectX12/command/DX12CommandListHooks.cpp</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“maxLines” string=“false”>30</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“startLine” string=“false”>749</｜｜DSML｜｜parameter>\n\
</｜｜DSML｜｜invoke>\n\
<｜｜DSML｜｜invoke name=“read_file”>\n\
<｜｜DSML｜｜parameter name=“file” string=“true”>src/DirectX12/command/DX12CommandListHooks.cpp</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“maxLines” string=“false”>15</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“startLine” string=“false”>547</｜｜DSML｜｜parameter>\n\
</｜｜DSML｜｜invoke>\n\
<｜｜DSML｜｜invoke name=“read_file”>\n\
<｜｜DSML｜｜parameter name=“file” string=“true”>src/DirectX12/command/DX12CommandListHooks.cpp</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“maxLines” string=“false”>50</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“startLine” string=“false”>1032</｜｜DSML｜｜parameter>\n\
</｜｜DSML｜｜invoke>\n\
<｜｜DSML｜｜invoke name=“read_file”>\n\
<｜｜DSML｜｜parameter name=“file” string=“true”>src/DirectX12/command/DX12CommandListHooks.cpp</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“maxLines” string=“false”>50</｜｜DSML｜｜parameter>\n\
<｜｜DSML｜｜parameter name=“startLine” string=“false”>1054</｜｜DSML｜｜parameter>\n\
</｜｜DSML｜｜invoke>\n\
</｜｜DSML｜｜tool_calls>";

        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": content,
        }));
        let tool_calls = message["tool_calls"]
            .as_array()
            .expect("native DeepSeek DSML should become tool calls");

        assert_eq!(tool_calls.len(), 4);
        for tool_call in tool_calls {
            assert_eq!(tool_call["function"]["name"], json!("read_file"));
        }

        let arguments = tool_calls
            .iter()
            .map(|tool_call| {
                serde_json::from_str::<Value>(tool_call["function"]["arguments"].as_str().unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(arguments[0]["maxLines"], json!(30));
        assert_eq!(arguments[0]["startLine"], json!(749));
        assert_eq!(arguments[1]["maxLines"], json!(15));
        assert_eq!(arguments[1]["startLine"], json!(547));
        assert_eq!(arguments[2]["startLine"], json!(1032));
        assert_eq!(arguments[3]["startLine"], json!(1054));

        let cleaned = message["content"].as_str().unwrap();
        assert!(cleaned.contains("Re-read these four"));
        assert!(!cleaned.contains("DSML"));
        assert!(!cleaned.contains("DX12CommandListHooks.cpp"));
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
            "content": "Let me update.\n<DSML | tool_calls>\n<DSML | invoke name=\"write_file\">\n<DSML | parameter name=\"content\" string=\"true\">body"
        }));

        // Content outside DSML tags is preserved
        assert!(message["content"]
            .as_str()
            .unwrap_or("")
            .contains("Let me update."));
        assert_eq!(
            message["tool_calls"][0]["function"]["name"],
            json!("write_file")
        );
    }

    #[test]
    fn parses_native_deepseek_dsml_with_truncated_tail() {
        let dsml = "\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C}";
        let content = format!(
            "\
Reviewing the next reads.\n\
<{dsml}tool_calls>\n\
<{dsml}invoke name=\"read_file\">\n\
<{dsml}parameter name=\"file\" string=\"true\">src/DirectX12/command/DX12CommandListHooks.cpp</{dsml}parameter>\n\
<{dsml}parameter name=\"startLine\" string=\"false\">375</{dsml}parameter>\n\
</{dsml}invoke>\n\
<{dsml}invoke name=\"read_file\">\n\
<{dsml}parameter name=\"file\" string=\"true\">src/DirectX12/command/DX12CommandListHooks.cpp</{dsml}parameter>\n\
<{dsml}"
        );

        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": content,
        }));
        let tool_calls = message["tool_calls"]
            .as_array()
            .expect("truncated native DeepSeek DSML should still become tool calls");

        assert_eq!(tool_calls.len(), 2);
        let first_args: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        let second_args: Value =
            serde_json::from_str(tool_calls[1]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(first_args["startLine"], json!(375));
        assert_eq!(
            second_args["file"],
            json!("src/DirectX12/command/DX12CommandListHooks.cpp")
        );
        assert!(!message["content"].as_str().unwrap_or("").contains("DSML"));
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
    fn normalize_strips_dsml_content_when_tool_calls_already_exist() {
        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": "\
Planning.\n\
<DSML | tool_calls>\n\
<DSML | invoke name=\"read_file\">\n\
<DSML | parameter name=\"file\" string=\"true\">src/lib.rs\n\
</DSML | invoke>\n\
</DSML | tool_calls>\n\
Continue.",
            "tool_calls": [{
                "id": "existing",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"file\":\"src/lib.rs\"}"
                }
            }]
        }));

        let content = message["content"].as_str().unwrap_or("");
        assert!(content.contains("Planning."));
        assert!(content.contains("Continue."));
        assert!(!content.contains("<DSML"));
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

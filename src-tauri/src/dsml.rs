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
    matches!(ch, '"' | '\'' | '“' | '”' | '‘' | '’')
}

fn dsml_closing_quote(opening: char) -> char {
    match opening {
        '“' => '”',
        '‘' => '’',
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

fn next_dsml_tag(content: &str, from: usize) -> Option<DsmlTag> {
    let mut search_from = from;

    while search_from < content.len() {
        let relative_start = content[search_from..].find('<')?;
        let start = search_from + relative_start;
        let remaining = &content[start..];
        let (closing, prefix_len) = if remaining.starts_with("<｜｜DSML｜｜") {
            (false, "<｜｜DSML｜｜".len())
        } else if remaining.starts_with("</｜｜DSML｜｜") {
            (true, "</｜｜DSML｜｜".len())
        } else {
            search_from = start + '<'.len_utf8();
            continue;
        };

        let body_start = start + prefix_len;
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
    fn parses_deepseek_dsml_tool_call_content() {
        let content = "<｜｜DSML｜｜tool_calls>\n\
<｜｜DSML｜｜invoke name=“write_file”>\n\
<｜｜DSML｜｜parameter name=“file” string=“true”>include/DX12ShaderDump.hpp\n\
<｜｜DSML｜｜parameter name=“content” string=“true”>#pragma once\n\n#include <Windows.h>\n";

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
    fn normalizes_deepseek_dsml_content_into_tool_calls() {
        let message = normalize_dsml_tool_calls_in_message(json!({
            "role": "assistant",
            "content": "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=“write_file”>\n<｜｜DSML｜｜parameter name=“content” string=“true”>body"
        }));

        assert_eq!(message["content"], json!(""));
        assert_eq!(
            message["tool_calls"][0]["function"]["name"],
            json!("write_file")
        );
    }
}

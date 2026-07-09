use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionRequest {
    provider_name: String,
    base_url: String,
    api_key: String,
    model: String,
    temperature: Option<f32>,
    system_prompt: Option<String>,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionResponse {
    content: String,
}

#[tauri::command]
async fn chat_completion(
    request: ChatCompletionRequest,
) -> Result<ChatCompletionResponse, String> {
    if request.api_key.trim().is_empty() {
        return Err(format!("{} API Key 未配置", request.provider_name));
    }

    if request.model.trim().is_empty() {
        return Err("模型名称不能为空".to_string());
    }

    let mut messages = Vec::new();
    if let Some(system_prompt) = request.system_prompt.as_deref() {
        let trimmed = system_prompt.trim();
        if !trimmed.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": trimmed,
            }));
        }
    }

    for message in request.messages {
        messages.push(json!({
            "role": message.role,
            "content": message.content,
        }));
    }

    let endpoint = format!(
        "{}/chat/completions",
        request.base_url.trim().trim_end_matches('/')
    );

    let client = reqwest::Client::new();
    let response = client
        .post(endpoint)
        .bearer_auth(request.api_key.trim())
        .json(&json!({
            "model": request.model,
            "messages": messages,
            "temperature": request.temperature.unwrap_or(0.7),
        }))
        .send()
        .await
        .map_err(|error| format!("请求 {} 失败：{}", request.provider_name, error))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取 {} 响应失败：{}", request.provider_name, error))?;

    if !status.is_success() {
        return Err(format!(
            "{} 返回 HTTP {}：{}",
            request.provider_name, status, body
        ));
    }

    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| format!("解析 {} 响应失败：{}", request.provider_name, error))?;

    let content = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if content.is_empty() {
        return Err(format!("{} 未返回可显示的消息内容", request.provider_name));
    }

    Ok(ChatCompletionResponse { content })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![chat_completion])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

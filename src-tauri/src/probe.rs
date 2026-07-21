use std::time::Duration;

use serde_json::Value;

use crate::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeModelProviderRequest {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeModelProviderResponse {
    pub(crate) ok: bool,
    pub(crate) latency_ms: u128,
    pub(crate) model_ids: Vec<String>,
    pub(crate) error: Option<String>,
}

pub(crate) fn models_endpoint(base_url: &str) -> String {
    let normalized = normalize_imported_base_url(base_url);
    format!("{}/models", normalized.trim_end_matches('/'))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn probe_model_provider(request: ProbeModelProviderRequest) -> ProbeModelProviderResponse {
    let started = std::time::Instant::now();
    let base_url = request.base_url.trim();

    if base_url.is_empty() {
        return ProbeModelProviderResponse {
            ok: false,
            latency_ms: 0,
            model_ids: Vec::new(),
            error: Some("Base URL is empty.".to_string()),
        };
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ProbeModelProviderResponse {
                ok: false,
                latency_ms: 0,
                model_ids: Vec::new(),
                error: Some(format!("Failed to build HTTP client: {error}")),
            };
        }
    };

    let mut http = client.get(models_endpoint(base_url));

    if !request.api_key.trim().is_empty() {
        http = http.bearer_auth(request.api_key.trim());
    }

    let response = match http.send().await {
        Ok(response) => response,
        Err(error) => {
            return ProbeModelProviderResponse {
                ok: false,
                latency_ms: started.elapsed().as_millis(),
                model_ids: Vec::new(),
                error: Some(error.to_string()),
            };
        }
    };

    let latency_ms = started.elapsed().as_millis();
    let status = response.status();

    if !status.is_success() {
        return ProbeModelProviderResponse {
            ok: false,
            latency_ms,
            model_ids: Vec::new(),
            error: Some(format!("HTTP {status}")),
        };
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            return ProbeModelProviderResponse {
                ok: false,
                latency_ms,
                model_ids: Vec::new(),
                error: Some(format!("Failed to read response: {error}")),
            };
        }
    };

    ProbeModelProviderResponse {
        ok: true,
        latency_ms,
        model_ids: parse_model_ids(&body),
        error: None,
    }
}

pub(crate) fn parse_model_ids(body: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };

    let mut ids = json
        .get("data")
        .and_then(Value::as_array)
        .map(|data| {
            data.iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod probe_tests {
    use super::parse_model_ids;

    #[test]
    fn parse_model_ids_reads_openai_models_shape() {
        let body = r#"{"object":"list","data":[{"id":"gpt-4.1-mini"},{"id":"deepseek-v4-flash"},{"id":"gpt-4.1-mini"},{"id":" "},{}]}"#;

        assert_eq!(
            parse_model_ids(body),
            vec!["deepseek-v4-flash".to_string(), "gpt-4.1-mini".to_string()]
        );
        assert!(parse_model_ids("not json").is_empty());
    }
}

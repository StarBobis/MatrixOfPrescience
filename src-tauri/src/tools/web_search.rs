use serde_json::Value;
use std::{path::Path, time::Duration};

use crate::tools::execution::{tool_arg_string, tool_arg_string_array, tool_arg_usize};
use crate::utils::string_utils::StrUtils;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "MatrixOfPrescience/0.1 web_search tool";
const MAX_RESULT_SNIPPET_CHARS: usize = 280;
const MAX_TOTAL_CHARS: usize = 12_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SearchSource {
    Web,
    MsLearn,
    Github,
}

impl SearchSource {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "web" | "duckduckgo" | "google" => Some(Self::Web),
            "mslearn" | "microsoft" | "learn" => Some(Self::MsLearn),
            "github" => Some(Self::Github),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::MsLearn => "mslearn",
            Self::Github => "github",
        }
    }
}

struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

/// web_search tool entry: runs inside the async chat loop, so it bridges back
/// into the runtime for the outbound HTTP calls instead of adding a blocking
/// reqwest stack. Oversized results are spilled to a workspace cache file
/// instead of being hard-truncated, so nothing is silently lost.
pub(crate) fn web_search_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let query = tool_arg_string(arguments, "query").trim().to_string();

    if query.is_empty() {
        return Err("web_search requires a non-empty query.".to_string());
    }

    let sources = parse_sources(arguments);
    let max_results = tool_arg_usize(arguments, "maxResults", 5, 1, 10);

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(run_web_search(&query, &sources, max_results))
    })?;

    Ok(crate::utils::spill::spill_tool_output(
        workspace,
        "web-search",
        result,
        MAX_TOTAL_CHARS,
    ))
}

fn parse_sources(arguments: &Value) -> Vec<SearchSource> {
    let requested = tool_arg_string_array(arguments, "sources");
    let mut sources = Vec::new();

    for value in &requested {
        if let Some(source) = SearchSource::parse(value) {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
    }

    if sources.is_empty() {
        vec![SearchSource::Web, SearchSource::MsLearn, SearchSource::Github]
    } else {
        sources
    }
}

async fn run_web_search(
    query: &str,
    sources: &[SearchSource],
    max_results: usize,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;

    let mut sections = Vec::new();

    for source in sources {
        let outcome = match source {
            SearchSource::Web => search_duckduckgo(&client, query, max_results).await,
            SearchSource::MsLearn => search_mslearn(&client, query, max_results).await,
            SearchSource::Github => search_github(&client, query, max_results).await,
        };

        match outcome {
            Ok(hits) if hits.is_empty() => {
                sections.push(format!("[{}] no results.", source.label()));
            }
            Ok(hits) => sections.push(format_hits(source.label(), &hits)),
            Err(error) => {
                sections.push(format!("[{}] search failed: {}", source.label(), error));
            }
        }
    }

    // Full text is returned here; oversized results are spilled to a cache
    // file by web_search_tool instead of being truncated.
    Ok(sections.join("\n\n"))
}

fn format_hits(source: &str, hits: &[SearchHit]) -> String {
    hits.iter()
        .map(|hit| {
            let snippet = if hit.snippet.is_empty() {
                String::new()
            } else {
                format!(
                    "\n  {}",
                    StrUtils::ellipsis_text(hit.snippet.clone(), MAX_RESULT_SNIPPET_CHARS)
                )
            };
            format!("[{source}] {} — {}{}", hit.title, hit.url, snippet)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- Microsoft Learn (official search API) ----------

async fn search_mslearn(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let top = max_results.to_string();
    let response = client
        .get("https://learn.microsoft.com/api/search")
        .query(&[("search", query), ("locale", "en-us"), ("$top", top.as_str())])
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let body = read_success_body(response, "learn.microsoft.com").await?;
    Ok(parse_mslearn_results(&body, max_results))
}

fn parse_mslearn_results(body: &str, max_results: usize) -> Vec<SearchHit> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };

    json.get("results")
        .and_then(Value::as_array)
        .map(|results| {
            results
                .iter()
                .take(max_results)
                .filter_map(|item| {
                    let title = item.get("title")?.as_str()?.trim().to_string();
                    let url = item.get("url")?.as_str()?.trim().to_string();

                    if title.is_empty() || url.is_empty() {
                        return None;
                    }

                    Some(SearchHit {
                        title,
                        url,
                        snippet: item
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------- GitHub (official repository search API) ----------

async fn search_github(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let per_page = max_results.to_string();
    let response = client
        .get("https://api.github.com/search/repositories")
        .query(&[("q", query), ("per_page", per_page.as_str())])
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let body = read_success_body(response, "api.github.com").await?;
    Ok(parse_github_results(&body, max_results))
}

fn parse_github_results(body: &str, max_results: usize) -> Vec<SearchHit> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };

    json.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(max_results)
                .filter_map(|item| {
                    let title = item.get("full_name")?.as_str()?.trim().to_string();
                    let url = item.get("html_url")?.as_str()?.trim().to_string();

                    if title.is_empty() || url.is_empty() {
                        return None;
                    }

                    let stars = item
                        .get("stargazers_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let language = item
                        .get("language")
                        .and_then(Value::as_str)
                        .map(|value| format!(", {value}"))
                        .unwrap_or_default();
                    let description = item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();

                    Some(SearchHit {
                        title,
                        url,
                        snippet: format!("★ {stars}{language} — {description}"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------- DuckDuckGo HTML (general web search) ----------

async fn search_duckduckgo(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let response = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let body = read_success_body(response, "duckduckgo").await?;
    Ok(parse_duckduckgo_results(&body, max_results))
}

fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<SearchHit> {
    let titles = extract_anchors(html, "result__a");
    let snippets = extract_anchors(html, "result__snippet");

    titles
        .into_iter()
        .zip(
            snippets
                .into_iter()
                .map(|(_, snippet)| snippet)
                .chain(std::iter::repeat(String::new())),
        )
        .take(max_results)
        .filter_map(|((href, title), snippet)| {
            let url = extract_uddg_url(&href);
            let title = title.trim().to_string();

            if title.is_empty() || url.is_empty() {
                return None;
            }

            Some(SearchHit {
                title,
                url,
                snippet: snippet.trim().to_string(),
            })
        })
        .collect()
}

/// Extracts (href, inner text) pairs from every anchor whose tag carries the
/// given class marker, in document order.
fn extract_anchors(html: &str, class_marker: &str) -> Vec<(String, String)> {
    let mut anchors = Vec::new();
    let mut rest = html;

    while let Some(class_pos) = rest.find(class_marker) {
        let Some(anchor_start) = rest[..class_pos].rfind("<a ") else {
            break;
        };
        let Some(tag_end_offset) = rest[class_pos..].find('>') else {
            break;
        };
        let tag_end = class_pos + tag_end_offset;
        let tag = &rest[anchor_start..=tag_end];
        let Some(close_offset) = rest[tag_end..].find("</a>") else {
            break;
        };
        let close = tag_end + close_offset;

        anchors.push((
            extract_href(tag).unwrap_or_default(),
            strip_tags(&rest[tag_end + 1..close]),
        ));
        rest = &rest[close + 4..];
    }

    anchors
}

fn extract_href(tag: &str) -> Option<String> {
    let href_pos = tag.find("href=\"")? + "href=\"".len();
    let href_end = tag[href_pos..].find('"')? + href_pos;
    Some(tag[href_pos..href_end].to_string())
}

/// DuckDuckGo wraps outbound links as //duckduckgo.com/l/?uddg=<encoded>.
fn extract_uddg_url(href: &str) -> String {
    if let Some(uddg_pos) = href.find("uddg=") {
        let encoded = &href[uddg_pos + "uddg=".len()..];
        let encoded = encoded.split('&').next().unwrap_or("");
        return percent_decode(encoded);
    }

    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }

    href.to_string()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    output.push(value);
                    index += 3;
                } else {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&output).to_string()
}

fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    decode_entities(output.trim())
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

// ---------- Shared HTTP helpers ----------

async fn read_success_body(response: reqwest::Response, service: &str) -> Result<String, String> {
    let status = response.status();

    if !status.is_success() {
        return Err(format!("{service} returned HTTP {status}"));
    }

    response
        .text()
        .await
        .map_err(|error| format!("failed to read {service} response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_escapes_and_plus() {
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com%2Fdocs%2Fa+b%3Fx%3D1%26y%3D2"),
            "https://example.com/docs/a b?x=1&y=2"
        );
    }

    #[test]
    fn extract_uddg_url_unwraps_redirects_and_protocol_relative_links() {
        assert_eq!(
            extract_uddg_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Ftauri.app%2Fdocs&rut=abc"),
            "https://tauri.app/docs"
        );
        assert_eq!(extract_uddg_url("//example.com/x"), "https://example.com/x");
        assert_eq!(extract_uddg_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn parse_duckduckgo_results_pairs_titles_urls_and_snippets() {
        let html = r#"
            <div class="result results_links">
              <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftauri.app%2Fv1%2Fguides%2F&amp;rut=1">Tauri Guide &amp; Tutorial</a>
              <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftauri.app">Build smaller, faster desktop apps with a web frontend.</a>
            </div>
            <div class="result results_links">
              <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Flearn.microsoft.com%2Fwindows&amp;rut=2">Windows docs</a>
              <a class="result__snippet">Second snippet &lt;here&gt;</a>
            </div>
        "#;

        let hits = parse_duckduckgo_results(html, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Tauri Guide & Tutorial");
        assert_eq!(hits[0].url, "https://tauri.app/v1/guides/");
        assert_eq!(
            hits[0].snippet,
            "Build smaller, faster desktop apps with a web frontend."
        );
        assert_eq!(hits[1].url, "https://learn.microsoft.com/windows");
        assert_eq!(hits[1].snippet, "Second snippet <here>");
    }

    #[test]
    fn parse_mslearn_results_reads_official_shape() {
        let body = r#"{"results":[
            {"title":"Windows Documentation","url":"https://learn.microsoft.com/en-us/windows/","description":"Technical docs.","category":"Documentation"},
            {"title":"","url":"https://skip-me","description":""}
        ]}"#;

        let hits = parse_mslearn_results(body, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Windows Documentation");
        assert_eq!(hits[0].url, "https://learn.microsoft.com/en-us/windows/");
        assert_eq!(hits[0].snippet, "Technical docs.");
    }

    #[test]
    fn parse_github_results_reads_repo_items() {
        let body = r#"{"items":[
            {"full_name":"tauri-apps/tauri","html_url":"https://github.com/tauri-apps/tauri","description":"Build smaller, faster, and more secure apps.","stargazers_count":109227,"language":"Rust"},
            {"full_name":"","html_url":"https://skip-me"}
        ]}"#;

        let hits = parse_github_results(body, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "tauri-apps/tauri");
        assert_eq!(
            hits[0].snippet,
            "★ 109227, Rust — Build smaller, faster, and more secure apps."
        );
    }

    #[test]
    fn parse_sources_defaults_to_all_and_honors_subset() {
        assert_eq!(
            parse_sources(&serde_json::json!({})),
            vec![SearchSource::Web, SearchSource::MsLearn, SearchSource::Github]
        );
        assert_eq!(
            parse_sources(&serde_json::json!({"sources": ["github", "web", "github"]})),
            vec![SearchSource::Github, SearchSource::Web]
        );
        assert_eq!(
            parse_sources(&serde_json::json!({"sources": ["bogus"]})),
            vec![SearchSource::Web, SearchSource::MsLearn, SearchSource::Github]
        );
    }
}

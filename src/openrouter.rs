use serde::{Deserialize, Serialize};

const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const DEFAULT_MODEL: &str = "anthropic/claude-3.5-sonnet";

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct UsageRequest {
    include: bool,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    /// Asks OpenRouter to report the call's real cost alongside tokens.
    usage: UsageRequest,
    /// OpenRouter's server tools (web search and fetch). The model decides
    /// when to use them; OpenRouter runs them inside this one request.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

/// The web tools for one answer, capped by `limits`.
fn web_tools(limits: &crate::web::WebLimits) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "openrouter:web_search",
            "parameters": {
                "max_results": limits.results_per_search,
                "max_total_results": limits.results_per_run,
            }
        }),
        serde_json::json!({ "type": "openrouter:web_fetch" }),
    ]
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Option<ChatResponseMessage>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Option<Vec<ChatChoice>>,
    error: Option<ChatError>,
    usage: Option<ResponseUsage>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    /// USD, as OpenRouter reports it.
    cost: Option<f64>,
}

/// What a completion actually used, as the provider reported it. Any field
/// may be missing (older models, other gateways), so callers record what is
/// there and never guess the rest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost_micros: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub usage: Usage,
}

impl From<ResponseUsage> for Usage {
    fn from(u: ResponseUsage) -> Self {
        Self {
            prompt_tokens: u.prompt_tokens.filter(|t| *t >= 0),
            completion_tokens: u.completion_tokens.filter(|t| *t >= 0),
            cost_micros: u
                .cost
                .filter(|c| c.is_finite() && *c >= 0.0)
                .map(|c| (c * 1_000_000.0).round() as i64),
        }
    }
}

#[derive(Deserialize)]
struct ChatError {
    message: Option<String>,
}

/// One shared client (connection pool + TLS sessions reused) with a hard
/// timeout, so a stalled provider can never hold a request or a dispatcher
/// coroutine open indefinitely.
fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    })
}

/// Calls OpenRouter API to generate a chat completion. The `Err` text is
/// for logs only: it may quote the provider's response, so callers must
/// not return it to clients.
pub async fn call_openrouter(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    call_openrouter_with_usage(api_key, model, system_prompt, user_prompt)
        .await
        .map(|c| c.text)
}

/// `call_openrouter`, plus the tokens and cost the provider reported, so
/// metered calls can record what they really cost.
pub async fn call_openrouter_with_usage(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<Completion, String> {
    call_openrouter_web(api_key, model, system_prompt, user_prompt, None).await
}

/// [`call_openrouter_with_usage`] with the web tools when `web` is given.
/// If the provider refuses the tools (a model without tool support, or a
/// changed tool shape), the answer is asked for again without them rather
/// than lost.
pub async fn call_openrouter_web(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    web: Option<&crate::web::WebLimits>,
) -> Result<Completion, String> {
    match request_completion(api_key, model, system_prompt, user_prompt, web).await {
        Err(RequestError::Refused(why)) if web.is_some() => {
            tracing::warn!(%why, "OpenRouter refused the web tools; answering without them");
            request_completion(api_key, model, system_prompt, user_prompt, None)
                .await
                .map_err(RequestError::into_message)
        }
        other => other.map_err(RequestError::into_message),
    }
}

enum RequestError {
    /// HTTP 400/404/422: the request itself was not accepted.
    Refused(String),
    Other(String),
}

impl RequestError {
    fn into_message(self) -> String {
        match self {
            RequestError::Refused(m) | RequestError::Other(m) => m,
        }
    }
}

async fn request_completion(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    web: Option<&crate::web::WebLimits>,
) -> Result<Completion, RequestError> {
    use RequestError::Other;
    let clean_key = api_key.trim();
    if clean_key.is_empty() {
        return Err(Other("OpenRouter API key is missing or empty".to_string()));
    }

    let chosen_model = if model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        model.trim()
    };

    let client = client();
    let payload = ChatRequest {
        model: chosen_model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: user_prompt.to_string(),
            },
        ],
        usage: UsageRequest { include: true },
        tools: web.map(web_tools),
    };

    let response = client
        .post(OPENROUTER_API_URL)
        .header("Authorization", format!("Bearer {clean_key}"))
        .header("HTTP-Referer", "https://airo-bot.com")
        .header("X-Title", "Air Noticeboard")
        .json(&payload)
        .send()
        .await
        .map_err(|e| Other(format!("Network request to OpenRouter failed: {e}")))?;

    let status = response.status();
    let body_text = response
        .text()
        .await
        .map_err(|e| Other(format!("Failed to read OpenRouter response: {e}")))?;

    if !status.is_success() {
        let message = format!(
            "OpenRouter API returned HTTP {}: {}",
            status.as_u16(),
            body_text
        );
        return Err(if matches!(status.as_u16(), 400 | 404 | 422) {
            RequestError::Refused(message)
        } else {
            Other(message)
        });
    }

    let parsed: ChatResponse = serde_json::from_str(&body_text).map_err(|e| {
        Other(format!(
            "Failed to parse OpenRouter JSON: {e} (raw: {body_text})"
        ))
    })?;

    if let Some(err) = parsed.error {
        if let Some(msg) = err.message {
            return Err(Other(format!("OpenRouter error: {msg}")));
        }
    }

    let usage = parsed.usage.map(Usage::from).unwrap_or_default();
    if let Some(choices) = parsed.choices {
        if let Some(first) = choices.into_iter().next() {
            if let Some(msg) = first.message {
                if let Some(content) = msg.content {
                    return Ok(Completion {
                        text: content.trim().to_string(),
                        usage,
                    });
                }
            }
        }
    }

    Err(Other(
        "OpenRouter returned empty choices in completion".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_tools_are_capped_and_left_out_when_off() {
        let limits = crate::web::WebLimits::default();
        let on = serde_json::to_value(ChatRequest {
            model: "m".into(),
            messages: Vec::new(),
            usage: UsageRequest { include: true },
            tools: Some(web_tools(&limits)),
        })
        .expect("json");
        assert_eq!(on["tools"][0]["type"], "openrouter:web_search");
        assert_eq!(on["tools"][0]["parameters"]["max_results"], 3);
        assert_eq!(on["tools"][0]["parameters"]["max_total_results"], 9);
        assert_eq!(on["tools"][1]["type"], "openrouter:web_fetch");
        let off = serde_json::to_value(ChatRequest {
            model: "m".into(),
            messages: Vec::new(),
            usage: UsageRequest { include: true },
            tools: None,
        })
        .expect("json");
        assert!(off.get("tools").is_none());
    }

    #[test]
    fn usage_converts_dollars_to_micros_and_drops_nonsense() {
        let u: Usage = ResponseUsage {
            prompt_tokens: Some(1200),
            completion_tokens: Some(300),
            cost: Some(0.004_25),
        }
        .into();
        assert_eq!(u.prompt_tokens, Some(1200));
        assert_eq!(u.completion_tokens, Some(300));
        assert_eq!(u.cost_micros, Some(4250));

        let bad: Usage = ResponseUsage {
            prompt_tokens: Some(-1),
            completion_tokens: None,
            cost: Some(f64::NAN),
        }
        .into();
        assert_eq!(bad, Usage::default());
    }

    #[test]
    fn missing_usage_block_parses() {
        let parsed: ChatResponse =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"hi"}}]}"#).expect("parse");
        assert!(parsed.usage.is_none());
        let with: ChatResponse = serde_json::from_str(
            r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7,"cost":0.00001}}"#,
        )
        .expect("parse");
        assert_eq!(
            Usage::from(with.usage.expect("usage")).cost_micros,
            Some(10)
        );
    }
}

use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{Backend, Message, Request, Response, Timing};
use crate::error::{AppError, Result};

const HTTP_TIMEOUT: Duration = Duration::from_secs(720); // 12 min

pub struct OpenAIBackend {
    base_url: String,
    #[allow(dead_code)]
    name: String,
    client: Client,
}

impl OpenAIBackend {
    pub fn new(base_url: &str, name: &str) -> Self {
        let client = Client::builder()
            .http1_only()
            .timeout(HTTP_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(1))
            .build()
            .expect("failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            name: name.to_string(),
            client,
        }
    }

    pub fn with_proxy(mut self, proxy_url: &str) -> Self {
        if !proxy_url.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
                self.client = Client::builder()
                    .http1_only()
                    .timeout(HTTP_TIMEOUT)
                    .pool_idle_timeout(Duration::from_secs(1))
                    .danger_accept_invalid_certs(true)
                    .proxy(proxy)
                    .build()
                    .unwrap_or(self.client);
            }
        }
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.client = Client::builder()
            .http1_only()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(1))
            .build()
            .unwrap_or(self.client);
        self
    }

    fn prepare_messages(req: &Request) -> Vec<Message> {
        if req.no_cache && !req.messages.is_empty() {
            let mut msgs = req.messages.clone();
            msgs[0].content = format!("{} {}", uuid::Uuid::new_v4(), msgs[0].content);
            msgs
        } else {
            req.messages.clone()
        }
    }
}

// --- JSON wire types ---

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: usize,
    temperature: f64,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<ChoiceMsg>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChoiceMsg {
    message: MsgContent,
}

#[derive(Deserialize)]
struct MsgContent {
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIChunk {
    choices: Vec<ChoiceDelta>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChoiceDelta {
    delta: DeltaContent,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct DeltaContent {
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: usize,
}

#[derive(Deserialize, Debug, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[async_trait]
impl Backend for OpenAIBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn with_proxy_opt(&self, proxy: &str) -> Option<Box<dyn Backend>> {
        let new_self = OpenAIBackend {
            base_url: self.base_url.clone(),
            name: self.name.clone(),
            client: self.client.clone(),
        }.with_proxy(proxy);
        Some(Box::new(new_self))
    }

    fn with_timeout_opt(&self, timeout: Duration) -> Option<Box<dyn Backend>> {
        let new_self = OpenAIBackend {
            base_url: self.base_url.clone(),
            name: self.name.clone(),
            client: self.client.clone(),
        }.with_timeout(timeout);
        Some(Box::new(new_self))
    }

    async fn send(&self, req: Request) -> Result<Response> {
        let start = Instant::now();
        let msgs = Self::prepare_messages(&req);

        let body = OpenAIRequest {
            model: &req.model,
            messages: &msgs,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            stream: false,
            stream_options: None,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Backend(format!("HTTP {status}: {text}")));
        }

        let oresp: OpenAIResponse = resp.json().await?;
        let total_dur = start.elapsed();

        let (prompt_tokens, output_tokens, cached_tokens) = match &oresp.usage {
            Some(u) => (
                u.prompt_tokens,
                u.completion_tokens,
                u.prompt_tokens_details.as_ref().map(|d| d.cached_tokens).unwrap_or(0),
            ),
            None => (0, 0, 0),
        };

        let content = oresp.choices.first()
            .map(|c| {
                c.message.content.clone()
                    .or_else(|| c.message.reasoning.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        Ok(Response {
            content,
            timing: Timing {
                ttft: total_dur,
                prefill_dur: total_dur,
                decode_dur: total_dur,  // 非 stream 模式：decode 时间 = 总时间（包含 prefill）
                total_dur,
                prompt_tokens,
                output_tokens,
                cached_tokens,
                tpot: if output_tokens > 1 { total_dur / output_tokens as u32 } else { Duration::ZERO },
                token_timings: vec![],
            },
        })
    }

    async fn send_stream(
        &self,
        req: Request,
        on_token: &mut (dyn FnMut(String, Duration) + Send),
    ) -> Result<Response> {
        use bytes::Buf;
        use futures_util::StreamExt;

        let start = Instant::now();
        let msgs = Self::prepare_messages(&req);

        let body = OpenAIRequest {
            model: &req.model,
            messages: &msgs,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
        };

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self.client.post(&url)
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Backend(format!("HTTP {status}: {text}")));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        let mut content = String::new();
        let mut token_count: usize = 0;
        let mut first_token_at: Option<Duration> = None;
        let mut last_token_at: Option<Duration> = None;
        let mut token_timings: Vec<Duration> = Vec::new();
        let mut server_prompt_tokens: usize = 0;
        let mut server_output_tokens: usize = 0;
        let mut server_cached_tokens: usize = 0;
        let mut usage_seen = false;
        let mut done = false; // set after finish_reason, break on next iteration

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.extend_from_slice(chunk.chunk());

            // Process complete SSE events (separated by \n\n or \r\n\r\n)
            loop {
                // Find event boundary: \n\n or \r\n\r\n or \r\n\n
                let event_end = buf.windows(2).position(|w| w == b"\n\n")
                    .or_else(|| buf.windows(4).position(|w| w == b"\r\n\r\n"))
                    .or_else(|| buf.windows(3).position(|w| w == b"\r\n\n"));

                let Some(pos) = event_end else {
                    break; // No complete event yet
                };

                // Determine the actual line ending length
                let end_len = if pos + 4 <= buf.len() && &buf[pos..pos + 4] == b"\r\n\r\n" {
                    4
                } else if pos + 3 <= buf.len() && &buf[pos..pos + 3] == b"\r\n\n" {
                    3
                } else {
                    2 // \n\n
                };

                // Extract event data (everything before the blank line)
                let event_bytes: Vec<u8> = buf.drain(..pos + end_len).collect();
                let event_str = String::from_utf8_lossy(&event_bytes);

                // Parse each line in the event
                for line in event_str.lines() {
                    let line = line.trim();
                    if line.is_empty() || !line.starts_with("data:") {
                        continue;
                    }

                    // Handle both "data: {...}" and "data:{...}" formats
                    let data = line[5..].trim_start();
                    if data == "[DONE]" {
                        break;
                    }

                    let chunk: OpenAIChunk = match serde_json::from_str(data) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    // Capture server usage (usually in final chunk with empty choices)
                    if let Some(usage) = &chunk.usage {
                        server_output_tokens = usage.completion_tokens;
                        server_prompt_tokens = usage.prompt_tokens;
                        server_cached_tokens = usage.prompt_tokens_details.as_ref().map(|d| d.cached_tokens).unwrap_or(0);
                        usage_seen = true;
                    }

                    // After finish_reason or seeing usage, we can break
                    if done || usage_seen {
                        break;
                    }

                    if chunk.choices.is_empty() {
                        if usage_seen {
                            break; // got usage, done
                        }
                        continue;
                    }

                    let delta_content = chunk.choices[0].delta.content.as_deref()
                        .or(chunk.choices[0].delta.reasoning.as_deref())
                        .unwrap_or("");

                    if delta_content.is_empty() {
                        if chunk.choices[0].finish_reason.is_some() {
                            // Signal that we should break after one more iteration
                            // (to capture usage if it comes in the next chunk)
                            done = true;
                            if usage_seen {
                                break;
                            }
                        }
                        continue;
                    }

                    let now = start.elapsed();
                    if token_count == 0 {
                        first_token_at = Some(now);
                    } else if let Some(last) = last_token_at {
                        token_timings.push(now - last);
                    }

                    content.push_str(delta_content);
                    token_count += 1;

                    let inter_token_dur = if token_count > 1 {
                        last_token_at.map(|l| now - l).unwrap_or(Duration::ZERO)
                    } else {
                        Duration::ZERO
                    };
                    last_token_at = Some(now);

                    on_token(delta_content.to_string(), inter_token_dur);
                } // end for line in event_str.lines()
            } // end loop (process complete events)
        } // end while let Some(chunk)

        let total_dur = start.elapsed();
        let ttft = first_token_at.unwrap_or(total_dur);
        let decode_dur = total_dur - ttft;
        let tpot = if server_output_tokens > 1 {
            decode_dur / (server_output_tokens - 1) as u32
        } else {
            Duration::ZERO
        };
        let output_tokens = if server_output_tokens > 0 { server_output_tokens } else { token_count };

        Ok(Response {
            content,
            timing: Timing {
                ttft,
                prefill_dur: ttft,
                decode_dur,
                total_dur,
                prompt_tokens: server_prompt_tokens,
                output_tokens,
                cached_tokens: server_cached_tokens,
                tpot,
                token_timings,
            },
        })
    }
}

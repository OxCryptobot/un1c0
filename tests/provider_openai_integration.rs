use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use un1c0::agentic::{built_in_registry, Action, Plan};
use un1c0::provider::{FinishReason, ModelProvider, ProviderError, ProviderRequest, TaskRisk};
use un1c0::provider_openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

fn request() -> ProviderRequest {
    ProviderRequest {
        request_id: "integration-1".into(),
        goal: "inspect safely".into(),
        context: vec![],
        schema_version: "plan.v1".into(),
        context_tokens: 100,
        max_output_tokens: 500,
        deadline_ms: 5_000,
        required_capabilities: Default::default(),
        risk: TaskRisk::Low,
        minimum_quality_score: 0,
    }
}

fn valid_plan_json() -> Value {
    serde_json::to_value(Plan {
        id: "plan-integration".into(),
        goal: "inspect safely".into(),
        actions: vec![Action {
            id: "echo".into(),
            tool: "echo".into(),
            input: json!({"message":"ok"}),
            depends_on: vec![],
            capabilities: vec![],
            timeout_ms: None,
        }],
        max_steps: 4,
        max_output_bytes: 1024,
    })
    .unwrap()
}

fn spawn_server(
    status: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let status = status.to_string();
    let headers = headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    let body = body.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_http_request(&mut stream);
        let mut response = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            status,
            body.len()
        );
        for (name, value) in headers {
            response.push_str(&format!("{}: {}\r\n", name, value));
        }
        response.push_str("\r\n");
        response.push_str(&body);
        stream.write_all(response.as_bytes()).unwrap();
        request
    });
    (address, handle)
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_body = None;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                bytes.extend_from_slice(&buffer[..read]);
                if expected_body.is_none() {
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let header = String::from_utf8_lossy(&bytes[..header_end]);
                        expected_body = header.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            (name.eq_ignore_ascii_case("content-length"))
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        });
                    }
                }
                if let (Some(header_end), Some(body_length)) = (
                    bytes.windows(4).position(|window| window == b"\r\n\r\n"),
                    expected_body,
                ) {
                    if bytes.len() >= header_end + 4 + body_length {
                        break;
                    }
                }
            }
            Err(error) => panic!("request read failed: {error}"),
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn provider_for(base_url: String) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig {
            base_url,
            api_key: Some("test-secret".into()),
            model_id: "test-model".into(),
            timeout: Duration::from_secs(2),
            ..Default::default()
        },
        &built_in_registry(),
    )
    .unwrap()
}

#[test]
fn sends_strict_schema_and_decodes_valid_plan() {
    let content = serde_json::to_string(&valid_plan_json()).unwrap();
    let body = json!({
        "id":"response-1",
        "choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":42,"completion_tokens":19}
    });
    let (base_url, server) = spawn_server("200 OK", &[], body);
    let provider = provider_for(base_url);
    let response = provider.complete(&request()).unwrap();
    let request_text = server.join().unwrap();
    let request_body = request_text.split("\r\n\r\n").nth(1).unwrap();
    let sent: Value = serde_json::from_str(request_body).unwrap();

    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.usage.input_tokens, 42);
    assert_eq!(response.usage.output_tokens, 19);
    assert_eq!(
        response.structured_output.unwrap()["id"],
        "plan-integration"
    );
    assert_eq!(sent["response_format"]["type"], "json_schema");
    assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
    assert_eq!(sent["response_format"]["json_schema"]["name"], "un1c0_plan");
    assert_eq!(sent["model"], "test-model");
    assert!(request_text
        .to_lowercase()
        .contains("authorization: bearer test-secret"));
}

#[test]
fn maps_refusal_without_treating_it_as_a_plan() {
    let body = json!({
        "choices":[{"message":{"role":"assistant","content":"","refusal":"unsafe request"},"finish_reason":"stop"}],
        "usage":{}
    });
    let (base_url, server) = spawn_server("200 OK", &[], body);
    let provider = provider_for(base_url);
    let response = provider.complete(&request()).unwrap();
    server.join().unwrap();
    assert_eq!(response.finish_reason, FinishReason::Refusal);
    assert_eq!(response.refusal.as_deref(), Some("unsafe request"));
    assert!(response.structured_output.is_none());
}

#[test]
fn maps_rate_limit_and_retry_after() {
    let (base_url, server) = spawn_server(
        "429 Too Many Requests",
        &[("Retry-After", "3")],
        json!({"error":{"message":"slow down"}}),
    );
    let provider = provider_for(base_url);
    let error = provider.complete(&request()).unwrap_err();
    server.join().unwrap();
    assert!(matches!(
        error,
        ProviderError::RateLimited {
            retry_after_ms: Some(3_000),
            ..
        }
    ));
}

#[test]
fn rejects_malformed_success_and_redacts_auth_failures() {
    let (base_url, server) = spawn_server("200 OK", &[], json!("not an envelope"));
    let provider = provider_for(base_url);
    let error = provider.complete(&request()).unwrap_err();
    server.join().unwrap();
    assert!(matches!(error, ProviderError::MalformedOutput { .. }));

    let (base_url, server) = spawn_server(
        "401 Unauthorized",
        &[],
        json!({"error":{"message":"Bearer test-secret invalid"}}),
    );
    let provider = provider_for(base_url);
    let error = provider.complete(&request()).unwrap_err();
    server.join().unwrap();
    let message = error.to_string();
    assert!(!message.contains("test-secret"));
    assert!(matches!(error, ProviderError::Configuration { .. }));
}

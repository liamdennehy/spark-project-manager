//! LLM completion API client — send board summaries to Spark/OpenAI-compatible endpoint.

use anyhow::{Context, Result};

/// Build the JSON payload for a chat completion request.
///
/// The structure mirrors the shell script's `completionAPIJSON` function:
/// - system prompt (from Spark-Instructions.md)
/// - system note: current date/time
/// - user message: the board summary JSON
pub fn completion_api_json(system_prompt: &str, now: &str, board_summary_json: &str) -> String {
    serde_json::json!({
        "model": "default",
        "messages": [
            {
                "role": "system",
                "content": system_prompt
            },
            {
                "role": "system",
                "content": format!("It is currently {}", now)
            },
            {
                "role": "user",
                "content": board_summary_json
            }
        ]
    })
    .to_string()
}

/// Send a completion request to the Spark API and return the raw response JSON.
pub async fn send_completion(
    spark_url: &str,
    api_key: Option<&str>,
    payload_json: &str,
) -> Result<String> {
    let client = reqwest::Client::new();

    let mut request = client
        .post(&format!("{}/v1/chat/completions", spark_url.trim_end_matches('/')))
        .header("Content-Type", "application/json")
        .body(payload_json.to_string());

    // Attach API key as Bearer token if provided
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .with_context(|| "Failed to send completion request to Spark API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "Spark API returned {}: {}\nPayload size: {} bytes",
            status,
            body,
            payload_json.len()
        );
    }

    let text = response
        .text()
        .await
        .with_context(|| "Failed to read Spark API response body")?;

    // Validate it's parseable JSON
    serde_json::from_str::<serde_json::Value>(&text)
        .with_context(|| "Spark API response was not valid JSON")?;

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_api_json_structure() {
        let payload = completion_api_json(
            "You are a project assistant.",
            "2025-01-15 10:30",
            r#"{"To-Do":[]}"#,
        );

        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(value["model"], "default");
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // First system message
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a project assistant.");

        // Time system message
        assert_eq!(messages[1]["role"], "system");
        assert_eq!(messages[1]["content"], "It is currently 2025-01-15 10:30");

        // User message — board summary
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], r#"{"To-Do":[]}"#);
    }

    #[test]
    fn test_completion_api_json_with_board_data() {
        let board = r#"{
  "To-Do": [{"id": 1, "title": "Task A", "description": "Do something", "overdue": null}],
  "Doing": [{"id": 2, "title": "Task B", "description": "In progress"}]
}"#;

        let payload = completion_api_json("Analyze this.", "2026-01-01 00:00", board);
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let user_msg = value["messages"][2]["content"].as_str().unwrap();
        // User content should be the raw board JSON (not double-encoded)
        assert!(user_msg.contains("Task A"));
        assert!(user_msg.contains("Task B"));
    }
}

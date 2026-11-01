//! NextCloud Deck API client — fetch board stacks and their cards.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a single Deck card (after ignored fields are stripped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: u64,
    pub title: String,
    pub description: String,
    #[serde(rename = "duedate")]
    pub duedate: Option<String>,
    #[serde(rename = "startdate")]
    pub startdate: Option<String>,
    #[serde(rename = "overdue")]
    pub overdue: Option<i64>,
    #[serde(rename = "assignedUsers")]
    pub assigned_users: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub done: Option<serde_json::Value>,
    /// Any remaining fields not explicitly mapped (e.g., labels).
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// A Deck stack summary (from the board listing, no cards).
#[derive(Debug, Clone, Deserialize)]
pub struct StackSummary {
    pub id: u64,
    pub title: String,
}

/// A Deck stack (column) with its cards.
/// Deserialized from /boards/{id}/stacks/{stackId} responses.
/// Extra fields from the API (boardId, ETag, etc.) are captured via flatten.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Fields used by serde deserialization, accessed via HashMap
pub struct Stack {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cards: Vec<Card>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Full board summary: a map from stack title to its cards.
#[derive(Debug, Clone, Serialize)]
pub struct BoardSummary(pub HashMap<String, Vec<Card>>);

impl BoardSummary {
    /// Serialize to pretty JSON string.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.0)?)
    }
}

/// Expected stack names that must exist for a valid board structure.
pub const EXPECTED_STACKS: &[&str] = &[
    "To-Do",
    "Doing",
    "Done",
    "Cancelled",
];

/// Fetch all stacks from the board and build a summary.
///
/// Validates that every expected stack exists; returns an error if any
/// are missing.
pub async fn get_board_summary(
    nc_url: &str,
    user: &str,
    password: &str,
    board_id: u64,
    ignored_fields_jq: &str,
) -> Result<BoardSummary> {
    let client = reqwest::Client::new();

    // Step 1: Fetch the board — response is a map with a "stacks" array
    let board_url = format!(
        "{}/apps/deck/api/v1/boards/{}",
        nc_url.trim_end_matches('/'),
        board_id
    );

    let board_response = client
        .get(&board_url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("Failed to fetch board from Deck API: {}", board_url))?;

    if !board_response.status().is_success() {
        let status = board_response.status();
        let body = board_response.text().await.unwrap_or_default();
        anyhow::bail!("Deck API returned {}: {}\nBoard ID: {}", status, body, board_id);
    }

    // Parse the board object to extract stack summaries
    let board_value: serde_json::Value = board_response
        .json()
        .await
        .with_context(|| "Failed to parse board JSON from Deck API")?;

    let stack_summaries: Vec<StackSummary> = board_value
        .get("stacks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| StackSummary::deserialize(v).ok())
                .collect()
        })
        .context("Board response has no 'stacks' array")?;

    // Step 2: Validate expected stacks exist
    let found_names: Vec<&str> = stack_summaries.iter().map(|s| s.title.as_str()).collect();
    for expected in EXPECTED_STACKS {
        if !found_names.contains(&expected) {
            anyhow::bail!(
                "Invalid board structure: expected stack '{}' not found.\n\
                 Found stacks: {:?}",
                expected,
                found_names
            );
        }
    }

    // Step 3: Fetch cards for each stack individually, stripping ignored fields
    let mut summary_map = HashMap::new();

    for summary in &stack_summaries {
        let stack_url = format!(
            "{}/apps/deck/api/v1/boards/{}/stacks/{}",
            nc_url.trim_end_matches('/'),
            board_id,
            summary.id
        );

        let stack_data = client
            .get(&stack_url)
            .basic_auth(user, Some(password))
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to fetch stack '{}' ({}) from Deck API",
                    summary.title, summary.id
                )
            })?;

        if !stack_data.status().is_success() {
            let status = stack_data.status();
            anyhow::bail!(
                "Deck API returned {} for stack '{}'",
                status,
                summary.title
            );
        }

        let stack_detail: Stack = stack_data
            .json()
            .await
            .with_context(|| format!("Failed to parse stack '{}' JSON", summary.title))?;

        // Strip ignored fields from each card using jq-style filter
        let cards: Vec<Card> = strip_ignored_fields(&stack_detail.cards, ignored_fields_jq)?;

        summary_map.insert(summary.title.clone(), cards);
    }

    Ok(BoardSummary(summary_map))
}

/// Strip ignored fields from a list of cards.
///
/// Parses each card as JSON, removes keys matching the jq filter paths,
/// then re-serializes. The ignored fields use jq-style dotted paths like
/// `.color`, `.assignedUsers[].id`, etc.
fn strip_ignored_fields(cards: &[Card], filter: &str) -> Result<Vec<Card>> {
    let mut result = Vec::new();

    for card in cards {
        let json = serde_json::to_value(card)?;
        let stripped = apply_jq_filter(&json, filter)?;
        let card_out: Card = serde_json::from_value(stripped)?;
        result.push(card_out);
    }

    Ok(result)
}

/// Apply a simplified jq `del(...)` filter to a JSON value.
///
/// Handles paths like `.color`, `.stackId`, `.assignedUsers[].id`,
/// `.assignedUsers[].participant.type`.
fn apply_jq_filter(value: &serde_json::Value, filter: &str) -> Result<serde_json::Value> {
    let inner = filter
        .strip_prefix("del(")
        .and_then(|s| s.strip_suffix(')'))
        .context("Filter must be in the form del(path1, path2, ...)")?;

    let paths: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();

    let mut result = value.clone();
    for path in paths {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        // Ensure path starts with .
        let path = path.strip_prefix('.').unwrap_or(path);
        result = remove_path(&result, path);
    }

    Ok(result)
}

/// Remove a field path from a JSON value.
///
/// Supports:
///   - `fieldName` → remove `fieldName` from object
///   - `field[].subField` → for each element in `field` array, remove `subField`
///   - `field[].nested[].leaf` → nested array traversal
///   - `field.nested.field` → dotted path through nested objects
fn remove_path(value: &serde_json::Value, path: &str) -> serde_json::Value {
    // Check for array traversal first
    if let Some(pos) = path.find("[]") {
        let array_field = &path[..pos];
        let remaining = path[pos + 2..].trim_start_matches('.');

        if let Some(arr) = value.get(array_field).and_then(|v| v.as_array()) {
            let new_arr: Vec<serde_json::Value> = arr
                .iter()
                .map(|item| remove_path(item, remaining))
                .collect();
            let mut map = value.as_object().cloned().unwrap_or_default();
            map.insert(array_field.to_string(), serde_json::Value::Array(new_arr));
            return serde_json::Value::Object(map);
        }
        return value.clone();
    }

    // Simple or dotted path: e.g. `type` or `participant.type`
    if let Some(dot_pos) = path.find('.') {
        let field = &path[..dot_pos];
        let remaining = &path[dot_pos + 1..];

        if let Some(obj) = value.get(field) {
            let new_val = remove_path(obj, remaining);
            let mut map = value.as_object().cloned().unwrap_or_default();
            map.insert(field.to_string(), new_val);
            return serde_json::Value::Object(map);
        }
        return value.clone();
    }

    // Simple field removal
    if let Some(obj) = value.as_object() {
        let mut map = obj.clone();
        map.remove(path);
        serde_json::Value::Object(map)
    } else {
        value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_simple_field() {
        let json = serde_json::json!({
            "id": 137,
            "title": "Clean apartment",
            "color": null,
            "stackId": 42
        });
        let result = remove_path(&json, "color");
        assert!(result.get("color").is_none());
        assert_eq!(result.get("id").unwrap().as_u64(), Some(137));
    }

    #[test]
    fn test_remove_array_nested_field() {
        let json = serde_json::json!({
            "assignedUsers": [
                { "id": 19, "cardId": 137, "type": 0, "participant": { "primaryKey": "user1", "type": 0 } },
                { "id": 18, "cardId": 140, "type": 0, "participant": { "primaryKey": "user2", "type": 0 } }
            ]
        });
        let result = remove_path(&json, "assignedUsers[].id");
        let arr = result.get("assignedUsers").and_then(|v| v.as_array()).unwrap();
        for item in arr {
            assert!(item.get("id").is_none());
        }
    }

    #[test]
    fn test_remove_nested_array_field() {
        let json = serde_json::json!({
            "assignedUsers": [
                {
                    "id": 19,
                    "cardId": 137,
                    "type": 0,
                    "participant": { "primaryKey": "user1", "uid": "user1", "type": 0 }
                }
            ]
        });
        let result = remove_path(&json, "assignedUsers[].participant.type");
        let arr = result.get("assignedUsers").and_then(|v| v.as_array()).unwrap();
        for item in arr {
            let participant = item.get("participant").and_then(|p| p.as_object()).unwrap();
            assert!(participant.get("type").is_none());
            assert!(participant.get("primaryKey").is_some());
        }
    }

    #[test]
    fn test_strip_ignored_fields() {
        let cards = vec![
            Card {
                id: 137,
                title: "Clean apartment".to_string(),
                description: "Find cleaner".to_string(),
                duedate: Some("2026-11-22T07:00:00+00:00".to_string()),
                startdate: None,
                overdue: Some(0),
                assigned_users: Some(vec![
                    serde_json::json!({ "id": 19, "cardId": 137, "type": 0 })
                ]),
                done: None,
                extra: std::collections::HashMap::new(),
            },
        ];

        let filter = "del(.color, .stackId, .type)";
        let stripped = strip_ignored_fields(&cards, filter).unwrap();
        assert_eq!(stripped.len(), 1);

        let json = serde_json::to_value(&stripped[0]).unwrap();
        assert!(json.get("color").is_none());
        assert!(json.get("stackId").is_none());
        assert!(json.get("type").is_none());
        assert_eq!(json.get("id").unwrap().as_u64(), Some(137));
        assert_eq!(json.get("title").unwrap().as_str(), Some("Clean apartment"));
    }

    #[test]
    fn test_apply_jq_filter_multiple_paths() {
        let json = serde_json::json!({
            "id": 1,
            "title": "test",
            "color": "red",
            "stackId": 42,
            "lastModified": 12345
        });
        let filter = "del(.color, .stackId, .lastModified)";
        let result = apply_jq_filter(&json, filter).unwrap();

        assert!(result.get("color").is_none());
        assert!(result.get("stackId").is_none());
        assert!(result.get("lastModified").is_none());
        assert_eq!(result.get("id").unwrap().as_u64(), Some(1));
        assert_eq!(result.get("title").unwrap().as_str(), Some("test"));
    }

    #[test]
    fn test_expected_stacks() {
        // Sanity: our validation targets are known stacks
        assert_eq!(EXPECTED_STACKS.len(), 4);
        assert!(EXPECTED_STACKS.contains(&"To-Do"));
        assert!(EXPECTED_STACKS.contains(&"Doing"));
        assert!(EXPECTED_STACKS.contains(&"Done"));
        assert!(EXPECTED_STACKS.contains(&"Cancelled"));
    }

    // ── Real data test fixtures ─────────────────────────────────────

    #[test]
    fn test_parse_real_stack_response() {
        // Anonymized Deck API response fixture
        let json = r#"[
          {
            "id": 137,
            "title": "Find cleaning person for apartment",
            "description": "Find a professional cleaner",
            "color": null,
            "stackId": 42,
            "type": "plain",
            "lastModified": 1792858217,
            "lastEditor": null,
            "createdAt": 1792831698,
            "labels": null,
            "assignedUsers": [
              {
                "id": 19,
                "participant": {
                  "primaryKey": "user1",
                  "uid": "user1",
                  "displayname": "User One",
                  "type": 0
                },
                "cardId": 137,
                "type": 0
              }
            ],
            "attachments": [],
            "attachmentCount": 0,
            "owner": "user",
            "order": 999,
            "archived": false,
            "done": null,
            "duedate": "2026-11-22T07:00:00+00:00",
            "startdate": null,
            "deletedAt": 0,
            "commentsUnread": 0,
            "commentsCount": 0,
            "dependentCards": null,
            "ETag": "b2feb3af00ad9c7881bd686bb162f1f8",
            "overdue": 0,
            "referenceData": null
          },
          {
            "id": 140,
            "title": "Apartment cleaning task",
            "description": "",
            "color": null,
            "stackId": 42,
            "type": "plain",
            "lastModified": 1792926202,
            "lastEditor": null,
            "createdAt": 1792833376,
            "labels": null,
            "assignedUsers": [
              {
                "id": 18,
                "participant": {
                  "primaryKey": "user2",
                  "uid": "user2",
                  "displayname": "User Two",
                  "type": 0
                },
                "cardId": 140,
                "type": 0
              }
            ],
            "attachments": [],
            "attachmentCount": 0,
            "owner": "user",
            "order": 999,
            "archived": false,
            "done": null,
            "duedate": "2026-12-20T07:00:00+00:00",
            "startdate": null,
            "deletedAt": 0,
            "commentsUnread": 0,
            "commentsCount": 0,
            "dependentCards": null,
            "ETag": "c0cd04e9bb8b8b1f54b755d2c639fa7f",
            "overdue": 0,
            "referenceData": null
          }
        ]"#;

        let cards: Vec<Card> = serde_json::from_str(json).unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].id, 137);
        assert_eq!(cards[0].title, "Find cleaning person for apartment");
        assert_eq!(cards[0].duedate, Some("2026-11-22T07:00:00+00:00".to_string()));
    }
}

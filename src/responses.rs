//! Building requests in the OpenAI "Responses" format, which the ChatGPT sign-in backend speaks.
//!
//! The rest of the program keeps its conversation in chat-completions shape; this module
//! translates it: system messages become `instructions`, and everything else becomes `input`
//! items (messages, function calls and their outputs).

use crate::provider::ChatMessage;
use crate::tools::ToolSet;
use serde_json::{Value, json};

/// The text of a message's content, ignoring images.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn user_content(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![json!({"type": "input_text", "text": text})],
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => Some(json!({
                    "type": "input_text",
                    "text": part.get("text").and_then(Value::as_str).unwrap_or_default()
                })),
                Some("image_url") => part
                    .pointer("/image_url/url")
                    .and_then(Value::as_str)
                    .map(|url| json!({"type": "input_image", "image_url": url})),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The request body for one streamed turn. `effort` is already the provider's own word
/// (`low`, `medium`, `high`, `xhigh`), or `None` for the model's default.
pub(crate) fn build_request(
    model: &str,
    messages: &[ChatMessage],
    tools: ToolSet,
    effort: Option<&str>,
) -> Value {
    let instructions = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| text_of(&message.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut input = Vec::new();
    for message in messages.iter().filter(|message| message.role != "system") {
        match message.role.as_str() {
            "user" => input.push(json!({
                "type": "message",
                "role": "user",
                "content": user_content(&message.content)
            })),
            "assistant" => {
                let text = text_of(&message.content);
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}]
                    }));
                }
                for call in message.tool_calls.iter().flatten() {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.get("id").and_then(Value::as_str).unwrap_or_default(),
                        "name": call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                        "arguments": call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                    }));
                }
            }
            "tool" => input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.clone().unwrap_or_default(),
                "output": text_of(&message.content)
            })),
            _ => {}
        }
    }
    let mut body = json!({
        "model": model,
        // The backend insists on instructions, so there is always something.
        "instructions": if instructions.is_empty() { "You are a helpful assistant." } else { instructions.as_str() },
        "input": input,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
    });
    if tools.any() {
        body["tools"] = Value::Array(
            tools
                .definitions()
                .into_iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                        "strict": false
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(false);
    }
    if let Some(effort) = effort {
        body["reasoning"] = json!({"effort": effort, "summary": "auto"});
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call_message() -> ChatMessage {
        ChatMessage::assistant_tool_calls(
            "looking".to_owned(),
            vec![json!({
                "id": "call_1",
                "type": "function",
                "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}
            })],
        )
    }

    #[test]
    fn a_conversation_becomes_instructions_and_input_items() {
        let messages = vec![
            ChatMessage::system("be brief".to_owned()),
            ChatMessage::user_with_images("hi".to_owned(), "hi".to_owned(), Vec::new()),
            tool_call_message(),
            ChatMessage::tool_result(
                "call_1".to_owned(),
                "read_file".to_owned(),
                "file text".to_owned(),
            ),
            ChatMessage::assistant("done".to_owned()),
        ];
        let body = build_request("gpt-5", &messages, ToolSet::None, None);
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        let input = body["input"].as_array().unwrap();
        let kinds = input
            .iter()
            .map(|item| {
                format!(
                    "{}:{}",
                    item["type"].as_str().unwrap(),
                    item["role"].as_str().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                "message:user",
                "message:assistant",
                "function_call:-",
                "function_call_output:-",
                "message:assistant"
            ]
        );
        assert_eq!(
            input[0]["content"][0],
            json!({"type": "input_text", "text": "hi"})
        );
        assert_eq!(
            input[1]["content"][0],
            json!({"type": "output_text", "text": "looking"})
        );
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["name"], "read_file");
        assert_eq!(input[2]["arguments"], "{\"path\":\"a.rs\"}");
        assert_eq!(input[3]["output"], "file text");
        assert!(body.get("tools").is_none() && body.get("reasoning").is_none());
    }

    #[test]
    fn images_and_text_parts_keep_their_order() {
        let mut message =
            ChatMessage::user_with_images("look".to_owned(), "look".to_owned(), Vec::new());
        message.content = json!([
            {"type": "text", "text": "what is this"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
        ]);
        let body = build_request("m", &[message], ToolSet::None, None);
        let content = &body["input"][0]["content"];
        assert_eq!(
            content[0],
            json!({"type": "input_text", "text": "what is this"})
        );
        assert_eq!(
            content[1],
            json!({"type": "input_image", "image_url": "data:image/png;base64,AAAA"})
        );
    }

    #[test]
    fn tools_are_offered_in_the_responses_shape_only_when_there_are_any() {
        let messages = vec![ChatMessage::user_with_images(
            "x".to_owned(),
            "x".to_owned(),
            Vec::new(),
        )];
        let none = build_request("m", &messages, ToolSet::None, None);
        assert!(none.get("tools").is_none() && none.get("tool_choice").is_none());
        let some = build_request("m", &messages, ToolSet::Explore, None);
        let tools = some["tools"].as_array().unwrap();
        assert_eq!(tools.len(), ToolSet::Explore.definitions().len());
        assert_eq!(tools[0]["type"], "function");
        assert!(tools[0]["name"].is_string() && tools[0]["parameters"].is_object());
        assert!(
            tools[0].get("function").is_none(),
            "flat, unlike chat completions"
        );
        assert_eq!(some["tool_choice"], "auto");
        assert_eq!(some["parallel_tool_calls"], false);
    }

    #[test]
    fn effort_becomes_a_reasoning_setting_and_instructions_are_never_empty() {
        let body = build_request("m", &[], ToolSet::None, Some("high"));
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(!body["instructions"].as_str().unwrap().is_empty());
        assert_eq!(body["input"], json!([]));
    }
}

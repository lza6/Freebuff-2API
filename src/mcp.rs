//! MCP（Model Context Protocol）只读服务：JSON-RPC 2.0 envelope
//!
//! 把网关能力以 MCP 工具暴露给外部 agent（Claude Code / Cursor 等），
//! 当前仅 3 个只读工具，零写操作：
//! - `list_models`   可用模型 ID 列表
//! - `list_accounts` 账号健康状态快照
//! - `usage_summary` 累计用量统计
//!
//! 本模块不依赖 `AppState`（避免循环依赖），通过调用方注入的
//! [`GatewaySnapshot`] 数据快照工作；HTTP 接线方负责从 AppState 组装快照
//! 并在 POST 路由中调用 [`handle_json`]。
//!
//! 协议要点（MCP 2024-11-05）：
//! - 请求：`{"jsonrpc":"2.0","id":N,"method":"...","params":{...}}`
//! - 成功：`{"jsonrpc":"2.0","id":N,"result":{...}}`
//! - 失败：`{"jsonrpc":"2.0","id":N,"error":{"code":N,"message":"..."}}`
//! - 无 `id` 字段 = notification，不应答（返回 `None`）
//! - `tools/call` 的工具级失败用 `isError:true` 的 tool result 表达，不是协议错误

use serde_json::{json, Value};

/// 支持的 MCP 协议版本
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// serverInfo.name
pub const SERVER_NAME: &str = "freebuff2api";

const ERR_PARSE: i64 = -32700;
const ERR_INVALID_REQUEST: i64 = -32600;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;

/// 工具执行时由调用方提供的数据快照（接线方从 AppState 组装）
#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewaySnapshot {
    /// 可用模型 ID 列表
    pub models: Vec<String>,
    /// 账号健康摘要
    pub accounts: Vec<AccountBrief>,
    /// 用量汇总（`usage.totals()` 原样透传）
    pub usage_totals: Value,
    /// 网关版本（建议 `env!("CARGO_PKG_VERSION")`）
    pub version: String,
    /// 进程已运行秒数
    pub uptime_sec: u64,
}

/// 账号健康摘要（从 `pool::AccountSnapshot` 映射）
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountBrief {
    pub name: String,
    pub healthy: bool,
    pub score: f64,
    /// 会话状态（`session::SessionSnapshot::status`，缺失时为 "unknown"）
    pub session_status: String,
}

/// MCP 工具定义（`tools/list` 的 result）
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "list_models",
                "description": "列出网关当前可用的模型 ID 列表（只读）",
                "inputSchema": { "type": "object", "properties": {}, "required": [] }
            },
            {
                "name": "list_accounts",
                "description": "列出上游账号健康状态：名称、是否可用、评分、会话状态（只读）",
                "inputSchema": { "type": "object", "properties": {}, "required": [] }
            },
            {
                "name": "usage_summary",
                "description": "返回网关累计用量统计（请求数、token 等，只读）",
                "inputSchema": { "type": "object", "properties": {}, "required": [] }
            }
        ]
    })
}

/// 处理一个 JSON-RPC 请求，返回 JSON-RPC 响应（`None` = notification，无需响应）
///
/// 支持方法：`initialize` / `tools/list` / `tools/call` / `ping`；
/// 未知方法返回 `-32601`；非法信封返回 `-32600`；`tools/call` 参数缺失返回 `-32602`。
pub fn handle_request(req: &Value, snapshot: &GatewaySnapshot) -> Option<Value> {
    let Some(obj) = req.as_object() else {
        return Some(error_response(
            &Value::Null,
            ERR_INVALID_REQUEST,
            "Invalid Request: 请求必须是 JSON 对象",
        ));
    };

    // 无 id 字段 = notification，按 JSON-RPC 2.0 不应答
    if !obj.contains_key("id") {
        return None;
    }
    let id = obj.get("id").cloned().unwrap_or(Value::Null);

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_response(
            &id,
            ERR_INVALID_REQUEST,
            "Invalid Request: jsonrpc 必须为 \"2.0\"",
        ));
    }
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Some(error_response(
            &id,
            ERR_INVALID_REQUEST,
            "Invalid Request: method 必须是字符串",
        ));
    };

    let outcome = match method {
        "initialize" => Ok(initialize_result(snapshot)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tool_definitions()),
        "tools/call" => tools_call(obj.get("params"), snapshot),
        _ => Err((ERR_METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    };

    Some(match outcome {
        Ok(result) => success_response(&id, result),
        Err((code, message)) => error_response(&id, code, &message),
    })
}

/// 便捷入口：处理原始 JSON 字符串；解析失败返回 `-32700`（id 为 null）
pub fn handle_json(raw: &str, snapshot: &GatewaySnapshot) -> Option<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(req) => handle_request(&req, snapshot).map(|resp| resp.to_string()),
        Err(_) => {
            Some(error_response(&Value::Null, ERR_PARSE, "Parse error: 非法 JSON").to_string())
        }
    }
}

/// `initialize` 结果：协议版本 + 能力 + 服务信息
fn initialize_result(snapshot: &GatewaySnapshot) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": snapshot.version }
    })
}

/// 分发 `tools/call`；仅参数信封错误走协议错误，未知工具走 isError tool result
fn tools_call(params: Option<&Value>, snapshot: &GatewaySnapshot) -> Result<Value, (i64, String)> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .ok_or((
            ERR_INVALID_PARAMS,
            "Invalid params: tools/call 需要字符串字段 \"name\"".to_string(),
        ))?;

    match name {
        "list_models" => Ok(tool_text(to_json(&snapshot.models), false)),
        "list_accounts" => Ok(tool_text(to_json(&snapshot.accounts), false)),
        "usage_summary" => Ok(tool_text(snapshot.usage_totals.clone(), false)),
        other => Ok(tool_text(
            json!({ "error": format!("未知工具: {other}") }),
            true,
        )),
    }
}

/// 构造 MCP tool result；文本统一为 pretty JSON 字符串
fn tool_text(payload: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|e| format!("{{\"error\":\"序列化失败: {e}\"}}"));
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

/// 序列化为 JSON 值（这些类型不会失败，降级为错误对象而非 panic）
fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| json!({ "error": e.to_string() }))
}

/// 成功响应信封
fn success_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// 错误响应信封
fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> GatewaySnapshot {
        GatewaySnapshot {
            models: vec!["claude-sonnet-5".into(), "gpt-5".into()],
            accounts: vec![
                AccountBrief {
                    name: "token-1".into(),
                    healthy: true,
                    score: 120.5,
                    session_status: "active".into(),
                },
                AccountBrief {
                    name: "token-2".into(),
                    healthy: false,
                    score: -999.0,
                    session_status: "cooldown".into(),
                },
            ],
            usage_totals: json!({ "requests": 42, "prompt_tokens": 1000 }),
            version: "0.3.0".into(),
            uptime_sec: 3600,
        }
    }

    fn call(method: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method })
    }

    fn call_tool(name: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": {} }
        })
    }

    #[test]
    fn initialize_returns_protocol_shape() {
        let resp = handle_request(&call("initialize"), &snapshot()).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(resp["result"]["serverInfo"]["version"], "0.3.0");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn tools_list_returns_three_tools_with_valid_schema() {
        let resp = handle_request(&call("tools/list"), &snapshot()).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["list_models", "list_accounts", "usage_summary"]);
        for t in tools {
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["inputSchema"]["properties"].is_object());
            assert!(t["inputSchema"]["required"].is_array());
        }
    }

    #[test]
    fn tools_call_list_models_returns_catalog() {
        let resp = handle_request(&call_tool("list_models"), &snapshot()).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let models: Vec<String> = serde_json::from_str(text).unwrap();
        assert_eq!(models, vec!["claude-sonnet-5", "gpt-5"]);
        assert_eq!(resp["result"]["content"][0]["type"], "text");
    }

    #[test]
    fn tools_call_list_accounts_returns_health() {
        let resp = handle_request(&call_tool("list_accounts"), &snapshot()).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let accounts: Value = serde_json::from_str(text).unwrap();
        assert_eq!(accounts[0]["name"], "token-1");
        assert_eq!(accounts[0]["healthy"], true);
        assert_eq!(accounts[1]["session_status"], "cooldown");
    }

    #[test]
    fn tools_call_usage_summary_returns_totals() {
        let resp = handle_request(&call_tool("usage_summary"), &snapshot()).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let totals: Value = serde_json::from_str(text).unwrap();
        assert_eq!(totals["requests"], 42);
        assert_eq!(totals["prompt_tokens"], 1000);
    }

    #[test]
    fn tools_call_unknown_tool_is_tool_error_not_protocol_error() {
        let resp = handle_request(&call_tool("delete_everything"), &snapshot()).unwrap();
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("未知工具"));
    }

    #[test]
    fn tools_call_without_name_returns_invalid_params() {
        let req = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {} });
        let resp = handle_request(&req, &snapshot()).unwrap();
        assert_eq!(resp["error"]["code"], ERR_INVALID_PARAMS);
        assert!(resp.get("result").is_none());
    }

    #[test]
    fn notification_without_id_returns_none() {
        let req = json!({ "jsonrpc": "2.0", "method": "tools/list" });
        assert!(handle_request(&req, &snapshot()).is_none());
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_request(&note, &snapshot()).is_none());
    }

    #[test]
    fn ping_returns_empty_result() {
        let resp = handle_request(&call("ping"), &snapshot()).unwrap();
        assert_eq!(resp["result"], json!({}));
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let resp = handle_request(&call("tools/delete"), &snapshot()).unwrap();
        assert_eq!(resp["error"]["code"], ERR_METHOD_NOT_FOUND);
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tools/delete"));
    }

    #[test]
    fn invalid_jsonrpc_version_returns_invalid_request() {
        let req = json!({ "jsonrpc": "1.0", "id": 3, "method": "ping" });
        let resp = handle_request(&req, &snapshot()).unwrap();
        assert_eq!(resp["error"]["code"], ERR_INVALID_REQUEST);
        assert_eq!(resp["id"], 3);
    }

    #[test]
    fn non_object_request_returns_invalid_request() {
        let resp = handle_request(&json!([1, 2, 3]), &snapshot()).unwrap();
        assert_eq!(resp["error"]["code"], ERR_INVALID_REQUEST);
        assert_eq!(resp["id"], Value::Null);
    }

    #[test]
    fn id_is_echoed_for_number_string_and_null() {
        for id in [json!(123), json!("abc"), json!(null)] {
            let req = json!({ "jsonrpc": "2.0", "id": id, "method": "ping" });
            let resp = handle_request(&req, &snapshot()).unwrap();
            assert_eq!(resp["id"], id);
        }
    }

    #[test]
    fn handle_json_parse_error_returns_32700() {
        let out = handle_json("{ not json", &snapshot()).unwrap();
        let resp: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(resp["error"]["code"], ERR_PARSE);
        assert_eq!(resp["id"], Value::Null);
    }

    #[test]
    fn handle_json_end_to_end_string() {
        let out = handle_json(
            r#"{"jsonrpc":"2.0","id":"req-9","method":"tools/call","params":{"name":"list_models"}}"#,
            &snapshot(),
        )
        .unwrap();
        let resp: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(resp["id"], "req-9");
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains('\n'), "文本应为 pretty JSON");
        assert!(text.contains("claude-sonnet-5"));
    }

    #[test]
    fn handle_json_notification_returns_none() {
        let out = handle_json(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &snapshot(),
        );
        assert!(out.is_none());
    }
}

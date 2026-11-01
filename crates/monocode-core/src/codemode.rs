//! Session-scoped MCP adapter for the separately packaged Pi codemode runtime.
//! This does not edit global provider configuration or expose provider-private tools.
use crate::{
    agent::AccessMode,
    provider::{Kind, Request},
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

fn script() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent()?;
        for path in [
            dir.join("../Resources/codemode/server.mjs"),
            dir.join("codemode/server.mjs"),
        ] {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/codemode/server.mjs");
    path.is_file().then_some(path)
}

pub fn mcp_server(request: &Request) -> Option<Value> {
    if !request.settings.shared_codemode
        || request.settings.text_only
        || matches!(request.kind, Kind::Pi | Kind::Omp)
        || crate::files::remote::is_remote(&request.cwd)
    {
        return None;
    }
    let node = crate::scm::resolve_gui_binary("node")?;
    let script = script()?;
    let full = request.settings.access == AccessMode::FullAccess && !request.settings.plan;
    Some(
        json!({"type":"stdio", "command":node, "args":[script,"--cwd",request.cwd,"--access",if full {"full"} else {"read-only"}]}),
    )
}

pub fn configure_codex(command: &mut Command, request: &Request) {
    if let Some(server) = mcp_server(request) {
        for key in ["command", "args"] {
            command.args([
                "-c",
                &format!("mcp_servers.monocode_codemode.{key}={}", server[key]),
            ]);
        }
        command.env("PATH", crate::scm::terminal_search_path());
        command.args([
            "-c",
            &format!(
                "mcp_servers.monocode_codemode.env_vars={}",
                json!(forwarded_environment())
            ),
        ]);
        command.args(["-c", "mcp_servers.monocode_codemode.tool_timeout_sec=90"]);
        command.args(["-c", "mcp_servers.monocode_codemode.startup_timeout_sec=30"]);
    }
}

pub fn acp_servers(request: &Request) -> Value {
    let mut servers = crate::visual_bridge::acp_servers(&request.settings.environment);
    if let Some(mut server) = mcp_server(request) {
        server.as_object_mut().unwrap().remove("type");
        server["name"] = "monocode_codemode".into();
        server["env"] = json!([{"name":"PATH","value":crate::scm::terminal_search_path()}]);
        servers.as_array_mut().unwrap().push(server);
    }
    servers
}

fn forwarded_environment() -> Vec<String> {
    fn collect(value: &Value, names: &mut Vec<String>) {
        match value {
            Value::String(value) => {
                if let Some(name) = value.strip_prefix("env:") {
                    if !name.is_empty()
                        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    {
                        names.push(name.into());
                    }
                }
            }
            Value::Object(map) => {
                for value in map.values() {
                    collect(value, names);
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(value, names);
                }
            }
            _ => {}
        }
    }
    let mut names = vec!["PATH".into()];
    let config = crate::scm::shell::dirs_home()
        .and_then(|home| {
            std::fs::read_to_string(PathBuf::from(home).join(".config/monocode/codemode.json")).ok()
        })
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if let Some(value) = config {
        collect(&value, &mut names);
    }
    names.sort();
    names.dedup();
    names
}

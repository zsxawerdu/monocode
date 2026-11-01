//! Muse's ACP 1.4 effort menu drops tiers without descriptions. Use MSP for
//! the complete catalog and durable effort selection; ACP still runs turns.
use super::*;

struct Host {
    wire: parked::Wire,
    sequence: u64,
}
impl Host {
    fn start(request: &Request, cancel: &AtomicBool) -> Result<Self, String> {
        let mut command = Command::new(&request.binary);
        crate::scm::shell::apply_gui_env(&mut command);
        command.envs(&request.settings.environment);
        request.settings.account.apply(Kind::Muse, &mut command)?;
        command
            .args([
                "serve",
                "--disable-write",
                "--disable-shell",
                "--disable-request-user-input",
            ])
            .current_dir(&request.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process::isolate(&mut command);
        let mut host = Self {
            wire: parked::Wire::spawn(&mut command, request)?,
            sequence: 0,
        };
        host.call("initialize", json!({"clientInfo":{"name":"monocode","version":env!("CARGO_PKG_VERSION")},"capabilities":{"userInputDialogs":false}}), cancel)?;
        write_json(
            &mut host.wire.stdin,
            json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        )?;
        Ok(host)
    }
    fn call(&mut self, method: &str, params: Value, cancel: &AtomicBool) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        write_json(
            &mut self.wire.stdin,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Stopped".into());
            }
            if Instant::now() >= deadline {
                return Err(format!("Muse {method} timed out"));
            }
            let bytes = match self.wire.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => result?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err(format!("Muse exited during {method}")),
            };
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Muse response: {e}"))?;
            if value["id"] != id {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!(
                    "Muse {method}: {}",
                    error["message"].as_str().unwrap_or("request failed")
                ));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| format!("Muse {method} returned no result"));
        }
    }
}

pub(crate) fn catalog(account: &crate::accounts::Profile) -> Result<Value, String> {
    let request = Request {
        kind: Kind::Muse,
        binary: resolve(Kind::Muse).ok_or_else(|| Kind::Muse.unavailable_hint())?,
        cwd: std::env::temp_dir(),
        prompt: String::new(),
        provider_id: None,
        model: None,
        settings: Settings {
            account: account.clone(),
            ..Default::default()
        },
    };
    let cancel = AtomicBool::new(false);
    Host::start(&request, &cancel)?.call("model/list", json!({}), &cancel)
}

pub(super) fn prepare_effort(request: &mut Request, cancel: &AtomicBool) -> Result<(), String> {
    let Some(effort) = request.settings.effort.clone() else {
        return Ok(());
    };
    if !matches!(
        effort.as_str(),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    ) {
        return Err(format!("Unsupported Muse reasoning effort: {effort}"));
    }
    if request.settings.plan
        || (request.settings.access == AccessMode::ReadOnly && !request.settings.text_only)
    {
        return Err("Muse Code does not support read-only or plan mode. Select Supervised to use Muse with approval prompts.".into());
    }
    // Release any older ACP process before acquiring Muse's single-writer lease.
    parked::discard_session(request);
    let mut host = Host::start(request, cancel)?;
    let session = if let Some(id) = &request.provider_id {
        host.call("session/resume", json!({"commandId":uuid::Uuid::now_v7().to_string(),"sessionId":id,"excludeItems":true}), cancel)?;
        id.clone()
    } else {
        let cwd = request.cwd.canonicalize().map_err(|e| e.to_string())?;
        let mut params =
            json!({"commandId":uuid::Uuid::now_v7().to_string(),"workspaceRoots":[cwd]});
        if let Some(model) = &request.model {
            params["modelId"] = model.clone().into();
        }
        let result = host.call("session/start", params, cancel)?;
        let id = result["session"]["sessionId"]
            .as_str()
            .ok_or("Muse returned no session id")?
            .to_owned();
        request.provider_id = Some(id.clone());
        id
    };
    let result = host.call("session/setReasoningEffort", json!({"commandId":uuid::Uuid::now_v7().to_string(),"sessionId":session,"reasoningEffort":effort}), cancel)?;
    if result["status"] != "accepted" {
        return Err("Muse did not accept the reasoning effort".into());
    }
    // The setter's acknowledgement is durable. Dropping the host reaps it and
    // releases the lease before ACP loads the same session for the prompt.
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn muse_effort_is_durable_before_acp_for_new_and_resumed_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("muse");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
 v=json.loads(line)
 with open('calls.jsonl','a') as f: f.write(json.dumps(v)+'\n')
 if 'id' not in v: continue
 method=v['method']
 result={'session':{'sessionId':'saved'}} if method=='session/start' else {'status':'accepted'}
 print(json.dumps({'jsonrpc':'2.0','id':v['id'],'result':result}),flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut request = Request {
            kind: Kind::Muse,
            binary,
            cwd: dir.path().to_owned(),
            prompt: "test".into(),
            provider_id: None,
            model: Some("wonder".into()),
            settings: Settings {
                effort: Some("medium".into()),
                ..Default::default()
            },
        };
        let cancel = AtomicBool::new(false);
        prepare_effort(&mut request, &cancel).unwrap();
        assert_eq!(request.provider_id.as_deref(), Some("saved"));
        request.settings.effort = Some("high".into());
        prepare_effort(&mut request, &cancel).unwrap();
        let calls: Vec<Value> = std::fs::read_to_string(dir.path().join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let methods: Vec<_> = calls
            .iter()
            .map(|v| v["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "session/start",
                "session/setReasoningEffort",
                "initialize",
                "initialized",
                "session/resume",
                "session/setReasoningEffort"
            ]
        );
        assert_eq!(calls[2]["params"]["modelId"], "wonder");
        assert_eq!(calls[3]["params"]["reasoningEffort"], "medium");
        assert_eq!(calls[7]["params"]["reasoningEffort"], "high");
        assert_eq!(calls[7]["params"]["sessionId"], "saved");
        let id = uuid::Uuid::parse_str(calls[7]["params"]["commandId"].as_str().unwrap()).unwrap();
        assert_eq!(id.get_version_num(), 7);
    }
}

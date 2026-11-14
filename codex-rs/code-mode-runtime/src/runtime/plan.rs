//! Data-only calls run through the linked Zi2c executor. No compiler, child
//! process, filesystem, or toolchain is needed. Rust retains tool authority.
use super::*;
use std::collections::VecDeque;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    calls: Vec<Call>,
    #[serde(default)]
    parallel: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    tool: String,
    #[serde(default = "empty_input")]
    input: JsonValue,
}

fn empty_input() -> JsonValue {
    json!({})
}

pub(super) async fn run(
    request: ExecuteRequest,
    events: RuntimeEventSender,
    mut commands: mpsc::UnboundedReceiver<RuntimeCommand>,
    mut controls: mpsc::UnboundedReceiver<RuntimeControlCommand>,
    pending_mode: PendingRuntimeMode,
) -> Result<(), String> {
    let plan: Plan = serde_json::from_str(&request.source)
        .map_err(|error| format!("invalid tool plan: {error}"))?;
    let mut executor = crate::native_dispatch::Executor::new(plan.calls.len(), plan.parallel)?;
    let catalog = crate::native_dispatch::Catalog::new(&request.enabled_tools);
    // Validate the whole plan before any effects. Freeform inputs remain exact
    // text; function inputs remain JSON values, without double serialization.
    let tools = plan
        .calls
        .iter()
        .map(|call| {
            let tool = catalog
                .resolve(&call.tool)
                .ok_or_else(|| format!("unknown or ambiguous enabled tool: {}", call.tool))?;
            if tool.kind == CodeModeToolKind::Freeform && !call.input.is_string() {
                return Err("a freeform tool plan input must be a string".into());
            }
            Ok(tool)
        })
        .collect::<Result<Vec<_>, String>>()?;
    events
        .send(RuntimeEvent::Started)
        .await
        .map_err(|_| "cell actor closed")?;
    let mut pending = HashMap::new();
    let mut ready = VecDeque::new();
    let mut paused = false;
    loop {
        if !paused {
            while let Some((index, result)) = ready.pop_front() {
                let text = serde_json::to_string(&result).map_err(|error| error.to_string())?;
                if text.len() > MAX_FRAME {
                    return Err("tool plan reply exceeds 64 MiB".into());
                }
                events
                    .send(RuntimeEvent::ContentItem(OutputItem::InputText { text }))
                    .await
                    .map_err(|_| "cell actor closed")?;
                let _ = index;
                executor.complete();
            }
            while let Some(index) = executor.next() {
                let tool = tools[index];
                let id = format!("{}_{}", request.tool_call_id, index + 1);
                pending.insert(id.clone(), index);
                events
                    .send(RuntimeEvent::ToolCall {
                        id,
                        name: tool.tool_name.clone(),
                        kind: tool.kind,
                        input: Some(plan.calls[index].input.clone()),
                    })
                    .await
                    .map_err(|_| "cell actor closed")?;
            }
            if executor.done() {
                events
                    .send(RuntimeEvent::Result {
                        stored_value_writes: HashMap::new(),
                        error_text: None,
                    })
                    .await
                    .map_err(|_| "cell actor closed")?;
                return Ok(());
            }
            if pending_mode == PendingRuntimeMode::PauseUntilResumed && !pending.is_empty() {
                paused = true;
                events
                    .send(RuntimeEvent::Pending)
                    .await
                    .map_err(|_| "cell actor closed")?;
            }
        }
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(RuntimeCommand::ToolResponse { id, result }) => {
                    if let Some(index) = pending.remove(&id) {
                        ready.push_back((index, json!({"call":index,"tool":plan.calls[index].tool,"ok":true,"value":result})));
                    }
                }
                Some(RuntimeCommand::ToolError { id, error_text }) => {
                    if let Some(index) = pending.remove(&id) {
                        ready.push_back((index, json!({"call":index,"tool":plan.calls[index].tool,"ok":false,"error":error_text})));
                    }
                }
                Some(RuntimeCommand::ObservePendingFrontier) => {
                    if !paused && (!pending.is_empty() || !ready.is_empty()) {
                        paused = true;
                        events.send(RuntimeEvent::Pending).await.map_err(|_| "cell actor closed")?;
                    }
                }
                Some(RuntimeCommand::Terminate) | None => return Ok(()),
            },
            control = controls.recv() => match control {
                Some(RuntimeControlCommand::Continue | RuntimeControlCommand::Resume) => paused = false,
                Some(RuntimeControlCommand::Terminate) | None => return Ok(()),
            },
        }
    }
}

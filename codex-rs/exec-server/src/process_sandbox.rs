use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::process_telemetry::ProcessTelemetry;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_network_proxy::CUSTOM_CA_ENV_KEYS;
use codex_network_proxy::CredentialMaskingFailureMode;
use codex_network_proxy::ManagedNetworkSandboxContext;
use codex_network_proxy::ManagedProxyRouting;
use codex_network_proxy::NetworkPolicyAuditObserver;
use codex_network_proxy::NetworkPolicyDecider;
use codex_network_proxy::NetworkProxy;
use codex_network_proxy::NetworkProxyHandle;
use codex_network_proxy::NetworkProxyState;
use codex_network_proxy::RemoteCredentialBrokerCache;
use codex_network_proxy::is_managed_mitm_ca_trust_bundle_path;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxManager;
use codex_sandboxing::SandboxTransformRequest;
use codex_sandboxing::SandboxType;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

use crate::CODEX_ARG0_EXEC_HELPER_ARG1;
use crate::ExecServerRuntimeOptions;
use crate::process_telemetry::trace_process_id;
use crate::protocol::ExecParams;
use crate::rpc::internal_error;
use crate::rpc::invalid_params;
use codex_sandboxing::SandboxSelection;

const CREDENTIAL_MASKING_FAILURE_MODE: CredentialMaskingFailureMode =
    CredentialMaskingFailureMode::Warn;

pub(crate) struct PreparedExecRequest {
    pub(crate) command: Vec<String>,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) env: HashMap<String, String>,
    pub(crate) arg0: Option<String>,
    pub(crate) sandbox: SandboxType,
    pub(crate) network_proxy_handle: Option<NetworkProxyHandle>,
}

impl PreparedExecRequest {
    pub(crate) fn credential_snapshot_proxy(&self, params: &ExecParams) -> Option<&NetworkProxy> {
        let launch = params.network_proxy.as_ref()?;
        if !launch.proxy.credential_masking_enabled() || !supports_credential_snapshot(params) {
            return None;
        }
        self.network_proxy_handle.as_ref()?.proxy()
    }
}

fn supports_credential_snapshot(params: &ExecParams) -> bool {
    params.arg0.is_none()
        && params.shell_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.shell.name == "zsh"
                && matches!(params.argv.as_slice(), [program, flag, _]
                    if program == &snapshot.shell.path
                        && flag == "-lc"
                        && Path::new(program).file_name().is_some_and(|name| name == "zsh"))
        })
}

#[tracing::instrument(
    name = "codex.exec_server.process_prepare_sandbox",
    skip_all,
    fields(process.id = trace_process_id(params.process_id.as_str())),
)]
pub(crate) async fn prepare_exec_request_with_telemetry(
    params: &ExecParams,
    env: HashMap<String, String>,
    runtime_paths: Option<&ExecServerRuntimeOptions>,
    network_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
    network_policy_audit_observer: Option<NetworkPolicyAuditObserver>,
    credential_brokers: &RemoteCredentialBrokerCache,
    telemetry: &ProcessTelemetry,
) -> Result<PreparedExecRequest, JSONRPCErrorError> {
    let (env, managed_network, network_proxy_handle) = prepare_managed_network(
        params,
        credential_brokers,
        runtime_paths.is_some_and(|paths| paths.proxy_private_ips_via_upstream),
        env,
        network_policy_decider,
        network_policy_audit_observer,
        telemetry,
    )
    .await?;
    let Some(sandbox_context) = params.sandbox.as_ref() else {
        return Ok(PreparedExecRequest {
            command: params.argv.clone(),
            cwd: native_path(&params.cwd, "cwd")?,
            env,
            arg0: params.arg0.clone(),
            sandbox: SandboxType::None,
            network_proxy_handle,
        });
    };
    let runtime_paths = runtime_paths
        .ok_or_else(|| invalid_params("sandbox runtime paths are not configured".to_string()))?;
    let mut runtime_readable_paths = Vec::new();
    if cfg!(target_os = "linux") || (params.arg0.is_some()) {
        runtime_readable_paths.push(runtime_paths.codex_self_exe.clone());
    }
    if cfg!(target_os = "linux") {
        runtime_readable_paths.extend(runtime_paths.codex_linux_sandbox_exe.iter().cloned());
    }
    let managed_mitm_ca_trust_bundle_path = managed_network.as_ref().and_then(|_| {
        CUSTOM_CA_ENV_KEYS.iter().find_map(|key| {
            let path = env.get(*key)?;
            if !is_managed_mitm_ca_trust_bundle_path(path) {
                return None;
            }
            AbsolutePathBuf::from_absolute_path(path).ok()
        })
    });
    runtime_readable_paths.extend(managed_mitm_ca_trust_bundle_path);
    let sandbox_manager = SandboxManager::new()
        .with_linux_sandbox_pid_namespace(runtime_paths.linux_sandbox_pid_namespace);
    #[cfg(target_os = "macos")]
    let sandbox_manager = sandbox_manager
        .with_allowed_symlinked_codex_home(runtime_paths.allowed_symlinked_codex_home.clone());
    let selection = SandboxSelection::RequirePlatform;
    let (program, args) = params
        .argv
        .split_first()
        .ok_or_else(|| invalid_params("argv must not be empty".to_string()))?;
    let (program, args) = params.arg0.as_ref().map_or_else(
        || (program.into(), args.to_vec()),
        |arg0| {
            let mut helper_args = Vec::with_capacity(params.argv.len() + 2);
            helper_args.push(CODEX_ARG0_EXEC_HELPER_ARG1.to_string());
            helper_args.push(arg0.clone());
            helper_args.extend(params.argv.iter().cloned());
            (
                runtime_paths
                    .codex_self_exe
                    .as_path()
                    .as_os_str()
                    .to_owned(),
                helper_args,
            )
        },
    );
    let mut request = sandbox_manager
        .transform(SandboxTransformRequest {
            command: SandboxCommand {
                program,
                args,
                cwd: params.cwd.clone(),
                env,
                managed_network,
                additional_permissions: None,
            },
            permissions: &sandbox_context.permissions,
            workspace_roots: Some(&sandbox_context.workspace_roots),
            runtime_readable_paths: &runtime_readable_paths,
            selection,
            enforce_managed_network: params.enforce_managed_network,
            environment_id: None,
            network: None,
            sandbox_policy_cwd: &sandbox_context.cwd,
            sandbox_exe: runtime_paths.codex_linux_sandbox_exe.as_deref(),
            use_legacy_landlock: sandbox_context.use_legacy_landlock,
        })
        .map_err(|err| match err {
            codex_sandboxing::SandboxTransformError::SandboxUnavailable => {
                invalid_params(err.to_string())
            }
            err => invalid_params(format!("failed to prepare process sandbox: {err}")),
        })?
        .into_command_request();
    request.sandbox_override = sandbox_context.sandbox_override;
    crate::run_integrity_checks(&request).await;
    Ok(PreparedExecRequest {
        command: request.command,
        cwd: native_path(&request.cwd, "cwd")?,
        env: request.env,
        arg0: request.arg0,
        sandbox: request.sandbox,
        network_proxy_handle,
    })
}

async fn prepare_managed_network(
    params: &ExecParams,
    credential_brokers: &RemoteCredentialBrokerCache,
    proxy_private_ips_via_upstream: bool,
    env: HashMap<String, String>,
    network_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
    network_policy_audit_observer: Option<NetworkPolicyAuditObserver>,
    telemetry: &ProcessTelemetry,
) -> Result<
    (
        HashMap<String, String>,
        Option<ManagedNetworkSandboxContext>,
        Option<NetworkProxyHandle>,
    ),
    JSONRPCErrorError,
> {
    let Some(launch) = &params.network_proxy else {
        return Ok((env, params.managed_network.clone(), None));
    };
    let routing = ManagedProxyRouting::SharedIngress;
    let mut state = NetworkProxyState::from_remote_launch_config(
        launch.clone(),
        codex_utils_path_uri::Platform::native(),
        credential_brokers,
        &env,
    )
    .map_err(|err| invalid_params(format!("invalid network proxy config: {err}")))?;
    let credential_masking = state
        .current_cfg()
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to read executor network proxy config: {err}"
            ))
        })?
        .credential_broker;
    state.set_proxy_private_ips_via_upstream(proxy_private_ips_via_upstream);
    if let Some(observer) = network_policy_audit_observer {
        state.set_policy_audit_observer(observer);
    }
    if let Some(launch_context) = &telemetry.launch_context {
        state.set_launch_span_context(launch_context.clone());
    }
    state.set_process_log_metadata(codex_network_proxy::NetworkProxyProcessLogMetadata {
        thread_id: telemetry.thread_id.clone(),
        tool_call_id: telemetry.tool_call_id.clone(),
        executor_identity: telemetry
            .executor_registration
            .as_ref()
            .map(|registration| codex_network_proxy::ExecutorLogIdentity {
                environment_id: registration.environment_id.clone(),
                registration_id: registration.executor_registration_id.clone(),
            }),
    });
    let mut builder = NetworkProxy::builder()
        .state(Arc::new(state))
        .managed_proxy_routing(routing);
    if let Some(network_policy_decider) = network_policy_decider {
        builder = builder.policy_decider_arc(network_policy_decider);
    }
    let proxy = builder
        .build()
        .await
        .map_err(|err| internal_error(format!("failed to build executor network proxy: {err}")))?;
    let handle = proxy
        .run()
        .await
        .map_err(|err| internal_error(format!("failed to start executor network proxy: {err}")))?;
    let supported_shell = matches!(params.argv.as_slice(), [program, flag, _]
        if Path::new(program).file_name().is_some_and(|name| name == "bash") && flag == "-c")
        && !["BASH_ENV", "ENV", "SSH_CLIENT", "SSH2_CLIENT"]
            .iter()
            .any(|key| env.contains_key(*key));
    let mut prepared = proxy
        .prepare_for_optional_environment(env, /*environment_id*/ None)
        .map_err(|err| {
            internal_error(format!("failed to prepare executor network proxy: {err}"))
        })?;
    if credential_masking {
        if supports_credential_snapshot(params) {
            // Requested snapshots trust startup profiles before masking their captured state.
            proxy.restore_and_disable_brokered_credentials(&mut prepared.env, &mut []);
        } else if supported_shell && params.arg0.is_none() && params.shell_snapshot.is_none() {
            proxy
                .handle_unmasked_credentials(&mut prepared.env, CREDENTIAL_MASKING_FAILURE_MODE)
                .map_err(|err| invalid_params(err.to_string()))?;
        } else {
            tracing::warn!(
                "Credential masking is unavailable for this shell startup; preserving shell profiles and snapshots and running with unmasked credentials."
            );
            // Restore this child's environment without disabling translation of placeholders
            // issued to earlier commands in the same environment.
            proxy.restore_and_disable_brokered_credentials(&mut prepared.env, &mut []);
        }
    }
    Ok((prepared.env, Some(prepared.sandbox_context), Some(handle)))
}

fn native_path(path: &PathUri, label: &str) -> Result<AbsolutePathBuf, JSONRPCErrorError> {
    path.to_abs_path().map_err(|err| {
        invalid_params(format!(
            "{label} URI `{path}` is not valid on this exec-server host: {err}"
        ))
    })
}

#[cfg(test)]
#[path = "process_sandbox_tests.rs"]
mod tests;

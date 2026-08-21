//! Minimal, bounded client for Grok Build's `x.ai/billing` ACP extension.
//!
//! The child is invoked directly (never through a shell), receives no prompt
//! or session, and is killed after the one billing response. stdout is treated
//! as untrusted framed JSON: both line size and message count are bounded.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::{AppError, Result};

use super::types::BillingResponse;

const ACP_TIMEOUT: Duration = Duration::from_secs(20);
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RPC_LINE_BYTES: usize = 1024 * 1024;
const MAX_RPC_MESSAGES: usize = 32;

const INITIALIZE_ID: u64 = 1;
const BILLING_ID: u64 = 2;

pub async fn fetch_billing(grok_binary: &Path) -> Result<BillingResponse> {
    tokio::time::timeout(ACP_TIMEOUT, fetch_billing_inner(grok_binary))
        .await
        .map_err(|_| AppError::Transport("Grok Build ACP billing request timed out".into()))?
}

async fn fetch_billing_inner(grok_binary: &Path) -> Result<BillingResponse> {
    let mut command = Command::new(grok_binary);
    command
        .args(["agent", "--no-leader", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Diagnostics can contain local paths or provider details. ACP errors
        // are returned on stdout, so discard stderr instead of persisting it.
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&["XAI_API_KEY", "GROK_API_KEY"]) {
        command.env_remove(var);
    }

    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::Credentials(
                "official Grok Build CLI not found; install it or set [supergrok] grok_binary"
                    .into(),
            )
        } else {
            AppError::Other("failed to start the configured Grok Build ACP process".into())
        }
    })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::Other("Grok Build ACP stdin was unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("Grok Build ACP stdout was unavailable".into()))?;

    let result = run_protocol(stdout, stdin).await;

    // The same "not installed" that `spawn` reports on macOS arrives on Linux as
    // a child that exited 127: `posix_spawn` cannot fail the parent's call, so
    // the exec failure surfaces in the child's status instead. Without this the
    // friendly message above never fires on Linux and the user is told the
    // protocol failed, rather than that the binary is missing.
    let missing = matches!(child.try_wait(), Ok(Some(status)) if status.code() == Some(127));
    if missing {
        let _ = child.start_kill();
        return Err(AppError::Credentials(
            "official Grok Build CLI not found; install it or set [supergrok] grok_binary".into(),
        ));
    }

    // `grok agent stdio` is long-lived. End it immediately after this one
    // extension response; kill_on_drop is the final backstop on every error.
    let _ = child.start_kill();
    let _ = tokio::time::timeout(CHILD_EXIT_TIMEOUT, child.wait()).await;
    result
}

async fn run_protocol<R, W>(reader: R, mut writer: W) -> Result<BillingResponse>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);

    write_request(
        &mut writer,
        INITIALIZE_ID,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {},
                "terminal": false
            },
            "_meta": {
                "startupHints": {
                    "nonInteractive": true,
                    "skipGitStatus": true,
                    "skipProjectLayout": true
                },
                "clientType": "ai-usagebar",
                "clientVersion": env!("CARGO_PKG_VERSION")
            }
        }),
    )
    .await?;
    read_result(&mut reader, INITIALIZE_ID, RpcStage::Initialize).await?;

    write_request(&mut writer, BILLING_ID, "x.ai/billing", json!({})).await?;
    let billing = read_result(&mut reader, BILLING_ID, RpcStage::Billing).await?;
    serde_json::from_value(billing).map_err(|_| {
        // serde's detailed type errors can include the rejected field value.
        // The ACP process is untrusted input, so never persist or display it.
        AppError::Schema("Grok Build billing response does not match the expected schema".into())
    })
}

async fn write_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: u64,
    method: &str,
    params: Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum RpcStage {
    Initialize,
    Billing,
}

async fn read_result<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    expected_id: u64,
    stage: RpcStage,
) -> Result<Value> {
    for _ in 0..MAX_RPC_MESSAGES {
        let line = read_line_bounded(reader).await?;
        let message: Value = serde_json::from_slice(&line).map_err(|_| {
            AppError::Schema("configured grok binary did not emit valid Grok Build ACP JSON".into())
        })?;

        if message.get("id").and_then(Value::as_u64) != Some(expected_id) {
            // Initialization can emit bounded notifications. Ignore only
            // well-formed messages with another/no id.
            continue;
        }
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(AppError::Schema(
                "Grok Build ACP response has an invalid protocol version".into(),
            ));
        }
        if let Some(error) = message.get("error") {
            return Err(map_rpc_error(error, stage));
        }
        return message.get("result").cloned().ok_or_else(|| {
            AppError::Schema("Grok Build ACP response has neither result nor error".into())
        });
    }

    Err(AppError::Schema(
        "Grok Build ACP emitted too many messages before the requested response".into(),
    ))
}

fn map_rpc_error(error: &Value, stage: RpcStage) -> AppError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    match (stage, code) {
        (RpcStage::Initialize, _) | (_, -32601) => AppError::Credentials(
            "configured grok binary does not support the Grok Build x.ai/billing ACP method; install or select the official current CLI"
                .into(),
        ),
        (RpcStage::Billing, _) => AppError::Credentials(
            "Grok Build could not return billing data; run `grok login` and verify the selected account"
                .into(),
        ),
    }
}

async fn read_line_bounded<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(AppError::Other(
                "configured grok binary closed before returning a Grok Build ACP response".into(),
            ));
        }

        let newline = available.iter().position(|b| *b == b'\n');
        let take = newline.map_or(available.len(), |i| i + 1);
        if line.len().saturating_add(take) > MAX_RPC_LINE_BYTES {
            return Err(AppError::Schema(
                "Grok Build ACP response exceeded the size limit".into(),
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);

        if newline.is_some() {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if line.is_empty() {
                return Err(AppError::Schema(
                    "Grok Build ACP emitted an empty response line".into(),
                ));
            }
            return Ok(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex, split};

    #[tokio::test]
    async fn protocol_sends_initialize_then_the_billing_extension() {
        let (client, server) = duplex(16 * 1024);
        let (client_read, client_write) = split(client);
        let (server_read, mut server_write) = split(server);
        let mut server_read = BufReader::new(server_read);

        let fake_server = async move {
            let mut request = String::new();
            server_read.read_line(&mut request).await.unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "initialize");
            assert_eq!(request["params"]["protocolVersion"], 1);
            assert_eq!(request["params"]["clientCapabilities"]["terminal"], false);
            assert_eq!(
                request["params"]["_meta"]["startupHints"]["nonInteractive"],
                true
            );
            server_write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
                .await
                .unwrap();

            let mut request = String::new();
            server_read.read_line(&mut request).await.unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "x.ai/billing");
            assert_eq!(request["params"], json!({}));
            server_write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"config\":{\"creditUsagePercent\":42.5,\"currentPeriod\":{\"type\":\"USAGE_PERIOD_TYPE_WEEKLY\",\"end\":\"2026-08-10T00:00:00Z\"}},\"subscription_tier\":\"SuperGrok Heavy\"}}\n")
                .await
                .unwrap();
        };

        let (billing, ()) = tokio::join!(run_protocol(client_read, client_write), fake_server);
        let billing = billing.unwrap();
        assert_eq!(
            billing.subscription_tier.as_deref(),
            Some("SuperGrok Heavy")
        );
        assert_eq!(billing.config.unwrap().credit_usage_percent, Some(42.5));
    }

    #[tokio::test]
    async fn protocol_rejects_an_oversized_response_without_unbounded_allocation() {
        let (client, mut server) = duplex(64 * 1024);
        let writer = async move {
            let oversized = vec![b'x'; MAX_RPC_LINE_BYTES + 1];
            server.write_all(&oversized).await.unwrap();
        };
        let mut reader = BufReader::new(client);
        let (result, ()) = tokio::join!(read_line_bounded(&mut reader), writer);
        assert!(matches!(result, Err(AppError::Schema(_))));
    }

    #[test]
    fn rpc_errors_do_not_echo_provider_data_or_tokens() {
        let error = json!({
            "code": -32603,
            "message": "Internal error",
            "data": "token=secret person@example.test"
        });
        let rendered = map_rpc_error(&error, RpcStage::Billing).to_string();
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("person@example.test"));
        assert!(rendered.contains("grok login"));
    }

    #[tokio::test]
    async fn malformed_billing_values_are_not_echoed() {
        let (client, server) = duplex(16 * 1024);
        let (client_read, client_write) = split(client);
        let (server_read, mut server_write) = split(server);
        let mut server_read = BufReader::new(server_read);

        let fake_server = async move {
            for response in [
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n".as_slice(),
                b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"config\":{\"creditUsagePercent\":\"secret-token-value\"}}}\n".as_slice(),
            ] {
                let mut request = String::new();
                server_read.read_line(&mut request).await.unwrap();
                server_write.write_all(response).await.unwrap();
            }
        };

        let (result, ()) = tokio::join!(run_protocol(client_read, client_write), fake_server);
        let rendered = result.unwrap_err().to_string();
        assert!(rendered.contains("expected schema"));
        assert!(!rendered.contains("secret-token-value"));
    }

    #[tokio::test]
    async fn missing_binary_has_a_clear_non_secret_error() {
        let td = tempfile::TempDir::new().unwrap();
        let error = fetch_billing(&td.path().join("definitely-not-installed"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("official Grok Build CLI not found"));
        assert!(!error.contains(&td.path().display().to_string()));
    }
}

use crate::config::HardwareMode;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::process::Stdio;
use tokio::{
    fs as async_fs,
    process::Command,
    time::{sleep, Duration},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterStatus {
    pub mode: String,
    pub live_apply_enabled: bool,
    pub tc_available: bool,
    pub nft_available: bool,
    pub dnsmasq_available: bool,
    pub opennds_available: bool,
    pub cake_available: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeRequest {
    pub interface: String,
    pub wan_interface: Option<String>,
    pub download_mbps: u32,
    pub upload_mbps: u32,
    pub dry_run: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterPlan {
    pub accepted: bool,
    pub applied: bool,
    pub commands: Vec<Vec<String>>,
    pub message: String,
}

pub async fn status(mode: &HardwareMode) -> RouterStatus {
    let live_apply_enabled = std::env::var("CHASSELFI_LIVE_ROUTER")
        .or_else(|_| std::env::var("BANTAY_LIVE_ROUTER"))
        .as_deref()
        == Ok("1");
    if mode != &HardwareMode::Linux {
        return RouterStatus {
            mode: "simulated".into(),
            live_apply_enabled,
            tc_available: false,
            nft_available: false,
            dnsmasq_available: false,
            opennds_available: false,
            cake_available: false,
            message: "Safe simulation mode. No host networking commands will run.".into(),
        };
    }
    let tc_available = command_available("tc").await;
    let cake_available = if tc_available {
        Command::new("sh")
            .args([
                "-c",
                "modinfo sch_cake >/dev/null 2>&1 || grep -qw cake /proc/modules",
            ])
            .status()
            .await
            .is_ok_and(|status| status.success())
    } else {
        false
    };
    RouterStatus {
        mode: "linux".into(),
        live_apply_enabled,
        tc_available,
        nft_available: command_available("nft").await,
        dnsmasq_available: command_available("dnsmasq").await,
        opennds_available: command_available("ndsctl").await,
        cake_available,
        message: if live_apply_enabled {
            "Live router commands are enabled by CHASSELFI_LIVE_ROUTER=1.".into()
        } else {
            "Dry-run only. Set CHASSELFI_LIVE_ROUTER=1 after reviewing the target interface.".into()
        },
    }
}

pub async fn apply(mode: &HardwareMode, request: ShapeRequest) -> Result<RouterPlan, String> {
    validate_interface(&request.interface)?;
    if let Some(interface) = request.wan_interface.as_deref() {
        validate_interface(interface)?;
        if interface == request.interface {
            return Err("LAN and WAN shaping interfaces must be different".into());
        }
    }
    if !(1..=10_000).contains(&request.download_mbps)
        || !(1..=10_000).contains(&request.upload_mbps)
    {
        return Err("Bandwidth must be between 1 and 10,000 Mbps".into());
    }
    // A root qdisc controls egress on one interface. Upload shaping normally
    // needs a separate WAN egress interface or an IFB redirect, so do not
    // issue two conflicting `root` replacements on the same device.
    let mut commands = vec![vec![
        "tc".into(),
        "qdisc".into(),
        "replace".into(),
        "dev".into(),
        request.interface.clone(),
        "root".into(),
        "cake".into(),
        "bandwidth".into(),
        format!("{}Mbit", request.download_mbps),
    ]];
    if let Some(wan_interface) = request.wan_interface.as_ref() {
        commands.push(vec![
            "tc".into(),
            "qdisc".into(),
            "replace".into(),
            "dev".into(),
            wan_interface.clone(),
            "root".into(),
            "cake".into(),
            "bandwidth".into(),
            format!("{}Mbit", request.upload_mbps),
        ]);
    }
    let dry_run = request.dry_run.unwrap_or(true)
        || mode != &HardwareMode::Linux
        || std::env::var("CHASSELFI_LIVE_ROUTER")
            .or_else(|_| std::env::var("BANTAY_LIVE_ROUTER"))
            .as_deref()
            != Ok("1");
    if dry_run {
        return Ok(RouterPlan {
            accepted: true,
            applied: false,
            commands,
            message: format!(
                "Plan validated but not applied (dry-run). LAN egress is {} Mbps on {}{}.",
                request.download_mbps,
                request.interface,
                request
                    .wan_interface
                    .as_ref()
                    .map(|wan| format!("; WAN egress is {} Mbps on {wan}", request.upload_mbps))
                    .unwrap_or_else(|| "; supply wanInterface to shape upstream egress".into())
            ),
        });
    }
    let helper_message = apply_via_privileged_helper(&request).await?;
    Ok(RouterPlan {
        accepted: true,
        applied: true,
        commands,
        message: helper_message,
    })
}

async fn apply_via_privileged_helper(request: &ShapeRequest) -> Result<String, String> {
    let runtime = "/run/chasselfi";
    let pending = format!("{runtime}/shaping.request.pending");
    let request_path = format!("{runtime}/shaping.request");
    let result_path = format!("{runtime}/shaping.result");
    let _ = async_fs::remove_file(&result_path).await;
    let body = format!(
        "lan={}\nwan={}\ndownload={}\nupload={}\n",
        request.interface,
        request.wan_interface.as_deref().unwrap_or(""),
        request.download_mbps,
        request.upload_mbps
    );
    async_fs::write(&pending, body)
        .await
        .map_err(|error| format!("could not queue the privileged shaping request: {error}"))?;
    async_fs::rename(&pending, &request_path)
        .await
        .map_err(|error| format!("could not activate the shaping request: {error}"))?;
    for _ in 0..50 {
        if let Ok(result) = async_fs::read_to_string(&result_path).await {
            let _ = async_fs::remove_file(&result_path).await;
            if let Some(message) = result.strip_prefix("ok=") {
                return Ok(message.trim().to_string());
            }
            return Err(result
                .strip_prefix("error=")
                .unwrap_or(result.as_str())
                .trim()
                .to_string());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err("the privileged shaping helper did not respond; check chasselfi-shaping.path".into())
}

/// Authorize or refresh one private-LAN client through openNDS. The setup
/// script grants the unprivileged chasselfi group access to only ndsctl's
/// Unix socket; the web process never runs as root.
pub async fn opennds_authorize(
    client_ip: &str,
    minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
) -> Result<(), String> {
    validate_client_ip(client_ip)?;
    if minutes == 0 || minutes > 525_600 {
        return Err("Session duration is outside the supported range".into());
    }
    // openNDS does not reliably replace limits on an already authenticated
    // client. Reauthorize from a clean state so added time and rate changes
    // become the forwarding engine's actual values.
    let _ = run_ndsctl(&["deauth".into(), client_ip.into()]).await;
    run_ndsctl(&[
        "auth".into(),
        client_ip.into(),
        minutes.to_string(),
        upload_mbps.saturating_mul(1000).to_string(),
        download_mbps.saturating_mul(1000).to_string(),
        "0".into(),
        "0".into(),
        "chasselfi".into(),
    ])
    .await
}

pub async fn opennds_deauthorize(client_ip: &str) -> Result<(), String> {
    validate_client_ip(client_ip)?;
    run_ndsctl(&["deauth".into(), client_ip.into()]).await
}

async fn run_ndsctl(arguments: &[String]) -> Result<(), String> {
    let output = Command::new("ndsctl")
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            format!("openNDS control is unavailable ({error}); run deploy/router/setup-opennds.sh")
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if detail.is_empty() {
        "openNDS rejected the client session update".into()
    } else {
        format!("openNDS rejected the client session update: {detail}")
    })
}

fn validate_client_ip(client_ip: &str) -> Result<(), String> {
    let parsed = client_ip
        .parse::<IpAddr>()
        .map_err(|_| "Client IP address is invalid".to_string())?;
    if !matches!(parsed, IpAddr::V4(address) if address.is_private()) {
        return Err("Only private IPv4 captive-LAN clients can be controlled".into());
    }
    Ok(())
}

async fn command_available(command: &str) -> bool {
    let probe = format!("command -v -- {command} >/dev/null 2>&1");
    Command::new("sh")
        .args(["-c", &probe])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

fn validate_interface(interface: &str) -> Result<(), String> {
    if interface.is_empty()
        || interface.len() > 15
        || !interface.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err("Interface name contains unsupported characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opennds_control_only_accepts_private_ipv4_clients() {
        assert!(validate_client_ip("10.0.0.100").is_ok());
        assert!(validate_client_ip("172.16.1.5").is_ok());
        assert!(validate_client_ip("8.8.8.8").is_err());
        assert!(validate_client_ip("::1").is_err());
        assert!(validate_client_ip("not-an-ip").is_err());
    }
}

use crate::config::HardwareMode;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterStatus {
    pub mode: String,
    pub live_apply_enabled: bool,
    pub tc_available: bool,
    pub nft_available: bool,
    pub dnsmasq_available: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeRequest {
    pub interface: String,
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
            message: "Safe simulation mode. No host networking commands will run.".into(),
        };
    }
    RouterStatus {
        mode: "linux".into(),
        live_apply_enabled,
        tc_available: command_available("tc").await,
        nft_available: command_available("nft").await,
        dnsmasq_available: command_available("dnsmasq").await,
        message: if live_apply_enabled {
            "Live router commands are enabled by CHASSELFI_LIVE_ROUTER=1.".into()
        } else {
            "Dry-run only. Set CHASSELFI_LIVE_ROUTER=1 after reviewing the target interface.".into()
        },
    }
}

pub async fn apply(mode: &HardwareMode, request: ShapeRequest) -> Result<RouterPlan, String> {
    validate_interface(&request.interface)?;
    if !(1..=10_000).contains(&request.download_mbps)
        || !(1..=10_000).contains(&request.upload_mbps)
    {
        return Err("Bandwidth must be between 1 and 10,000 Mbps".into());
    }
    // A root qdisc controls egress on one interface. Upload shaping normally
    // needs a separate WAN egress interface or an IFB redirect, so do not
    // issue two conflicting `root` replacements on the same device.
    let commands = vec![vec![
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
                "Plan validated but not applied (dry-run). Download shaping is planned on {}; upload requires a topology-specific WAN/IFB rule for {} Mbps.",
                request.interface, request.upload_mbps
            ),
        });
    }
    for args in &commands {
        let status = Command::new(&args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .map_err(|error| format!("could not start tc: {error}"))?;
        if !status.success() {
            return Err("tc rejected the shaping command; inspect router logs".into());
        }
    }
    Ok(RouterPlan {
        accepted: true,
        applied: true,
        commands,
        message: format!(
            "Download shaping applied on {}; upload still requires a topology-specific WAN/IFB rule for {} Mbps.",
            request.interface, request.upload_mbps
        ),
    })
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
    Command::new(command)
        .arg("--version")
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

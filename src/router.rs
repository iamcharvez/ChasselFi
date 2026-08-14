use crate::config::HardwareMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClient {
    pub ip: String,
    pub mac: String,
    pub client_if: String,
    pub state: String,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub average_download_kbps: f64,
    pub average_upload_kbps: f64,
    pub session_start: u64,
    pub session_end: u64,
    pub last_active: u64,
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
        "besteffort".into(),
        "dual-dsthost".into(),
        "nat".into(),
        "wash".into(),
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
            "besteffort".into(),
            "dual-srchost".into(),
            "nat".into(),
            "wash".into(),
            "ack-filter".into(),
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
    client_mac: &str,
    minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
) -> Result<(), String> {
    validate_client_ip(client_ip)?;
    let mac = normalize_mac(client_mac).ok();
    if minutes == 0 || minutes > 525_600 {
        return Err("Session duration is outside the supported range".into());
    }
    // openNDS does not reliably replace limits on an already authenticated
    // client. Reauthorize from a clean state so added time and rate changes
    // become the forwarding engine's actual values.
    verify_client_identity(client_ip, mac.as_deref()).await?;
    if let Some(mac) = mac.as_deref() {
        let _ = run_ndsctl(&["deauth".into(), mac.into()]).await;
    }
    let _ = run_ndsctl(&["deauth".into(), client_ip.into()]).await;
    let identity = mac.as_deref().unwrap_or(client_ip);
    run_ndsctl(&[
        "auth".into(),
        identity.into(),
        minutes.to_string(),
        upload_mbps.saturating_mul(1000).to_string(),
        download_mbps.saturating_mul(1000).to_string(),
        "0".into(),
        "0".into(),
        "chasselfi".into(),
    ])
    .await?;
    wait_for_gateway_state(client_ip, mac.as_deref(), true).await
}

pub async fn opennds_deauthorize(client_ip: &str, client_mac: &str) -> Result<(), String> {
    validate_client_ip(client_ip)?;
    let mac = normalize_mac(client_mac).ok();
    let mut last_error = None;
    if let Some(mac) = mac.as_deref() {
        if let Err(error) = run_ndsctl(&["deauth".into(), mac.into()]).await {
            last_error = Some(error);
        }
    }
    if let Err(error) = run_ndsctl(&["deauth".into(), client_ip.into()]).await {
        last_error = Some(error);
    }
    match wait_for_gateway_state(client_ip, mac.as_deref(), false).await {
        Ok(()) => Ok(()),
        Err(verify_error) => Err(last_error.unwrap_or(verify_error)),
    }
}

pub async fn opennds_clients() -> Result<Vec<GatewayClient>, String> {
    let output = run_ndsctl_output(&["json".into()]).await?;
    let mut clients = parse_opennds_clients(&output)?;
    merge_neighbor_clients(&mut clients).await;
    Ok(clients)
}

async fn merge_neighbor_clients(clients: &mut Vec<GatewayClient>) {
    let Ok(output) = Command::new("ip")
        .args(["-j", "neigh", "show"])
        .stdin(Stdio::null())
        .output()
        .await
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(Value::Array(neighbors)) = serde_json::from_slice::<Value>(&output.stdout) else {
        return;
    };
    for neighbor in neighbors {
        let Some(object) = neighbor.as_object() else {
            continue;
        };
        let ip = string_field(object.get("dst"));
        let mac = string_field(object.get("lladdr")).to_ascii_lowercase();
        let client_if = string_field(object.get("dev"));
        let state = object
            .get("state")
            .and_then(Value::as_array)
            .and_then(|states| states.first())
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !is_customer_ip(&ip)
            || normalize_mac(&mac).is_err()
            || matches!(state, "FAILED" | "INCOMPLETE")
        {
            continue;
        }
        if let Some(existing) = clients
            .iter_mut()
            .find(|client| client.ip == ip && client.mac.eq_ignore_ascii_case(&mac))
        {
            if existing.client_if.is_empty() {
                existing.client_if = client_if;
            }
            continue;
        }
        clients.push(GatewayClient {
            ip,
            mac,
            client_if,
            state: "Connected".into(),
            downloaded_bytes: 0,
            uploaded_bytes: 0,
            average_download_kbps: 0.0,
            average_upload_kbps: 0.0,
            session_start: 0,
            session_end: 0,
            last_active: 0,
        });
    }
}

fn parse_opennds_clients(output: &str) -> Result<Vec<GatewayClient>, String> {
    let value: Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("openNDS returned invalid client JSON: {error}"))?;
    let clients = value
        .get("clients")
        .ok_or_else(|| "openNDS client JSON has no clients collection".to_string())?;
    let records: Vec<(String, &Value)> = if let Some(object) = clients.as_object() {
        object
            .iter()
            .map(|(key, value)| (key.clone(), value))
            .collect()
    } else if let Some(array) = clients.as_array() {
        array.iter().map(|value| (String::new(), value)).collect()
    } else {
        return Err("openNDS clients collection is not an object or array".into());
    };
    Ok(records
        .into_iter()
        .filter_map(|(key, value)| {
            let object = value.as_object()?;
            Some(GatewayClient {
                ip: string_field(object.get("ip")),
                mac: {
                    let value = string_field(object.get("mac"));
                    if value.is_empty() {
                        key.to_ascii_lowercase()
                    } else {
                        value.to_ascii_lowercase()
                    }
                },
                client_if: string_field(object.get("clientif")),
                state: string_field(object.get("state")),
                // openNDS exposes these cumulative counters in KiB. The API and
                // browser use bytes so formatting remains consistent elsewhere.
                downloaded_bytes: integer_field(object.get("downloaded")).saturating_mul(1024),
                uploaded_bytes: integer_field(object.get("uploaded")).saturating_mul(1024),
                average_download_kbps: float_field(object.get("avg_down_speed")),
                average_upload_kbps: float_field(object.get("avg_up_speed")),
                session_start: integer_field(object.get("session_start")),
                session_end: integer_field(object.get("session_end")),
                last_active: integer_field(object.get("last_active")),
            })
        })
        .collect())
}

async fn verify_client_identity(client_ip: &str, client_mac: Option<&str>) -> Result<(), String> {
    let Some(client_mac) = client_mac else {
        return Ok(());
    };
    let clients = opennds_clients().await?;
    if let Some(client) = clients.iter().find(|client| client.ip == client_ip) {
        if !client.mac.eq_ignore_ascii_case(client_mac) {
            let _ = run_ndsctl(&["deauth".into(), client.ip.clone()]).await;
            let _ = run_ndsctl(&["deauth".into(), client.mac.clone()]).await;
            return Err(format!(
                "security check failed: {client_ip} belongs to gateway MAC {}, not {client_mac}",
                client.mac
            ));
        }
    }
    if let Some(client) = clients
        .iter()
        .find(|client| client.mac.eq_ignore_ascii_case(client_mac))
    {
        if client.ip != client_ip {
            let _ = run_ndsctl(&["deauth".into(), client.ip.clone()]).await;
            let _ = run_ndsctl(&["deauth".into(), client.mac.clone()]).await;
            return Err(format!(
                "security check failed: gateway MAC {client_mac} belongs to {}, not {client_ip}",
                client.ip
            ));
        }
    }
    Ok(())
}

async fn wait_for_gateway_state(
    client_ip: &str,
    client_mac: Option<&str>,
    authenticated: bool,
) -> Result<(), String> {
    for _ in 0..10 {
        if let Ok(clients) = opennds_clients().await {
            let matching: Vec<_> = clients
                .iter()
                .filter(|client| {
                    client.ip == client_ip
                        || client_mac.is_some_and(|mac| client.mac.eq_ignore_ascii_case(mac))
                })
                .collect();
            let has_authenticated = matching
                .iter()
                .any(|client| client.state.eq_ignore_ascii_case("authenticated"));
            if has_authenticated == authenticated {
                return Ok(());
            }
        }
        sleep(Duration::from_millis(150)).await;
    }
    Err(if authenticated {
        "openNDS did not confirm the client as authenticated".into()
    } else {
        "openNDS still reports the client as authenticated; access was not revoked".into()
    })
}

async fn run_ndsctl(arguments: &[String]) -> Result<(), String> {
    run_ndsctl_output(arguments).await.map(|_| ())
}

async fn run_ndsctl_output(arguments: &[String]) -> Result<String, String> {
    let mut last_detail = String::new();
    // ndsctl serializes access with one lock file. Dashboard polling and an
    // operator action can legitimately collide, so retry the documented busy
    // response instead of making pause/revoke fail intermittently.
    for attempt in 0..10 {
        let output = Command::new("ndsctl")
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| {
                format!(
                    "openNDS control is unavailable ({error}); run deploy/router/setup-opennds.sh"
                )
            })?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        last_detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        let transient = last_detail.to_ascii_lowercase();
        if attempt < 9
            && (transient.contains("connection refused")
                || transient.contains("not yet started")
                || transient.contains("temporarily unavailable")
                || transient.contains("thread is busy")
                || transient.contains("try later"))
        {
            sleep(Duration::from_millis(100)).await;
            continue;
        }
        break;
    }
    Err(if last_detail.is_empty() {
        "openNDS rejected the client session update".into()
    } else {
        format!("openNDS rejected the client session update: {last_detail}")
    })
}

fn string_field(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            value
                .filter(|value| !value.is_null())
                .map(Value::to_string)
                .unwrap_or_default()
        })
}

fn integer_field(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_u64)
        .or_else(|| value.and_then(Value::as_str)?.parse().ok())
        .unwrap_or(0)
}

fn float_field(value: Option<&Value>) -> f64 {
    value
        .and_then(Value::as_f64)
        .or_else(|| value.and_then(Value::as_str)?.parse().ok())
        .unwrap_or(0.0)
}

pub(crate) fn normalize_mac(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase().replace('-', ":");
    let valid = normalized.len() == 17
        && normalized.split(':').count() == 6
        && normalized
            .split(':')
            .all(|part| part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit()));
    valid
        .then_some(normalized)
        .ok_or_else(|| "Client MAC address is invalid".into())
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

fn is_customer_ip(client_ip: &str) -> bool {
    match client_ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let octets = ip.octets();
            octets[0] == 10 && octets[1] == 0 && octets[2] <= 15
        }
        _ => false,
    }
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

    #[test]
    fn parses_authenticated_and_waiting_opennds_clients() {
        let clients = parse_opennds_clients(
            r#"{"clients":{"aa:bb:cc:dd:ee:ff":{"ip":"10.0.0.100","mac":"aa:bb:cc:dd:ee:ff","clientif":"enp2s0f0.799","state":"Authenticated","downloaded":"2048","uploaded":1024,"avg_down_speed":"9123.5","avg_up_speed":321.0,"session_start":123,"session_end":"456","last_active":234},"11:22:33:44:55:66":{"ip":"10.0.0.101","state":"Preauthenticated"}}}"#,
        )
        .unwrap();
        assert_eq!(clients.len(), 2);
        assert_eq!(
            clients
                .iter()
                .find(|client| client.ip == "10.0.0.100")
                .unwrap()
                .downloaded_bytes,
            2_097_152
        );
        assert!(clients
            .iter()
            .any(|client| client.state == "Preauthenticated"));

        let array_clients = parse_opennds_clients(
            r#"{"clients":[{"ip":"10.0.0.102","mac":"22:33:44:55:66:77","state":"Preauthenticated"}]}"#,
        )
        .unwrap();
        assert_eq!(array_clients[0].mac, "22:33:44:55:66:77");
    }

    #[test]
    fn validates_full_mac_addresses() {
        assert_eq!(
            normalize_mac("AA-BB-CC-DD-EE-FF").unwrap(),
            "aa:bb:cc:dd:ee:ff"
        );
        assert!(normalize_mac("aa:bb:cc").is_err());
    }

    #[test]
    fn recognizes_only_the_configured_customer_subnet() {
        assert!(is_customer_ip("10.0.0.100"));
        assert!(is_customer_ip("10.0.15.250"));
        assert!(!is_customer_ip("10.0.16.1"));
        assert!(!is_customer_ip("172.16.253.192"));
        assert!(!is_customer_ip("not-an-ip"));
    }
}

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, net::IpAddr, path::Path};
use tokio::process::Command;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceSnapshot {
    pub name: String,
    pub mac: Option<String>,
    pub state: String,
    pub kind: String,
    pub usb: bool,
    pub has_default_route: bool,
    pub addresses: Vec<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub recommended_wan: Option<String>,
    pub recommended_lan: Option<String>,
    pub confidence: String,
    pub reason: String,
    pub containerized: bool,
    pub interfaces: Vec<InterfaceSnapshot>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPlanRequest {
    pub wan_interface: String,
    pub lan_interface: String,
    pub lan_address: Option<String>,
    pub lan_prefix: Option<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPlan {
    pub accepted: bool,
    pub applied: bool,
    pub wan_interface: String,
    pub lan_interface: String,
    pub lan_cidr: String,
    pub commands: Vec<Vec<String>>,
    pub message: String,
}

pub async fn discover() -> DiscoveryResult {
    let route_interface = default_route_interface().await;
    let address_map = ip_addresses().await;
    let mut interfaces = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let read_text = |suffix: &str| {
                fs::read_to_string(entry.path().join(suffix))
                    .ok()
                    .map(|value| value.trim().to_string())
            };
            let read_counter = |suffix: &str| {
                read_text(suffix)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            let usb = fs::canonicalize(entry.path().join("device"))
                .ok()
                .is_some_and(|path| path.to_string_lossy().to_lowercase().contains("usb"));
            let kind = interface_kind(&name);
            interfaces.push(InterfaceSnapshot {
                has_default_route: route_interface.as_deref() == Some(name.as_str()),
                addresses: address_map.get(&name).cloned().unwrap_or_default(),
                name,
                mac: read_text("address"),
                state: read_text("operstate").unwrap_or_else(|| "unknown".into()),
                kind,
                usb,
                rx_bytes: read_counter("statistics/rx_bytes"),
                tx_bytes: read_counter("statistics/tx_bytes"),
            });
        }
    }
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));

    let recommended_wan = route_interface.or_else(|| {
        interfaces
            .iter()
            .find(|item| item.kind == "ethernet" && item.state == "up")
            .map(|item| item.name.clone())
    });
    let recommended_lan = interfaces
        .iter()
        .find(|item| {
            item.addresses
                .iter()
                .any(|address| address == "10.0.0.1/20")
                || item.name.ends_with(".799")
        })
        .or_else(|| {
            interfaces.iter().find(|item| {
                item.name != recommended_wan.as_deref().unwrap_or("")
                    && item.usb
                    && item.kind == "ethernet"
            })
        })
        .or_else(|| {
            interfaces.iter().find(|item| {
                item.name != recommended_wan.as_deref().unwrap_or("")
                    && item.kind == "ethernet"
                    && !item.has_default_route
            })
        })
        .map(|item| item.name.clone());
    let confidence = if recommended_wan.is_some() && recommended_lan.is_some() {
        if interfaces.iter().any(|item| {
            recommended_lan.as_deref() == Some(item.name.as_str())
                && (item
                    .addresses
                    .iter()
                    .any(|address| address == "10.0.0.1/20")
                    || item.name.ends_with(".799"))
        }) || interfaces
            .iter()
            .any(|item| item.usb && recommended_lan.as_deref() == Some(item.name.as_str()))
        {
            "high"
        } else {
            "medium"
        }
    } else {
        "low"
    };
    let base_reason = match (&recommended_wan, &recommended_lan) {
        (Some(wan), Some(lan)) if lan.ends_with(".799") => {
            format!("WAN {wan} owns the default route; active customer VLAN {lan} is the routed LAN.")
        }
        (Some(wan), Some(lan)) if confidence == "high" => {
            format!("WAN {wan} owns the default route; USB Ethernet {lan} is the recommended client LAN.")
        }
        (Some(wan), Some(lan)) => {
            format!("WAN {wan} owns the default route; {lan} is the best remaining Ethernet candidate. Confirm the mapping.")
        }
        _ => "Could not confidently determine both interfaces. Connect both adapters and review the detected list.".into(),
    };
    let containerized = Path::new("/.dockerenv").exists()
        || fs::read_to_string("/proc/1/cgroup")
            .ok()
            .is_some_and(|value| value.contains("docker") || value.contains("containerd"));
    let reason = if containerized {
        format!("Container visibility warning: this process can only see container interfaces. {base_reason} Run the service natively or with host networking when identifying physical WAN/LAN adapters.")
    } else {
        base_reason
    };
    DiscoveryResult {
        recommended_wan,
        recommended_lan,
        confidence: confidence.into(),
        reason,
        containerized,
        interfaces,
    }
}

/// Validate a proposed server/router mapping and return the commands that an
/// administrator can review before configuring the host. This intentionally
/// does not execute anything: changing the active NIC can disconnect the
/// management session, so a later privileged installer step must apply it.
pub fn plan(input: NetworkPlanRequest) -> Result<NetworkPlan, String> {
    validate_interface(&input.wan_interface)?;
    validate_interface(&input.lan_interface)?;
    if input.wan_interface == input.lan_interface {
        return Err("WAN and LAN must use different interfaces".into());
    }
    let address = input.lan_address.unwrap_or_else(|| "10.0.0.1".into());
    let prefix = input.lan_prefix.unwrap_or(20);
    if !matches!(address.parse::<IpAddr>(), Ok(IpAddr::V4(_))) {
        return Err("LAN address must be a valid IPv4 address".into());
    }
    if !(8..=30).contains(&prefix) {
        return Err("LAN prefix must be between /8 and /30".into());
    }
    let lan_cidr = format!("{address}/{prefix}");
    let commands = vec![
        vec![
            "ip".into(),
            "addr".into(),
            "replace".into(),
            lan_cidr.clone(),
            "dev".into(),
            input.lan_interface.clone(),
        ],
        vec!["sysctl".into(), "-w".into(), "net.ipv4.ip_forward=1".into()],
        vec!["nft".into(), "-f".into(), "/etc/nftables.conf".into()],
    ];
    Ok(NetworkPlan {
        accepted: true,
        applied: false,
        wan_interface: input.wan_interface,
        lan_interface: input.lan_interface,
        lan_cidr,
        commands,
        message: "Server mode: mapping validated and commands generated for review. No host network changes were applied.".into(),
    })
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

async fn default_route_interface() -> Option<String> {
    let output = Command::new("ip")
        .args(["-j", "route", "show", "default"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let routes: Value = serde_json::from_slice(&output.stdout).ok()?;
    routes
        .as_array()?
        .iter()
        .find_map(|route| route.get("dev").and_then(Value::as_str).map(str::to_string))
}

async fn ip_addresses() -> std::collections::HashMap<String, Vec<String>> {
    let mut result = std::collections::HashMap::new();
    let Ok(output) = Command::new("ip").args(["-j", "address"]).output().await else {
        return result;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return result;
    };
    for item in value.as_array().into_iter().flatten() {
        let Some(name) = item.get("ifname").and_then(Value::as_str) else {
            continue;
        };
        let addresses = item
            .get("addr_info")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|address| {
                let local = address.get("local")?.as_str()?;
                let prefix = address
                    .get("prefixlen")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                Some(format!("{local}/{prefix}"))
            })
            .collect();
        result.insert(name.to_string(), addresses);
    }
    result
}

fn interface_kind(name: &str) -> String {
    if name == "lo" {
        return "loopback".into();
    }
    if name.starts_with("wl") || name.starts_with("wlan") {
        return "wifi".into();
    }
    if name.starts_with("docker") || name.starts_with("br-") || name.starts_with("virbr") {
        return "bridge".into();
    }
    if name.contains('.') {
        return "vlan".into();
    }
    if Path::new(&format!("/sys/class/net/{name}/device")).exists() {
        return "ethernet".into();
    }
    "other".into()
}

#[cfg(test)]
mod tests {
    use super::{plan, NetworkPlanRequest};

    #[test]
    fn network_plan_is_review_only() {
        let result = plan(NetworkPlanRequest {
            wan_interface: "eth0".into(),
            lan_interface: "enxusb0".into(),
            lan_address: None,
            lan_prefix: None,
        })
        .expect("valid mapping");
        assert!(!result.applied);
        assert_eq!(result.lan_cidr, "10.0.0.1/20");
        assert_eq!(result.commands.len(), 3);
    }

    #[test]
    fn network_plan_rejects_same_interface() {
        let error = plan(NetworkPlanRequest {
            wan_interface: "eth0".into(),
            lan_interface: "eth0".into(),
            lan_address: Some("10.0.0.1".into()),
            lan_prefix: Some(20),
        })
        .expect_err("same interface must be rejected");
        assert!(error.contains("different interfaces"));
    }
}

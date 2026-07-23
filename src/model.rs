use chrono::{DateTime, Duration, Local, Utc};
use rand::{distr::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rate {
    pub id: Uuid,
    pub price: u32,
    pub minutes: u32,
    pub download_mbps: u32,
    pub upload_mbps: u32,
    pub label: String,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voucher {
    pub id: Uuid,
    pub code: String,
    pub minutes: u32,
    pub price: u32,
    pub status: VoucherStatus,
    pub batch: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VoucherStatus {
    Ready,
    Used,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub id: Uuid,
    pub kind: String,
    pub amount: u32,
    pub minutes: u32,
    pub client_ip: String,
    pub mac: String,
    pub station: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: Uuid,
    pub client_name: String,
    pub ip: String,
    pub mac: String,
    pub remaining_seconds: i64,
    pub status: SessionStatus,
    pub download_mbps: f32,
    pub upload_mbps: f32,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub device_key: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Online,
    Paused,
    Ended,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedSite {
    pub id: Uuid,
    pub host: String,
    pub note: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub shop_name: String,
    pub timezone: String,
    pub currency: String,
    pub portal_message: String,
    pub buy_time: bool,
    pub vouchers: bool,
    pub auto_pause: bool,
    pub download_limit_mbps: u32,
    pub upload_limit_mbps: u32,
    pub maintenance_schedule: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub rates: Vec<Rate>,
    pub vouchers: Vec<Voucher>,
    pub transactions: Vec<Transaction>,
    pub sessions: Vec<Session>,
    pub blocked_sites: Vec<BlockedSite>,
    pub settings: Settings,
}

impl Default for Store {
    fn default() -> Self {
        let now = Utc::now();
        let rates = vec![
            rate(5, 30, 10, "Quick browse"),
            rate(10, 120, 15, "Popular"),
            rate(20, 300, 20, "Best value"),
            rate(30, 720, 25, "Day pass"),
        ];
        let clients = [
            ("realme C55", "10.10.0.14", "A4:55:90:10:21:0E"),
            ("Android phone", "10.10.0.27", "82:AF:31:44:1C:D9"),
            ("Juan's laptop", "10.10.0.42", "54:E1:AD:7B:03:11"),
        ];
        let sessions = clients
            .iter()
            .enumerate()
            .map(|(i, (name, ip, mac))| Session {
                id: Uuid::new_v4(),
                client_name: (*name).into(),
                ip: (*ip).into(),
                mac: (*mac).into(),
                remaining_seconds: [4420, 1860, 720][i],
                status: if i == 2 {
                    SessionStatus::Paused
                } else {
                    SessionStatus::Online
                },
                download_mbps: [3.8, 1.2, 0.0][i],
                upload_mbps: [0.4, 0.2, 0.0][i],
                started_at: now - Duration::minutes((i as i64 + 1) * 24),
                access_token: None,
                device_key: None,
                last_seen_at: Some(now),
            })
            .collect();
        let mut transactions = Vec::new();
        for day in 0..14 {
            let count = 2 + (day % 4);
            for n in 0..count {
                let r = &rates[((day + n) as usize) % rates.len()];
                transactions.push(Transaction {
                    id: Uuid::new_v4(),
                    kind: if n == 0 && day % 3 == 0 {
                        "Voucher".into()
                    } else {
                        "Coin".into()
                    },
                    amount: r.price,
                    minutes: r.minutes,
                    client_ip: format!("10.10.0.{}", 14 + day + n),
                    mac: format!("A4:55:90:10:{:02X}:{:02X}", day, n),
                    station: "Main vendo".into(),
                    created_at: now - Duration::days(day as i64) - Duration::minutes(n as i64 * 51),
                });
            }
        }
        Self {
            rates,
            vouchers: vec![],
            transactions,
            sessions,
            blocked_sites: vec![BlockedSite {
                id: Uuid::new_v4(),
                host: "example-blocked.test".into(),
                note: "Demo rule".into(),
                created_at: now,
            }],
            settings: Settings::default(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shop_name: "ChasselFi Piso WiFi".into(),
            timezone: "Asia/Manila".into(),
            currency: "PHP".into(),
            portal_message: "Fast, fair internet for everyone.".into(),
            buy_time: true,
            vouchers: true,
            auto_pause: true,
            download_limit_mbps: 15,
            upload_limit_mbps: 10,
            maintenance_schedule: false,
        }
    }
}

fn rate(price: u32, minutes: u32, speed: u32, label: &str) -> Rate {
    Rate {
        id: Uuid::new_v4(),
        price,
        minutes,
        download_mbps: speed,
        upload_mbps: speed,
        label: label.into(),
        active: true,
    }
}

pub fn voucher_code() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_uppercase()
}

pub fn batch_code() -> String {
    format!(
        "{}{:02}",
        Local::now().format("%m%d"),
        rand::rng().random_range(10..99)
    )
}

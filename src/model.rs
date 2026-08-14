use chrono::{DateTime, Local, Utc};
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
pub struct FreeTimeClaim {
    pub id: Uuid,
    pub device_key: String,
    pub client_ip: String,
    pub mac: String,
    pub minutes: u32,
    pub claimed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinNodeProfile {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMode {
    None,
    #[default]
    Voucher,
    Coin,
    Both,
}

impl PaymentMode {
    pub fn allows_voucher(self) -> bool {
        matches!(self, Self::Voucher | Self::Both)
    }

    pub fn allows_coin(self) -> bool {
        matches!(self, Self::Coin | Self::Both)
    }
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
    #[serde(default = "default_portal_eyebrow")]
    pub portal_eyebrow: String,
    #[serde(default = "default_portal_headline")]
    pub portal_headline: String,
    #[serde(default = "default_portal_status_label")]
    pub portal_status_label: String,
    #[serde(default = "default_portal_rates_label")]
    pub portal_rates_label: String,
    #[serde(default = "default_portal_voucher_label")]
    pub portal_voucher_label: String,
    #[serde(default = "default_portal_coin_label")]
    pub portal_coin_label: String,
    #[serde(default = "default_portal_free_label")]
    pub portal_free_label: String,
    #[serde(default)]
    pub portal_banner_image: String,
    #[serde(default)]
    pub portal_logo_image: String,
    #[serde(default = "default_true")]
    pub portal_show_device: bool,
    #[serde(default)]
    pub payment_mode: PaymentMode,
    #[serde(default = "default_coin_pulse_value")]
    pub coin_pulse_value: u32,
    #[serde(default = "default_true")]
    pub buy_time: bool,
    #[serde(default = "default_true")]
    pub vouchers: bool,
    pub auto_pause: bool,
    pub download_limit_mbps: u32,
    pub upload_limit_mbps: u32,
    pub maintenance_schedule: bool,
    #[serde(default)]
    pub free_time_enabled: bool,
    #[serde(default = "default_free_time_minutes")]
    pub free_time_minutes: u32,
    #[serde(default = "default_free_time_reset_hours")]
    pub free_time_reset_hours: u32,
    #[serde(default = "default_true")]
    pub require_terms: bool,
    #[serde(default = "default_terms_title")]
    pub terms_title: String,
    #[serde(default = "default_terms_body")]
    pub terms_body: String,
    #[serde(default = "default_portal_accent")]
    pub portal_accent: String,
    #[serde(default = "default_portal_template")]
    pub portal_template: String,
    #[serde(default = "default_voucher_template")]
    pub voucher_template: String,
    #[serde(default = "default_voucher_footer")]
    pub voucher_footer: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub rates: Vec<Rate>,
    pub vouchers: Vec<Voucher>,
    pub transactions: Vec<Transaction>,
    pub sessions: Vec<Session>,
    pub blocked_sites: Vec<BlockedSite>,
    #[serde(default)]
    pub free_time_claims: Vec<FreeTimeClaim>,
    #[serde(default)]
    pub coin_nodes: Vec<CoinNodeProfile>,
    pub settings: Settings,
}

impl Default for Store {
    fn default() -> Self {
        Self::production()
    }
}

impl Store {
    /// A clean first-run store. ChasselFi never invents sales or clients.
    pub fn production() -> Self {
        Self {
            rates: vec![
                rate(5, 30, 10, "Quick browse"),
                rate(10, 120, 15, "Popular"),
                rate(20, 300, 20, "Best value"),
                rate(30, 720, 25, "Day pass"),
            ],
            vouchers: Vec::new(),
            transactions: Vec::new(),
            sessions: Vec::new(),
            blocked_sites: Vec::new(),
            free_time_claims: Vec::new(),
            coin_nodes: Vec::new(),
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
            portal_eyebrow: default_portal_eyebrow(),
            portal_headline: default_portal_headline(),
            portal_status_label: default_portal_status_label(),
            portal_rates_label: default_portal_rates_label(),
            portal_voucher_label: default_portal_voucher_label(),
            portal_coin_label: default_portal_coin_label(),
            portal_free_label: default_portal_free_label(),
            portal_banner_image: String::new(),
            portal_logo_image: String::new(),
            portal_show_device: true,
            payment_mode: PaymentMode::Voucher,
            coin_pulse_value: 1,
            buy_time: true,
            vouchers: true,
            auto_pause: true,
            download_limit_mbps: 15,
            upload_limit_mbps: 10,
            maintenance_schedule: false,
            free_time_enabled: false,
            free_time_minutes: 15,
            free_time_reset_hours: 24,
            require_terms: true,
            terms_title: "Fair use and privacy".into(),
            terms_body: "Use this network lawfully and respectfully. Do not attack other devices, bypass access controls, or use the service for illegal activity. Network activity may be logged for security and operations.".into(),
            portal_accent: "#28d17c".into(),
            portal_template: "aurora".into(),
            voucher_template: "modern".into(),
            voucher_footer: "Thank you for choosing ChasselFi WiFi".into(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_coin_pulse_value() -> u32 {
    1
}

fn default_free_time_minutes() -> u32 {
    15
}
fn default_free_time_reset_hours() -> u32 {
    24
}
fn default_terms_title() -> String {
    "Fair use and privacy".into()
}
fn default_terms_body() -> String {
    "Use this network lawfully and respectfully. Do not attack other devices, bypass access controls, or use the service for illegal activity. Network activity may be logged for security and operations.".into()
}
fn default_portal_accent() -> String {
    "#28d17c".into()
}
fn default_portal_template() -> String {
    "aurora".into()
}
fn default_portal_eyebrow() -> String {
    "FAST • FAIR • LOCAL".into()
}
fn default_portal_headline() -> String {
    "Your WiFi. Your time.".into()
}
fn default_portal_status_label() -> String {
    "High-speed connection".into()
}
fn default_portal_rates_label() -> String {
    "View time rates".into()
}
fn default_portal_voucher_label() -> String {
    "Use voucher".into()
}
fn default_portal_coin_label() -> String {
    "Insert coin".into()
}
fn default_portal_free_label() -> String {
    "Claim free time".into()
}
fn default_voucher_template() -> String {
    "modern".into()
}
fn default_voucher_footer() -> String {
    "Thank you for choosing ChasselFi WiFi".into()
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

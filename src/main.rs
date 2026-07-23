mod config;
mod model;
mod router;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use argon2::{password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use chrono::{Duration, Utc};
use config::{Config, HardwareMode};
use model::{
    batch_code, voucher_code, BlockedSite, Rate, Session, SessionStatus, Store, Transaction, Voucher,
    VoucherStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use rusqlite::{params, Connection};
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Instant};
use sysinfo::System;
use tokio::{net::TcpListener, sync::RwLock, time::{interval, Duration as TokioDuration}};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    store: Arc<RwLock<Store>>,
    database_file: PathBuf,
    started_at: Instant,
    hardware_mode: HardwareMode,
    admin_username: String,
    admin_password_hash: String,
    sessions: Arc<RwLock<HashMap<String, AuthSession>>>,
    login_throttle: Arc<RwLock<HashMap<String, LoginThrottle>>>,
}

#[derive(Clone)]
struct AuthSession {
    csrf: String,
    expires_at: Instant,
    last_seen_at: Instant,
}

#[derive(Clone, Copy)]
struct LoginThrottle {
    failures: u8,
    window_started: Instant,
    locked_until: Option<Instant>,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn env_compat(primary: &str, legacy: &str) -> Option<String> {
    std::env::var(primary).ok().or_else(|| std::env::var(legacy).ok())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("chasselfi=info".parse().unwrap()),
        )
        .init();

    let config = Config::load();
    let data_dir = PathBuf::from(&config.data_dir);
    fs::create_dir_all(&data_dir).expect("create data directory");
    let database_file = data_dir.join("chasselfi.sqlite3");
    let legacy_database = data_dir.join("bantay.sqlite3");
    if !database_file.exists() && legacy_database.exists() {
        fs::copy(&legacy_database, &database_file).expect("migrate legacy database filename");
    }
    let legacy_file = data_dir.join("store.json");
    let store = load_store(&database_file, &legacy_file);
    let admin_username = env_compat("CHASSELFI_ADMIN_USER", "BANTAY_ADMIN_USER")
        .unwrap_or_else(|| "admin".into());
    let admin_password = env_compat("CHASSELFI_ADMIN_PASSWORD", "BANTAY_ADMIN_PASSWORD")
        .unwrap_or_else(|| {
        warn!("CHASSELFI_ADMIN_PASSWORD is not set; using the development password");
        "change-me-now".into()
    });
    let admin_password_hash = hash_password(&admin_password).expect("hash admin password");
    let state = AppState {
        store: Arc::new(RwLock::new(store)),
        database_file,
        started_at: Instant::now(),
        hardware_mode: config.hardware_mode,
        admin_username,
        admin_password_hash,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        login_throttle: Arc::new(RwLock::new(HashMap::new())),
    };

    tokio::spawn(session_enforcement_loop(state.clone()));

    let api = Router::new()
        .route("/health", get(health))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/auth/me", get(auth_me))
        .route("/overview", get(overview))
        .route("/business/summary", get(business_summary))
        .route("/system", get(system_status))
        .route("/router/status", get(router_status))
        .route("/router/apply", post(router_apply))
        .route("/backup", get(download_backup))
        .route("/backup/restore", post(restore_backup))
        .route("/portal/purchase", post(portal_purchase))
        .route("/session/heartbeat", post(session_heartbeat))
        .route("/rates", get(list_rates).post(create_rate))
        .route("/rates/{id}", put(update_rate).delete(delete_rate))
        .route("/vouchers", get(list_vouchers))
        .route("/vouchers/generate", post(generate_vouchers))
        .route("/vouchers/redeem", post(redeem_voucher))
        .route("/vouchers/{id}", delete(delete_voucher))
        .route(
            "/transactions",
            get(list_transactions).post(create_transaction),
        )
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}/{action}", post(session_action))
        .route(
            "/blocked-sites",
            get(list_blocked_sites).post(create_blocked_site),
        )
        .route("/blocked-sites/{id}", delete(delete_blocked_site))
        .route("/settings", get(get_settings).put(update_settings))
        .route("/system/{action}", post(system_action));

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(ServeDir::new("web").append_index_html_on_directories(true))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .layer(middleware::from_fn(security_headers))
        .with_state(state);

    let listener = TcpListener::bind(config.listen).await.expect("bind server");
    info!("ChasselFi Piso WiFi is running at http://{}", config.listen);
    axum::serve(listener, app).await.expect("serve application");
}

fn load_store(database_file: &PathBuf, legacy_file: &PathBuf) -> Store {
    let connection = Connection::open(database_file).expect("open SQLite database");
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_state (id INTEGER PRIMARY KEY CHECK (id = 1), schema_version INTEGER NOT NULL, payload TEXT NOT NULL, updated_at TEXT NOT NULL);",
    ).expect("create state table");
    let stored: Option<String> = connection
        .query_row("SELECT payload FROM app_state WHERE id = 1", [], |row| row.get(0))
        .ok();
    let store = stored
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| {
            fs::read_to_string(legacy_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
        })
        .unwrap_or_else(|| {
            warn!("using demo data for a new SQLite database");
            Store::default()
        });
    let raw = serde_json::to_string(&store).expect("serialize initial state");
    connection.execute(
        "INSERT INTO app_state (id, schema_version, payload, updated_at) VALUES (1, 1, ?1, ?2) ON CONFLICT(id) DO UPDATE SET schema_version=1, payload=excluded.payload, updated_at=excluded.updated_at",
        params![raw, Utc::now().to_rfc3339()],
    ).expect("seed SQLite state");
    store
}

async fn persist(state: &AppState) -> Result<(), String> {
    let store = state.store.read().await;
    let raw = serde_json::to_string(&*store).map_err(|e| e.to_string())?;
    let connection = Connection::open(&state.database_file).map_err(|e| e.to_string())?;
    connection.execute(
        "INSERT INTO app_state (id, schema_version, payload, updated_at) VALUES (1, 1, ?1, ?2) ON CONFLICT(id) DO UPDATE SET schema_version=1, payload=excluded.payload, updated_at=excluded.updated_at",
        params![raw, Utc::now().to_rfc3339()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Result<(HeaderMap, Json<Value>), (StatusCode, Json<Value>)> {
    let throttle_key = format!("{}:{}", client_key(&headers), input.username);
    {
        let throttle = state.login_throttle.write().await;
        if let Some(record) = throttle.get(&throttle_key) {
            if record.locked_until.is_some_and(|until| until > Instant::now()) {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({ "error": "Too many login attempts. Try again later." })),
                ));
            }
        }
    }
    if input.username != state.admin_username
        || !verify_password(&input.password, &state.admin_password_hash)
    {
        let mut throttle = state.login_throttle.write().await;
        let now = Instant::now();
        let record = throttle.entry(throttle_key).or_insert(LoginThrottle {
            failures: 0,
            window_started: now,
            locked_until: None,
        });
        if now.duration_since(record.window_started) > std::time::Duration::from_secs(300) {
            record.failures = 0;
            record.window_started = now;
        }
        record.failures = record.failures.saturating_add(1);
        if record.failures >= 5 {
            record.locked_until = Some(now + std::time::Duration::from_secs(900));
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid administrator credentials" })),
        ));
    }
    state.login_throttle.write().await.remove(&throttle_key);
    let session = Uuid::new_v4().to_string();
    let csrf = Uuid::new_v4().to_string();
    let now = Instant::now();
    state.sessions.write().await.insert(session.clone(), AuthSession {
        csrf: csrf.clone(),
        expires_at: now + std::time::Duration::from_secs(8 * 60 * 60),
        last_seen_at: now,
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "chasselfi_session={session}; Path=/; Max-Age=28800; HttpOnly; SameSite=Lax{}",
            if env_compat("CHASSELFI_SECURE_COOKIES", "BANTAY_SECURE_COOKIES").as_deref() == Some("1") { "; Secure" } else { "" }
        ))
        .expect("valid session cookie"),
    );
    Ok((headers, Json(json!({ "username": state.admin_username, "csrfToken": csrf }))))
}

async fn logout(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
) -> (HeaderMap, Json<Value>) {
    if let Some(token) = cookie_value(request.headers(), "chasselfi_session")
        .or_else(|| cookie_value(request.headers(), "bantay_session")) {
        state.sessions.write().await.remove(&token);
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_static("chasselfi_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"),
    );
    (headers, Json(json!({ "loggedOut": true })))
}

async fn auth_me(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let token = cookie_value(request.headers(), "chasselfi_session")
        .or_else(|| cookie_value(request.headers(), "bantay_session"))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Login required" }))))?;
    let csrf = session_csrf(&state, &token).await.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Login required" })))
    })?;
    Ok(Json(json!({ "username": state.admin_username, "csrfToken": csrf })))
}

async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/api")
        || matches!(path, "/api/health" | "/api/login" | "/api/logout")
        || (request.method() == Method::GET && matches!(path, "/api/rates" | "/api/settings"))
        || (request.method() == Method::POST
            && matches!(path, "/api/vouchers/redeem" | "/api/portal/purchase" | "/api/session/heartbeat"))
    {
        return next.run(request).await;
    }
    let token = cookie_value(request.headers(), "chasselfi_session")
        .or_else(|| cookie_value(request.headers(), "bantay_session"));
    let csrf = if let Some(session) = token.as_ref() {
        session_csrf(&state, session).await
    } else {
        None
    };
    if token.is_none() || csrf.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Login required" }))).into_response();
    }
    if request.method() != Method::GET
        && request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            != csrf.as_deref()
    {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "CSRF token missing or invalid" }))).into_response();
    }
    next.run(request).await
}

async fn session_csrf(state: &AppState, token: &str) -> Option<String> {
    let mut sessions = state.sessions.write().await;
    let now = Instant::now();
    let record = sessions.get_mut(token)?;
    if record.expires_at <= now {
        sessions.remove(token);
        return None;
    }
    record.last_seen_at = now;
    Some(record.csrf.clone())
}

fn client_key(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

async fn security_headers(request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("permissions-policy", HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    response
}

fn cookie_value(headers: &HeaderMap, key: &str) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == key).then(|| value.to_string())
            })
        })
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes()).map_err(|error| error.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .ok()
        .is_some_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

fn bad_request(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}

fn not_found(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": message.into() })),
    )
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "chasselfi" }))
}

async fn session_enforcement_loop(state: AppState) {
    let mut ticker = interval(TokioDuration::from_secs(30));
    loop {
        ticker.tick().await;
        let mut changed = false;
        {
            let mut store = state.store.write().await;
            for session in &mut store.sessions {
                if session.status != SessionStatus::Online {
                    continue;
                }
                session.remaining_seconds = (session.remaining_seconds - 30).max(0);
                session.last_seen_at = Some(Utc::now());
                changed = true;
                if session.remaining_seconds == 0 {
                    session.status = SessionStatus::Ended;
                }
            }
        }
        if changed {
            if let Err(error) = persist(&state).await {
                warn!(%error, "could not persist enforced session state");
            }
        }
    }
}

async fn router_status(State(state): State<AppState>) -> Json<router::RouterStatus> {
    Json(router::status(&state.hardware_mode).await)
}

async fn router_apply(
    State(state): State<AppState>,
    Json(request): Json<router::ShapeRequest>,
) -> ApiResult<router::RouterPlan> {
    router::apply(&state.hardware_mode, request)
        .await
        .map(Json)
        .map_err(bad_request)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupExport {
    schema_version: u32,
    created_at: String,
    store: Store,
}

async fn download_backup(State(state): State<AppState>) -> Json<BackupExport> {
    Json(BackupExport {
        schema_version: 1,
        created_at: Utc::now().to_rfc3339(),
        store: state.store.read().await.clone(),
    })
}

async fn restore_backup(
    State(state): State<AppState>,
    Json(backup): Json<BackupExport>,
) -> ApiResult<Value> {
    if backup.schema_version != 1 {
        return Err(bad_request("Unsupported backup schema version"));
    }
    *state.store.write().await = backup.store;
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({ "restored": true })))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    today_sales: u32,
    week_sales: u32,
    month_sales: u32,
    total_sales: u32,
    transaction_count: usize,
    online_users: usize,
    paused_users: usize,
    ready_vouchers: usize,
    daily_sales: Vec<DailySale>,
    recent_transactions: Vec<Transaction>,
}

#[derive(Serialize)]
struct DailySale {
    date: String,
    amount: u32,
}

async fn business_summary(State(state): State<AppState>) -> Json<Value> {
    let store = state.store.read().await;
    let transaction_count = store.transactions.len() as u32;
    let total_sales: u32 = store.transactions.iter().map(|tx| tx.amount).sum();
    let coin_sales: u32 = store
        .transactions
        .iter()
        .filter(|tx| tx.kind.eq_ignore_ascii_case("coin"))
        .map(|tx| tx.amount)
        .sum();
    let voucher_sales: u32 = store
        .transactions
        .iter()
        .filter(|tx| tx.kind.eq_ignore_ascii_case("voucher"))
        .map(|tx| tx.amount)
        .sum();
    let ready_inventory_value: u32 = store
        .vouchers
        .iter()
        .filter(|voucher| voucher.status == VoucherStatus::Ready)
        .map(|voucher| voucher.price)
        .sum();
    let unique_clients = store
        .transactions
        .iter()
        .map(|tx| tx.mac.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();
    Json(json!({
        "totalSales": total_sales,
        "averageTransaction": if transaction_count == 0 { 0 } else { total_sales / transaction_count },
        "coinSales": coin_sales,
        "voucherSales": voucher_sales,
        "readyInventoryValue": ready_inventory_value,
        "uniqueClients": unique_clients,
        "activeSessions": store.sessions.iter().filter(|session| session.status == SessionStatus::Online).count()
    }))
}

async fn overview(State(state): State<AppState>) -> Json<Overview> {
    let store = state.store.read().await;
    let now = Utc::now();
    let sum_since = |days: i64| {
        store
            .transactions
            .iter()
            .filter(|tx| tx.created_at >= now - Duration::days(days))
            .map(|tx| tx.amount)
            .sum()
    };
    let daily_sales = (0..7)
        .rev()
        .map(|days| {
            let date = (now - Duration::days(days)).date_naive();
            DailySale {
                date: date.format("%b %d").to_string(),
                amount: store
                    .transactions
                    .iter()
                    .filter(|tx| tx.created_at.date_naive() == date)
                    .map(|tx| tx.amount)
                    .sum(),
            }
        })
        .collect();
    let mut recent = store.transactions.clone();
    recent.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));
    recent.truncate(8);
    Json(Overview {
        today_sales: store
            .transactions
            .iter()
            .filter(|tx| tx.created_at.date_naive() == now.date_naive())
            .map(|tx| tx.amount)
            .sum(),
        week_sales: sum_since(7),
        month_sales: sum_since(30),
        total_sales: store.transactions.iter().map(|tx| tx.amount).sum(),
        transaction_count: store.transactions.len(),
        online_users: store
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Online)
            .count(),
        paused_users: store
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Paused)
            .count(),
        ready_vouchers: store
            .vouchers
            .iter()
            .filter(|v| v.status == VoucherStatus::Ready)
            .count(),
        daily_sales,
        recent_transactions: recent,
    })
}

async fn system_status(State(state): State<AppState>) -> Json<Value> {
    let mut system = System::new_all();
    system.refresh_all();
    let used_memory = system.used_memory();
    let total_memory = system.total_memory();
    let cpu = system.global_cpu_usage();
    let online = state
        .store
        .read()
        .await
        .sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Online)
        .count();
    Json(json!({
        "uptimeSeconds": state.started_at.elapsed().as_secs(),
        "cpuPercent": (cpu * 10.0).round() / 10.0,
        "memoryUsedMb": used_memory / 1024 / 1024,
        "memoryTotalMb": total_memory / 1024 / 1024,
        "onlineUsers": online,
        "serverOnline": true,
        "coinSlotOnline": true,
        "temperatureC": null,
        "hardwareMode": if state.hardware_mode == HardwareMode::Linux { "linux" } else { "simulated" }
    }))
}

async fn list_rates(State(state): State<AppState>) -> Json<Vec<Rate>> {
    Json(state.store.read().await.rates.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateInput {
    price: u32,
    minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
    label: String,
    active: Option<bool>,
}

async fn create_rate(
    State(state): State<AppState>,
    Json(input): Json<RateInput>,
) -> ApiResult<Rate> {
    if input.price == 0 || input.minutes == 0 {
        return Err(bad_request("Price and duration must be greater than zero"));
    }
    let rate = Rate {
        id: Uuid::new_v4(),
        price: input.price,
        minutes: input.minutes,
        download_mbps: input.download_mbps,
        upload_mbps: input.upload_mbps,
        label: input.label,
        active: input.active.unwrap_or(true),
    };
    state.store.write().await.rates.push(rate.clone());
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(rate))
}

async fn update_rate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<RateInput>,
) -> ApiResult<Rate> {
    let mut store = state.store.write().await;
    let rate = store
        .rates
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("Rate not found"))?;
    rate.price = input.price;
    rate.minutes = input.minutes;
    rate.download_mbps = input.download_mbps;
    rate.upload_mbps = input.upload_mbps;
    rate.label = input.label;
    rate.active = input.active.unwrap_or(rate.active);
    let response = rate.clone();
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
}

async fn delete_rate(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let before = store.rates.len();
    store.rates.retain(|r| r.id != id);
    if before == store.rates.len() {
        return Err(not_found("Rate not found"));
    }
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"deleted": true})))
}

async fn list_vouchers(State(state): State<AppState>) -> Json<Vec<Voucher>> {
    Json(state.store.read().await.vouchers.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoucherInput {
    quantity: u32,
    minutes: u32,
    price: u32,
    expires_in_days: Option<i64>,
}

async fn generate_vouchers(
    State(state): State<AppState>,
    Json(input): Json<VoucherInput>,
) -> ApiResult<Vec<Voucher>> {
    if input.quantity == 0 || input.quantity > 100 {
        return Err(bad_request("Quantity must be between 1 and 100"));
    }
    let batch = batch_code();
    let now = Utc::now();
    let vouchers: Vec<_> = (0..input.quantity)
        .map(|_| Voucher {
            id: Uuid::new_v4(),
            code: voucher_code(),
            minutes: input.minutes,
            price: input.price,
            status: VoucherStatus::Ready,
            batch: batch.clone(),
            created_at: now,
            expires_at: input.expires_in_days.map(|d| now + Duration::days(d)),
        })
        .collect();
    state.store.write().await.vouchers.extend(vouchers.clone());
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(vouchers))
}

async fn delete_voucher(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let before = store.vouchers.len();
    store.vouchers.retain(|v| v.id != id);
    if before == store.vouchers.len() {
        return Err(not_found("Voucher not found"));
    }
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"deleted": true})))
}

#[derive(Deserialize)]
struct RedeemInput {
    code: String,
    #[serde(rename = "deviceKey")]
    device_key: Option<String>,
}

async fn redeem_voucher(
    State(state): State<AppState>,
    Json(input): Json<RedeemInput>,
) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let index = store
        .vouchers
        .iter()
        .position(|v| v.code.eq_ignore_ascii_case(input.code.trim()))
        .ok_or_else(|| not_found("Voucher code not found"))?;
    if store.vouchers[index].status != VoucherStatus::Ready {
        return Err(bad_request("This voucher is no longer available"));
    }
    if store.vouchers[index]
        .expires_at
        .is_some_and(|expiry| expiry < Utc::now())
    {
        store.vouchers[index].status = VoucherStatus::Expired;
        return Err(bad_request("This voucher has expired"));
    }
    let (minutes, amount, code) = {
        let voucher = &mut store.vouchers[index];
        voucher.status = VoucherStatus::Used;
        (voucher.minutes, voucher.price, voucher.code.clone())
    };
    store.transactions.push(Transaction {
        id: Uuid::new_v4(),
        kind: "Voucher".into(),
        amount,
        minutes,
        client_ip: "10.10.0.100".into(),
        mac: "PORTAL-CLIENT".into(),
        station: "Main vendo".into(),
        created_at: Utc::now(),
    });
    let download_limit = store.settings.download_limit_mbps;
    let upload_limit = store.settings.upload_limit_mbps;
    let session = upsert_session(
        &mut store,
        input.device_key,
        minutes,
        download_limit,
        upload_limit,
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(
        json!({"redeemed": true, "code": code, "minutes": minutes, "session": session}),
    ))
}

async fn list_transactions(State(state): State<AppState>) -> Json<Vec<Transaction>> {
    let mut items = state.store.read().await.transactions.clone();
    items.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));
    Json(items)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionInput {
    amount: u32,
    minutes: u32,
    client_ip: Option<String>,
    mac: Option<String>,
}

async fn create_transaction(
    State(state): State<AppState>,
    Json(input): Json<TransactionInput>,
) -> ApiResult<Transaction> {
    let tx = Transaction {
        id: Uuid::new_v4(),
        kind: "Coin".into(),
        amount: input.amount,
        minutes: input.minutes,
        client_ip: input.client_ip.unwrap_or_else(|| "10.10.0.99".into()),
        mac: input.mac.unwrap_or_else(|| "00:00:00:00:00:00".into()),
        station: "Main vendo".into(),
        created_at: Utc::now(),
    };
    state.store.write().await.transactions.push(tx.clone());
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(tx))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortalPurchaseInput {
    rate_id: Uuid,
    #[serde(rename = "deviceKey")]
    device_key: Option<String>,
}

async fn portal_purchase(
    State(state): State<AppState>,
    Json(input): Json<PortalPurchaseInput>,
) -> ApiResult<Value> {
    let rate = state
        .store
        .read()
        .await
        .rates
        .iter()
        .find(|rate| rate.id == input.rate_id && rate.active)
        .cloned()
        .ok_or_else(|| bad_request("That package is no longer available"))?;
    let transaction = Transaction {
        id: Uuid::new_v4(),
        kind: "Coin".into(),
        amount: rate.price,
        minutes: rate.minutes,
        client_ip: "10.10.0.100".into(),
        mac: "PORTAL-CLIENT".into(),
        station: "Main vendo".into(),
        created_at: Utc::now(),
    };
    let mut store = state.store.write().await;
    store.transactions.push(transaction.clone());
    let session = upsert_session(
        &mut store,
        input.device_key,
        rate.minutes,
        rate.download_mbps,
        rate.upload_mbps,
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"transaction": transaction, "session": session})))
}

fn upsert_session(
    store: &mut Store,
    device_key: Option<String>,
    minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
) -> Value {
    let key = device_key
        .filter(|value| {
            (8..=80).contains(&value.len())
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
                })
        })
        .unwrap_or_else(|| format!("anonymous-{}", Uuid::new_v4()));
    let now = Utc::now();
    let seconds = i64::from(minutes).saturating_mul(60);
    if let Some(session) = store.sessions.iter_mut().find(|session| {
        session.device_key.as_deref() == Some(key.as_str())
            && matches!(session.status, SessionStatus::Online | SessionStatus::Paused)
    }) {
        session.remaining_seconds = session.remaining_seconds.saturating_add(seconds);
        session.status = SessionStatus::Online;
        session.download_mbps = download_mbps as f32;
        session.upload_mbps = upload_mbps as f32;
        session.last_seen_at = Some(now);
        return json!({
            "id": session.id,
            "token": session.access_token,
            "deviceKey": key,
            "remainingSeconds": session.remaining_seconds,
            "status": "online"
        });
    }
    let id = Uuid::new_v4();
    let token = Uuid::new_v4().to_string();
    store.sessions.push(Session {
        id,
        client_name: "Portal device".into(),
        ip: "10.10.0.100".into(),
        mac: key.clone(),
        remaining_seconds: seconds,
        status: SessionStatus::Online,
        download_mbps: download_mbps as f32,
        upload_mbps: upload_mbps as f32,
        started_at: now,
        access_token: Some(token.clone()),
        device_key: Some(key.clone()),
        last_seen_at: Some(now),
    });
    json!({
        "id": id,
        "token": token,
        "deviceKey": key,
        "remainingSeconds": seconds,
        "status": "online"
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionHeartbeatInput {
    session_id: Uuid,
    token: String,
    device_key: String,
}

async fn session_heartbeat(
    State(state): State<AppState>,
    Json(input): Json<SessionHeartbeatInput>,
) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let session = store
        .sessions
        .iter_mut()
        .find(|session| session.id == input.session_id)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Session not found" }))))?;
    if session.access_token.as_deref() != Some(input.token.as_str())
        || session.device_key.as_deref() != Some(input.device_key.as_str())
    {
        return Err((StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid session token" }))));
    }
    session.last_seen_at = Some(Utc::now());
    if session.remaining_seconds == 0 {
        session.status = SessionStatus::Ended;
    }
    Ok(Json(json!({
        "id": session.id,
        "remainingSeconds": session.remaining_seconds,
        "status": session.status
    })))
}

async fn list_sessions(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.store.read().await.sessions))
}

async fn session_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(Uuid, String)>,
) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let session = store
        .sessions
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| not_found("Session not found"))?;
    session.status = match action.as_str() {
        "pause" => SessionStatus::Paused,
        "resume" if session.remaining_seconds > 0 => SessionStatus::Online,
        "resume" => SessionStatus::Ended,
        "stop" => SessionStatus::Ended,
        _ => return Err(bad_request("Unknown session action")),
    };
    let response = json!(session);
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
}

async fn list_blocked_sites(State(state): State<AppState>) -> Json<Vec<BlockedSite>> {
    Json(state.store.read().await.blocked_sites.clone())
}

#[derive(Deserialize)]
struct BlockedSiteInput {
    host: String,
    note: Option<String>,
}

async fn create_blocked_site(
    State(state): State<AppState>,
    Json(input): Json<BlockedSiteInput>,
) -> ApiResult<BlockedSite> {
    let host = input
        .host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_lowercase();
    if host.is_empty() || host.contains(' ') {
        return Err(bad_request("Enter a valid hostname or IP address"));
    }
    let item = BlockedSite {
        id: Uuid::new_v4(),
        host,
        note: input.note.unwrap_or_default(),
        created_at: Utc::now(),
    };
    state.store.write().await.blocked_sites.push(item.clone());
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(item))
}

async fn delete_blocked_site(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Value> {
    state
        .store
        .write()
        .await
        .blocked_sites
        .retain(|item| item.id != id);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"deleted": true})))
}

async fn get_settings(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.store.read().await.settings))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(settings): Json<model::Settings>,
) -> ApiResult<Value> {
    state.store.write().await.settings = settings;
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"saved": true})))
}

async fn system_action(
    State(state): State<AppState>,
    Path(action): Path<String>,
) -> ApiResult<Value> {
    if !matches!(action.as_str(), "reboot" | "shutdown") {
        return Err(bad_request("Unknown system action"));
    }
    if state.hardware_mode == HardwareMode::Simulated {
        return Ok(Json(
            json!({ "accepted": true, "simulated": true, "message": format!("{} requested in safe simulation mode", action) }),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let command = if action == "reboot" {
            "reboot"
        } else {
            "poweroff"
        };
        std::process::Command::new("systemctl")
            .arg(command)
            .spawn()
            .map_err(|e| bad_request(e.to_string()))?;
        Ok(Json(json!({"accepted": true, "simulated": false})))
    }
    #[cfg(not(target_os = "linux"))]
    Err(bad_request("Live hardware mode is supported only on Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_store_has_valid_rates() {
        let store = Store::default();
        assert!(!store.rates.is_empty());
        assert!(store
            .rates
            .iter()
            .all(|rate| rate.price > 0 && rate.minutes > 0));
    }

    #[test]
    fn generated_voucher_codes_are_eight_characters() {
        let code = voucher_code();
        assert_eq!(code.len(), 8);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}

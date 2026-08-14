mod config;
mod model;
mod network;
mod router;

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Duration, Utc};
use config::{Config, HardwareMode};
use hmac::{Hmac, Mac};
use model::{
    batch_code, voucher_code, AuditEvent, BlockedSite, CoinNodeProfile, FreeTimeClaim, Rate,
    Session, SessionStatus, Store, Transaction, Voucher, VoucherStatus,
};
use rand::{distr::Alphanumeric, Rng};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Instant,
};
use sysinfo::System;
use tokio::{
    net::TcpListener,
    sync::RwLock,
    time::{interval, Duration as TokioDuration},
};
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
    coin: Arc<RwLock<CoinRuntime>>,
}

#[derive(Clone)]
struct CoinClaim {
    id: Uuid,
    client_ip: String,
    client_mac: String,
    device_key: String,
    node_id: Option<String>,
    rate: Rate,
    inserted_pesos: u32,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct CoinReceipt {
    client_ip: String,
    session: Value,
    completed_at: DateTime<Utc>,
    gateway_error: Option<String>,
}

#[derive(Clone)]
struct CoinNodeState {
    client_ip: String,
    last_seen_at: DateTime<Utc>,
    firmware: Option<String>,
}

#[derive(Default)]
struct CoinRuntime {
    active: Option<CoinClaim>,
    completed: HashMap<Uuid, CoinReceipt>,
    socket_ready: bool,
    last_pulse_at: Option<DateTime<Utc>>,
    nodes: HashMap<String, CoinNodeState>,
    processed_events: HashMap<String, DateTime<Utc>>,
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
    std::env::var(primary)
        .ok()
        .or_else(|| std::env::var(legacy).ok())
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
    let admin_username =
        env_compat("CHASSELFI_ADMIN_USER", "BANTAY_ADMIN_USER").unwrap_or_else(|| "admin".into());
    let admin_password = match env_compat("CHASSELFI_ADMIN_PASSWORD", "BANTAY_ADMIN_PASSWORD") {
        Some(password) if password.len() >= 12 && password != "change-me-now" => password,
        Some(_) if config.hardware_mode == HardwareMode::Linux => {
            panic!("CHASSELFI_ADMIN_PASSWORD must be at least 12 characters and cannot use the development default")
        }
        Some(password) => password,
        None if config.hardware_mode == HardwareMode::Linux => {
            panic!("CHASSELFI_ADMIN_PASSWORD is required in Linux hardware mode; run deploy/install.sh or set it in /etc/chasselfi/chasselfi.env")
        }
        None => {
            warn!("CHASSELFI_ADMIN_PASSWORD is not set; using the development password in simulation mode only");
            "change-me-now".into()
        }
    };
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
        coin: Arc::new(RwLock::new(CoinRuntime::default())),
    };

    if state.hardware_mode == HardwareMode::Linux {
        let store = state.store.read().await;
        if let Err(error) = queue_site_block_sync(&store) {
            warn!(%error, "could not queue the DNS block list during startup");
        }
    }

    tokio::spawn(session_enforcement_loop(state.clone()));
    tokio::spawn(gateway_startup_reconcile(state.clone()));
    tokio::spawn(coin_pulse_listener(state.clone()));

    let api = Router::new()
        .route("/health", get(health))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/auth/me", get(auth_me))
        .route("/overview", get(overview))
        .route("/business/summary", get(business_summary))
        .route("/system", get(system_status))
        .route("/network/interfaces", get(network_interfaces))
        .route("/network/discovery", get(network_discovery))
        .route("/network/plan", post(network_plan))
        .route("/router/status", get(router_status))
        .route("/router/apply", post(router_apply))
        .route("/gateway/reconcile", post(gateway_reconcile))
        .route("/diagnostics", get(diagnostics))
        .route("/operations/metrics", get(operations_metrics))
        .route("/audit-events", get(list_audit_events))
        .route("/backup", get(download_backup))
        .route("/backup/verify", post(verify_backup))
        .route("/backup/restore", post(restore_backup))
        .route("/portal/purchase", post(portal_purchase))
        .route("/portal/free", post(portal_free_claim))
        .route("/portal/coin/status", get(portal_coin_status))
        .route("/portal/coin/cancel", post(portal_coin_cancel))
        .route("/coin-node/status", get(coin_node_status))
        .route("/coin-node/heartbeat", post(coin_node_heartbeat))
        .route("/coin-node/pulse", post(coin_node_pulse))
        .route("/coin-nodes", get(list_coin_nodes).post(pair_coin_node))
        .route(
            "/coin-nodes/{id}",
            put(update_coin_node).delete(delete_coin_node),
        )
        .route("/portal/status", get(portal_status))
        .route("/portal/session/{action}", post(portal_session_action))
        .route("/session/heartbeat", post(session_heartbeat))
        .route("/rates", get(list_rates).post(create_rate))
        .route("/rates/{id}", put(update_rate).delete(delete_rate))
        .route("/vouchers", get(list_vouchers))
        .route("/vouchers/generate", post(generate_vouchers))
        .route("/vouchers/redeem", post(redeem_voucher))
        .route("/vouchers/{id}", delete(delete_voucher))
        .route("/transactions", get(list_transactions))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", put(update_session))
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
        .route("/portal/fas", get(portal_fas).post(portal_fas_redeem))
        .route("/portal/fas/free", post(portal_fas_free))
        .fallback_service(ServeDir::new("web").append_index_html_on_directories(true))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
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
        .query_row("SELECT payload FROM app_state WHERE id = 1", [], |row| {
            row.get(0)
        })
        .ok();
    let mut store = stored
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| {
            fs::read_to_string(legacy_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
        })
        .unwrap_or_else(|| {
            info!("creating a clean production SQLite database");
            Store::production()
        });
    remove_legacy_demo_data(&mut store);
    // Schema-v1 sessions were already decrementing their persisted balance,
    // but had no accounting checkpoint. Start their v2 checkpoint at upgrade
    // time to avoid deducting the same historical runtime twice.
    let migration_time = Utc::now();
    for session in &mut store.sessions {
        if session.last_accounted_at.is_none() {
            session.last_accounted_at = Some(migration_time);
        }
        if session.status == SessionStatus::Paused && session.paused_at.is_none() {
            session.paused_at = Some(migration_time);
        }
    }
    let raw = serde_json::to_string(&store).expect("serialize initial state");
    connection.execute(
        "INSERT INTO app_state (id, schema_version, payload, updated_at) VALUES (1, 2, ?1, ?2) ON CONFLICT(id) DO UPDATE SET schema_version=2, payload=excluded.payload, updated_at=excluded.updated_at",
        params![raw, Utc::now().to_rfc3339()],
    ).expect("seed SQLite state");
    store
}

/// Early development builds seeded dashboards with fictional 10.10.0.x
/// clients. Remove only those exact fixtures during upgrade; real VLAN 799
/// customers use 10.0.0.0/20 and are never matched here.
fn remove_legacy_demo_data(store: &mut Store) {
    let sessions_before = store.sessions.len();
    let transactions_before = store.transactions.len();
    let blocked_before = store.blocked_sites.len();
    store.sessions.retain(|session| {
        !(session.ip.starts_with("10.10.0.")
            && matches!(
                session.client_name.as_str(),
                "realme C55" | "Android phone" | "Juan's laptop"
            ))
    });
    store.transactions.retain(|transaction| {
        !(transaction.client_ip.starts_with("10.10.0.")
            && transaction.station == "Main vendo"
            && transaction.mac.starts_with("A4:55:90:10:"))
    });
    store
        .blocked_sites
        .retain(|site| !(site.host == "example-blocked.test" && site.note == "Demo rule"));
    let removed = sessions_before - store.sessions.len() + transactions_before
        - store.transactions.len()
        + blocked_before
        - store.blocked_sites.len();
    if removed > 0 {
        warn!(removed, "removed legacy fictional dashboard records");
    }
}

async fn persist(state: &AppState) -> Result<(), String> {
    let store = state.store.read().await;
    let raw = serde_json::to_string(&*store).map_err(|e| e.to_string())?;
    let connection = Connection::open(&state.database_file).map_err(|e| e.to_string())?;
    connection.execute(
        "INSERT INTO app_state (id, schema_version, payload, updated_at) VALUES (1, 2, ?1, ?2) ON CONFLICT(id) DO UPDATE SET schema_version=2, payload=excluded.payload, updated_at=excluded.updated_at",
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
            if record
                .locked_until
                .is_some_and(|until| until > Instant::now())
            {
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
    state.sessions.write().await.insert(
        session.clone(),
        AuthSession {
            csrf: csrf.clone(),
            expires_at: now + std::time::Duration::from_secs(8 * 60 * 60),
            last_seen_at: now,
        },
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "chasselfi_session={session}; Path=/; Max-Age=28800; HttpOnly; SameSite=Strict{}",
            if env_compat("CHASSELFI_SECURE_COOKIES", "BANTAY_SECURE_COOKIES").as_deref()
                == Some("1")
            {
                "; Secure"
            } else {
                ""
            }
        ))
        .expect("valid session cookie"),
    );
    Ok((
        headers,
        Json(json!({ "username": state.admin_username, "csrfToken": csrf })),
    ))
}

async fn logout(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
) -> (HeaderMap, Json<Value>) {
    if let Some(token) = cookie_value(request.headers(), "chasselfi_session")
        .or_else(|| cookie_value(request.headers(), "bantay_session"))
    {
        state.sessions.write().await.remove(&token);
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "chasselfi_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict",
        ),
    );
    (headers, Json(json!({ "loggedOut": true })))
}

async fn auth_me(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let token = cookie_value(request.headers(), "chasselfi_session")
        .or_else(|| cookie_value(request.headers(), "bantay_session"))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Login required" })),
            )
        })?;
    let csrf = session_csrf(&state, &token).await.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Login required" })),
        )
    })?;
    Ok(Json(
        json!({ "username": state.admin_username, "csrfToken": csrf }),
    ))
}

async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/api")
        || path.starts_with("/api/coin-node/")
        || matches!(path, "/api/health" | "/api/login" | "/api/logout")
        || (request.method() == Method::GET
            && matches!(
                path,
                "/api/rates" | "/api/settings" | "/api/portal/status" | "/api/portal/coin/status"
            ))
        || (request.method() == Method::POST
            && matches!(
                path,
                "/api/vouchers/redeem"
                    | "/api/portal/purchase"
                    | "/api/portal/coin/cancel"
                    | "/api/portal/session/pause"
                    | "/api/portal/session/resume"
                    | "/api/session/heartbeat"
            ))
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
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Login required" })),
        )
            .into_response();
    }
    if request.method() != Method::GET
        && request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            != csrf.as_deref()
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "CSRF token missing or invalid" })),
        )
            .into_response();
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
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: https:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self' http://10.0.0.1:2050",
        ),
    );
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "x-permitted-cross-domain-policies",
        HeaderValue::from_static("none"),
    );
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
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
    let salt =
        SaltString::encode_b64(Uuid::new_v4().as_bytes()).map_err(|error| error.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
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

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "chasselfi",
        "hardwareMode": if state.hardware_mode == HardwareMode::Linux { "linux" } else { "simulated" }
    }))
}

fn append_audit(
    store: &mut Store,
    category: &str,
    action: &str,
    actor: &str,
    subject: impl Into<String>,
    detail: impl Into<String>,
) {
    let now = Utc::now();
    let cutoff = now - Duration::days(i64::from(store.settings.audit_retention_days.max(1)));
    store.audit_events.retain(|event| event.timestamp >= cutoff);
    if store.audit_events.len() >= 20_000 {
        let remove = store.audit_events.len() - 19_999;
        store.audit_events.drain(0..remove);
    }
    store.audit_events.push(AuditEvent {
        id: Uuid::new_v4(),
        timestamp: now,
        category: category.into(),
        action: action.into(),
        actor: actor.into(),
        subject: subject.into(),
        detail: detail.into(),
    });
}

fn account_online_session(session: &mut Session, now: DateTime<Utc>) -> bool {
    let last = session.last_accounted_at.unwrap_or(session.started_at);
    let elapsed = now.signed_duration_since(last).num_seconds().max(0);
    session.last_accounted_at = Some(now);
    if elapsed == 0 || session.status != SessionStatus::Online {
        return false;
    }
    session.remaining_seconds = session.remaining_seconds.saturating_sub(elapsed).max(0);
    if session.remaining_seconds == 0 {
        session.status = SessionStatus::Ended;
        return true;
    }
    false
}

async fn gateway_startup_reconcile(state: AppState) {
    if state.hardware_mode != HardwareMode::Linux {
        return;
    }
    tokio::time::sleep(TokioDuration::from_secs(3)).await;
    let (_, errors) = reconcile_gateway(&state).await;
    for error in errors {
        warn!(%error, "gateway startup reconciliation failed");
    }
}

async fn reconcile_gateway(state: &AppState) -> (usize, Vec<String>) {
    let sessions = state.store.read().await.sessions.clone();
    let mut reconciled = 0;
    let mut errors = Vec::new();
    for session in sessions {
        let result = if session.status == SessionStatus::Online && session.remaining_seconds > 0 {
            router::opennds_authorize(
                &session.ip,
                &session.mac,
                ((session.remaining_seconds + 59) / 60) as u32,
                session.download_mbps.max(1.0).round() as u32,
                session.upload_mbps.max(1.0).round() as u32,
            )
            .await
        } else {
            router::opennds_deauthorize(&session.ip, &session.mac).await
        };
        if let Err(error) = result {
            errors.push(format!("session {} ({}): {error}", session.id, session.ip));
        } else {
            reconciled += 1;
        }
    }
    (reconciled, errors)
}

async fn session_enforcement_loop(state: AppState) {
    let mut ticker = interval(TokioDuration::from_secs(30));
    loop {
        ticker.tick().await;
        let mut changed = false;
        let mut deauthorize_clients = Vec::new();
        let mut authorize_clients = Vec::new();
        {
            let mut store = state.store.write().await;
            let now = Utc::now();
            let settings = store.settings.clone();
            for session in &mut store.sessions {
                if session.status == SessionStatus::Online {
                    if account_online_session(session, now) {
                        deauthorize_clients.push((session.ip.clone(), session.mac.clone()));
                    } else if settings.auto_pause_on_disconnect
                        && settings.inactivity_pause_minutes > 0
                        && session.last_seen_at.is_some_and(|last_seen| {
                            now.signed_duration_since(last_seen).num_minutes()
                                >= i64::from(settings.inactivity_pause_minutes)
                        })
                    {
                        session.status = SessionStatus::Paused;
                        session.paused_at = Some(now);
                        session.pause_count = session.pause_count.saturating_add(1);
                        deauthorize_clients.push((session.ip.clone(), session.mac.clone()));
                    }
                    changed = true;
                } else if session.status == SessionStatus::Paused
                    && settings.max_pause_minutes > 0
                    && session.paused_at.is_some_and(|paused_at| {
                        now.signed_duration_since(paused_at).num_minutes()
                            >= i64::from(settings.max_pause_minutes)
                    })
                {
                    if let Some(paused_at) = session.paused_at.take() {
                        session.total_paused_seconds = session.total_paused_seconds.saturating_add(
                            now.signed_duration_since(paused_at).num_seconds().max(0),
                        );
                    }
                    session.status = SessionStatus::Online;
                    session.last_accounted_at = Some(now);
                    authorize_clients.push((
                        session.ip.clone(),
                        session.mac.clone(),
                        ((session.remaining_seconds.max(1) + 59) / 60) as u32,
                        session.download_mbps.max(1.0).round() as u32,
                        session.upload_mbps.max(1.0).round() as u32,
                    ));
                    changed = true;
                }
            }
        }
        if changed {
            if let Err(error) = persist(&state).await {
                warn!(%error, "could not persist enforced session state");
            }
        }
        for (client_ip, client_mac) in deauthorize_clients {
            if let Err(error) = router::opennds_deauthorize(&client_ip, &client_mac).await {
                warn!(%client_ip, %error, "could not deauthorize expired openNDS client");
            }
        }
        for (client_ip, client_mac, minutes, down, up) in authorize_clients {
            if let Err(error) =
                router::opennds_authorize(&client_ip, &client_mac, minutes, down, up).await
            {
                warn!(%client_ip, %error, "could not resume a session after its maximum pause");
            }
        }
    }
}

async fn router_status(State(state): State<AppState>) -> Json<router::RouterStatus> {
    Json(router::status(&state.hardware_mode).await)
}

async fn gateway_reconcile(State(state): State<AppState>) -> ApiResult<Value> {
    if state.hardware_mode != HardwareMode::Linux {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Gateway reconciliation requires Linux hardware mode"})),
        ));
    }
    let (reconciled, errors) = reconcile_gateway(&state).await;
    let mut store = state.store.write().await;
    append_audit(
        &mut store,
        "gateway",
        "reconcile",
        "admin",
        format!("{reconciled} sessions"),
        if errors.is_empty() {
            "ok".into()
        } else {
            errors.join("; ")
        },
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    if errors.is_empty() {
        Ok(Json(json!({"ok": true, "reconciled": reconciled})))
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"error":"Gateway reconciliation was incomplete", "reconciled":reconciled, "failures":errors}),
            ),
        ))
    }
}

async fn command_diagnostic(program: &str, args: &[&str]) -> Value {
    let result = tokio::time::timeout(
        TokioDuration::from_secs(4),
        tokio::process::Command::new(program).args(args).output(),
    )
    .await;
    match result {
        Ok(Ok(output)) => json!({
            "ok": output.status.success(),
            "status": output.status.code(),
            "output": String::from_utf8_lossy(if output.stdout.is_empty() { &output.stderr } else { &output.stdout }).trim().chars().take(8000).collect::<String>()
        }),
        Ok(Err(error)) => json!({"ok":false,"output":error.to_string()}),
        Err(_) => json!({"ok":false,"output":"command timed out"}),
    }
}

async fn diagnostics(State(state): State<AppState>) -> Json<Value> {
    if state.hardware_mode != HardwareMode::Linux {
        return Json(json!({"hardwareMode":"simulated","healthy":true,"checks":[]}));
    }
    let services = ["chasselfi", "nginx", "dnsmasq", "nftables", "opennds"];
    let mut checks = Vec::new();
    for service in services {
        let probe = command_diagnostic("systemctl", &["is-active", service]).await;
        checks.push(
            json!({"name":service,"kind":"service","ok":probe["ok"],"detail":probe["output"]}),
        );
    }
    let nds = command_diagnostic("ndsctl", &["status"]).await;
    checks.push(
        json!({"name":"openNDS control","kind":"gateway","ok":nds["ok"],"detail":nds["output"]}),
    );
    let nft = command_diagnostic("test", &["-r", "/etc/nftables.d/chasselfi.nft"]).await;
    checks.push(json!({
        "name":"ChasselFi nftables policy",
        "kind":"network",
        "ok":nft["ok"],
        "detail":if nft["ok"].as_bool()==Some(true){"policy file installed"}else{"/etc/nftables.d/chasselfi.nft is not readable"}
    }));
    let qdisc = command_diagnostic("tc", &["qdisc", "show"]).await;
    let cake_active = qdisc["ok"].as_bool() == Some(true)
        && qdisc["output"]
            .as_str()
            .is_some_and(|output| output.contains("cake"));
    checks.push(json!({"name":"CAKE traffic control","kind":"network","ok":cake_active,"detail":qdisc["output"]}));
    let gateway_clients = router::opennds_clients().await;
    checks.push(match gateway_clients {
        Ok(clients) => json!({
            "name":"Live captive clients",
            "kind":"gateway",
            "ok":true,
            "detail":format!("{} visible; {} authenticated", clients.len(), clients.iter().filter(|client| client.state.eq_ignore_ascii_case("authenticated")).count())
        }),
        Err(error) => json!({"name":"Live captive clients","kind":"gateway","ok":false,"detail":error}),
    });
    let healthy = checks
        .iter()
        .all(|check| check["ok"].as_bool().unwrap_or(false));
    Json(json!({"hardwareMode":"linux","healthy":healthy,"checkedAt":Utc::now(),"checks":checks}))
}

async fn operations_metrics(State(state): State<AppState>) -> Json<Value> {
    let store = state.store.read().await;
    let now = Utc::now();
    let last_day = now - Duration::hours(24);
    let revenue_24h: u32 = store
        .transactions
        .iter()
        .filter(|tx| tx.created_at >= last_day)
        .map(|tx| tx.amount)
        .sum();
    let expired = store
        .sessions
        .iter()
        .filter(|session| session.status == SessionStatus::Ended)
        .count();
    let paused = store
        .sessions
        .iter()
        .filter(|session| session.status == SessionStatus::Paused)
        .count();
    let coin = state.coin.read().await;
    Json(json!({
        "generatedAt": now,
        "revenue24h": revenue_24h,
        "transactions24h": store.transactions.iter().filter(|tx| tx.created_at >= last_day).count(),
        "activeSessions": store.sessions.iter().filter(|session| session.status == SessionStatus::Online).count(),
        "pausedSessions": paused,
        "endedSessions": expired,
        "readyVouchers": store.vouchers.iter().filter(|voucher| voucher.status == VoucherStatus::Ready).count(),
        "auditEvents24h": store.audit_events.iter().filter(|event| event.timestamp >= last_day).count(),
        "coinNodesOnline": coin.nodes.iter().filter(|(id, node)| {
            node.last_seen_at >= now - Duration::seconds(45)
                && store.coin_nodes.iter().any(|profile| profile.id == **id && !profile.disabled)
        }).count(),
        "unpairedCoinSocketReady": coin.socket_ready
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    category: Option<String>,
    limit: Option<usize>,
}

async fn list_audit_events(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Json<Vec<AuditEvent>> {
    let store = state.store.read().await;
    let limit = query.limit.unwrap_or(250).clamp(1, 2000);
    let mut events = store
        .audit_events
        .iter()
        .rev()
        .filter(|event| {
            query
                .category
                .as_ref()
                .is_none_or(|category| event.category.eq_ignore_ascii_case(category))
        })
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    events.shrink_to_fit();
    Json(events)
}

async fn router_apply(
    State(state): State<AppState>,
    Json(request): Json<router::ShapeRequest>,
) -> ApiResult<router::RouterPlan> {
    let plan = router::apply(&state.hardware_mode, request)
        .await
        .map_err(bad_request)?;
    if plan.applied {
        let mut store = state.store.write().await;
        append_audit(
            &mut store,
            "network",
            "shape",
            "admin",
            "CAKE",
            plan.message.clone(),
        );
        drop(store);
        persist(&state).await.map_err(bad_request)?;
    }
    Ok(Json(plan))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FasContext {
    hid: String,
    client_ip: String,
    client_mac: String,
    client_if: String,
    auth_action: String,
    origin_url: String,
}

#[derive(Deserialize)]
struct FasRedeemForm {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct FasFreeTimeForm {
    state: String,
    accepted_terms: Option<String>,
}

type HmacSha256 = Hmac<Sha256>;

fn fas_key() -> Result<String, String> {
    std::env::var("CHASSELFI_FAS_KEY")
        .or_else(|_| std::env::var("BANTAY_FAS_KEY"))
        .map_err(|_| "CHASSELFI_FAS_KEY is not configured".into())
}

fn sign_fas_state(context: &FasContext, key: &str) -> Result<String, String> {
    let payload = serde_json::to_vec(context).map_err(|error| error.to_string())?;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(encoded.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{encoded}.{signature}"))
}

fn verify_fas_state(value: &str, key: &str) -> Result<FasContext, String> {
    let (encoded, signature) = value
        .split_once('.')
        .ok_or_else(|| "Invalid portal state".to_string())?;
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(encoded.as_bytes());
    let supplied = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "Invalid portal state signature".to_string())?;
    mac.verify_slice(&supplied)
        .map_err(|_| "Invalid or expired portal state".to_string())?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid portal state payload".to_string())?;
    serde_json::from_slice(&payload).map_err(|_| "Invalid portal state payload".to_string())
}

fn parse_fas_context(query: &HashMap<String, String>) -> Result<FasContext, String> {
    let encoded = query
        .get("fas")
        .ok_or_else(|| "Missing openNDS FAS context".to_string())?;
    // Form URL decoding can turn a literal `+` from standard base64 into a
    // space. Restore it before decoding the context supplied by openNDS.
    let normalized = encoded.replace(' ', "+");
    let decoded = BASE64_STANDARD
        .decode(&normalized)
        .or_else(|_| URL_SAFE_NO_PAD.decode(&normalized))
        .map_err(|_| "Could not decode openNDS FAS context".to_string())?;
    let values =
        String::from_utf8(decoded).map_err(|_| "Invalid openNDS FAS context".to_string())?;
    let mut fields = HashMap::new();
    for item in values.split(", ").flat_map(|part| part.split('&')) {
        if let Some((name, value)) = item.split_once('=') {
            fields.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let hid = fields
        .get("hid")
        .or_else(|| fields.get("client_hid"))
        .cloned()
        .ok_or_else(|| "openNDS did not provide a hashed client token".to_string())?;
    let client_ip = fields
        .get("clientip")
        .cloned()
        .unwrap_or_else(|| "0.0.0.0".into());
    let client_mac = fields
        .get("clientmac")
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    let client_if = fields
        .get("clientif")
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    let gateway_address = fields
        .get("gatewayaddress")
        .cloned()
        .ok_or_else(|| "openNDS did not provide a gateway address".to_string())?;
    let auth_dir = fields
        .get("authdir")
        .cloned()
        .unwrap_or_else(|| "/opennds_auth/".into());
    let auth_action = fields
        .get("authaction")
        .cloned()
        .unwrap_or_else(|| format!("http://{}{}", gateway_address, auth_dir));
    if !(auth_action.starts_with("http://") || auth_action.starts_with("https://")) {
        return Err("Invalid openNDS authentication endpoint".into());
    }
    Ok(FasContext {
        hid,
        client_ip,
        client_mac,
        client_if,
        auth_action,
        origin_url: fields
            .get("originurl")
            .cloned()
            .unwrap_or_else(|| "http://example.com/".into()),
    })
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn opennds_connect_page(
    context: &FasContext,
    key: &str,
    minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
    eyebrow: &str,
    heading: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(context.hid.as_bytes());
    digest.update(key.as_bytes());
    let token = format!("{:x}", digest.finalize());
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="theme-color" content="#06100c"><title>Connecting - ChasselFi</title><link rel="stylesheet" href="/vendor/bootstrap.min.css"><link rel="stylesheet" href="/styles.css"><script src="/fas-connect.js" defer></script></head><body class="portal-body fas-body" data-bs-theme="dark"><main class="portal-shell container"><section class="portal-card fas-result"><span class="brand-mark">C</span><span class="eyebrow">{}</span><h1>{}</h1><p>Your {} session is ready. Keep this window open for a moment.</p><div class="fas-loader"><i></i></div><form id="auth" method="get" action="{}"><input type="hidden" name="tok" value="{}"><input type="hidden" name="redir" value="http://10.0.0.1/"><input type="hidden" name="sessionlength" value="{}"><input type="hidden" name="downloadrate" value="{}"><input type="hidden" name="uploadrate" value="{}"><input type="hidden" name="custom" value="chasselfi"><button class="btn primary-btn portal-cta" type="submit">Continue now</button></form></section></main></body></html>"##,
        html_escape(eyebrow),
        html_escape(heading),
        human_minutes(minutes),
        html_escape(&context.auth_action),
        token,
        minutes,
        download_mbps.saturating_mul(1000),
        upload_mbps.saturating_mul(1000)
    )
}

async fn portal_fas(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let key = match fas_key() {
        Ok(key) => key,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, Html(error)).into_response(),
    };
    if query
        .get("status")
        .is_some_and(|status| status == "authenticated")
    {
        return Html(portal_message_page(
            "You're connected",
            "Your paid session is active. Open the customer portal to see your remaining time or add another voucher.",
            Some("http://10.0.0.1/"),
            "Open customer portal",
        ))
        .into_response();
    }
    let context = match parse_fas_context(&query) {
        Ok(context) => context,
        Err(error) => return (StatusCode::BAD_REQUEST, Html(error)).into_response(),
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let device_name = if user_agent.contains("Android") {
        "Android device"
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        "Apple mobile device"
    } else if user_agent.contains("Windows") {
        "Windows device"
    } else if user_agent.contains("Macintosh") {
        "Mac device"
    } else {
        "Connected device"
    };
    if client_key(&headers) != context.client_ip {
        return (
            StatusCode::FORBIDDEN,
            Html("The captive portal request does not match this client"),
        )
            .into_response();
    }
    // openNDS loses its in-memory authorization list when it restarts. The
    // ChasselFi database remains authoritative, so reconnect a client that
    // still has paid time without consuming another voucher.
    let existing_session = {
        let store = state.store.read().await;
        store
            .sessions
            .iter()
            .find(|session| {
                session.ip == context.client_ip
                    && session.mac.eq_ignore_ascii_case(&context.client_mac)
                    && session.status == SessionStatus::Online
                    && session.remaining_seconds > 0
            })
            .map(|session| {
                (
                    ((session.remaining_seconds + 59) / 60).max(1) as u32,
                    session.download_mbps.max(1.0).round() as u32,
                    session.upload_mbps.max(1.0).round() as u32,
                )
            })
    };
    if let Some((minutes, download_mbps, upload_mbps)) = existing_session {
        return Html(opennds_connect_page(
            &context,
            &key,
            minutes,
            download_mbps,
            upload_mbps,
            "PAID SESSION RESTORED",
            "Welcome back. Reconnecting...",
        ))
        .into_response();
    }
    let signed_state = match sign_fas_state(&context, &key) {
        Ok(state) => state,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, Html(error)).into_response(),
    };
    let store = state.store.read().await;
    let packages = store
        .rates
        .iter()
        .filter(|rate| rate.active)
        .map(|rate| {
            format!(
                "<article class=\"fas-rate\"><strong>₱{}</strong><span>{}</span><small>↓ {} Mbps · ↑ {} Mbps · {}</small></article>",
                rate.price,
                human_minutes(rate.minutes),
                rate.download_mbps,
                rate.upload_mbps,
                html_escape(&rate.label)
            )
        })
        .collect::<String>();
    let shop_name = html_escape(&store.settings.shop_name);
    let portal_message = html_escape(&store.settings.portal_message);
    let portal_eyebrow = html_escape(&store.settings.portal_eyebrow);
    let portal_headline = html_escape(&store.settings.portal_headline);
    let portal_status_label = html_escape(&store.settings.portal_status_label);
    let portal_rates_label = html_escape(&store.settings.portal_rates_label);
    let portal_voucher_label = html_escape(&store.settings.portal_voucher_label);
    let portal_coin_label = html_escape(&store.settings.portal_coin_label);
    let portal_free_label = html_escape(&store.settings.portal_free_label);
    let portal_banner_image = html_escape(&store.settings.portal_banner_image);
    let portal_logo_image = html_escape(&store.settings.portal_logo_image);
    let portal_show_device = store.settings.portal_show_device;
    let payment_mode = store.settings.payment_mode;
    let free_time_enabled = store.settings.free_time_enabled;
    let free_time_minutes = store.settings.free_time_minutes;
    let require_terms = store.settings.require_terms;
    let terms_title = html_escape(&store.settings.terms_title);
    let terms_body = html_escape(&store.settings.terms_body);
    let portal_accent = html_escape(&store.settings.portal_accent);
    let portal_template = html_escape(&store.settings.portal_template);
    drop(store);
    let voucher_form = if payment_mode.allows_voucher() {
        format!(
            r##"<form method="post" action="/portal/fas" class="voucher-entry fas-form portal-method">
<div class="method-icon">V</div><div><span class="eyebrow">VOUCHER ACCESS</span><h2>{portal_voucher_label}</h2><p>Enter a new eight-character code from the operator.</p></div>
<label class="form-label" for="voucher-code">Voucher code</label><input id="voucher-code" class="form-control form-control-lg" type="text" name="code" maxlength="8" minlength="8" placeholder="AB12CD34" required autocomplete="one-time-code">
<input type="hidden" name="state" value="{}"><button class="btn primary-btn portal-cta" type="submit">Connect now <span>→</span></button>
</form>"##,
            html_escape(&signed_state)
        )
    } else {
        String::new()
    };
    let coin_form = if payment_mode.allows_coin() {
        format!(
            r##"<section class="voucher-entry fas-form portal-method"><div class="method-icon">₱</div><div><span class="eyebrow">COIN ACCESS</span><h2>{portal_coin_label}</h2><p>Choose a package and wait for the physical coin node to show READY.</p></div><a class="btn secondary-btn portal-cta" href="http://10.0.0.1/portal.html#coin">Open coin mode <span>→</span></a></section>"##
        )
    } else {
        String::new()
    };
    let free_form = if free_time_enabled {
        let action = if require_terms {
            r##"<button class="btn free-claim-btn portal-cta" type="button" onclick="document.getElementById('free-terms-dialog').showModal()">Read terms &amp; claim <span>→</span></button>"##.to_string()
        } else {
            format!(
                r##"<form method="post" action="/portal/fas/free"><input type="hidden" name="state" value="{}"><button class="btn free-claim-btn portal-cta" type="submit">{} <span>→</span></button></form>"##,
                html_escape(&signed_state),
                portal_free_label
            )
        };
        format!(
            r##"<section class="voucher-entry fas-form portal-method free-method"><div class="free-ribbon">FREE ACCESS</div><div class="method-icon">✦</div><div><span class="eyebrow">COMPLIMENTARY SESSION</span><h2>{portal_free_label}</h2><p><strong>{}</strong> for each eligible device. No voucher or coin required.</p></div>{action}</section>"##,
            human_minutes(free_time_minutes)
        )
    } else {
        String::new()
    };
    let banner = if portal_banner_image.is_empty() {
        String::new()
    } else {
        format!(r##"<img class="portal-cover-image" src="{portal_banner_image}" alt="">"##)
    };
    let logo = if portal_logo_image.is_empty() {
        r##"<span class="brand-mark">C</span>"##.to_string()
    } else {
        format!(r##"<img class="portal-logo-image" src="{portal_logo_image}" alt="">"##)
    };
    let device_panel = if portal_show_device {
        format!(
            r##"<details class="device-details"><summary><span>Device</span><strong>{}</strong></summary><dl><div><dt>IP address</dt><dd>{}</dd></div><div><dt>MAC address</dt><dd>{}</dd></div><div><dt>Network</dt><dd>{}</dd></div><div><dt>Gateway</dt><dd>10.0.0.1</dd></div></dl></details>"##,
            html_escape(device_name),
            html_escape(&context.client_ip),
            html_escape(&context.client_mac),
            html_escape(&context.client_if)
        )
    } else {
        String::new()
    };
    let free_terms_dialog = if free_time_enabled && require_terms {
        format!(
            r##"<dialog id="free-terms-dialog" class="portal-dialog"><div class="dialog-head"><div><span class="eyebrow">FREE TIME TERMS</span><h2>{terms_title}</h2></div><button onclick="this.closest('dialog').close()" aria-label="Close">×</button></div><p class="terms-copy">{terms_body}</p><form method="post" action="/portal/fas/free"><input type="hidden" name="state" value="{}"><input type="hidden" name="accepted_terms" value="yes"><button class="btn free-claim-btn w-100" type="submit">Accept and claim {} <span>→</span></button></form></dialog>"##,
            html_escape(&signed_state),
            human_minutes(free_time_minutes)
        )
    } else {
        String::new()
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="theme-color" content="#06100c"><title>Connect · {shop_name}</title>
<link rel="stylesheet" href="/vendor/bootstrap.min.css"><link rel="stylesheet" href="/styles.css"></head>
<body class="portal-body fas-body template-{portal_template}" style="--portal-accent:{portal_accent}" data-bs-theme="dark"><main class="portal-shell portal-kiosk">
<section class="portal-card fas-card portal-kiosk-card"><div class="portal-cover">{banner}<div class="portal-cover-shade"></div><div class="portal-cover-brand">{logo}<span><small>{portal_eyebrow}</small><strong>{shop_name}</strong></span><i></i></div><div class="portal-cover-copy"><h1>{portal_headline}</h1><p>{portal_message}</p></div></div>
<div class="portal-kiosk-content"><div class="connection-banner"><span class="pulse-dot is-offline"></span><strong>{portal_status_label}</strong><small>Sign in required</small></div><section class="remaining-card"><small>TIME REMAINING</small><strong>-- : -- : --</strong><span>Connect to start your session</span></section>{device_panel}<button class="rate-modal-button rate-modal-wide" type="button" onclick="document.getElementById('rates-dialog').showModal()">{portal_rates_label} <span>↗</span></button>
<div class="fas-payment-options">{free_form}{coin_form}{voucher_form}</div></div></section><p class="portal-help">Session time and speed are enforced by the gateway for this device.</p>
<dialog id="rates-dialog" class="portal-dialog"><div class="dialog-head"><div><span class="eyebrow">TIME PACKAGES</span><h2>Choose what fits</h2></div><button onclick="this.closest('dialog').close()">×</button></div><div class="fas-rate-grid">{packages}</div></dialog>
{free_terms_dialog}
</main><script>document.querySelectorAll('dialog').forEach(d=>d.addEventListener('click',e=>{{if(e.target===d)d.close()}}))</script></body></html>"##,
    ))
    .into_response()
}

async fn portal_fas_redeem(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(input): Form<FasRedeemForm>,
) -> Response {
    let key = match fas_key() {
        Ok(key) => key,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, Html(error)).into_response(),
    };
    let context = match verify_fas_state(&input.state, &key) {
        Ok(context) => context,
        Err(error) => return (StatusCode::BAD_REQUEST, Html(error)).into_response(),
    };
    if client_key(&headers) != context.client_ip {
        return (
            StatusCode::FORBIDDEN,
            Html("The voucher can only be used by the requesting device"),
        )
            .into_response();
    }
    let (minutes, download_mbps, upload_mbps, _session) = match redeem_voucher_for_client(
        &state,
        &input.code,
        &context.client_ip,
        &context.client_mac,
        false,
    )
    .await
    {
        Ok(values) => values,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(portal_message_page(
                    "Voucher rejected",
                    &error,
                    None,
                    "Go back and try again",
                )),
            )
                .into_response()
        }
    };
    Html(opennds_connect_page(
        &context,
        &key,
        minutes,
        download_mbps,
        upload_mbps,
        "VOUCHER ACCEPTED",
        "Opening your internet...",
    ))
    .into_response()
}

async fn portal_fas_free(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(input): Form<FasFreeTimeForm>,
) -> Response {
    let key = match fas_key() {
        Ok(key) => key,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, Html(error)).into_response(),
    };
    let context = match verify_fas_state(&input.state, &key) {
        Ok(context) => context,
        Err(error) => return (StatusCode::BAD_REQUEST, Html(error)).into_response(),
    };
    if client_key(&headers) != context.client_ip {
        return (
            StatusCode::FORBIDDEN,
            Html("This free-time claim belongs to another device"),
        )
            .into_response();
    }

    let result: Result<(u32, u32, u32), String> = async {
        let mut store = state.store.write().await;
        let settings = store.settings.clone();
        if !settings.free_time_enabled {
            return Err("Free time is currently disabled".into());
        }
        if settings.require_terms && input.accepted_terms.as_deref() != Some("yes") {
            return Err("You must agree to the terms before claiming free time".into());
        }
        let device_key = if context.client_mac != "unknown" {
            context.client_mac.to_ascii_lowercase()
        } else {
            context.hid.clone()
        };
        let cutoff = Utc::now() - Duration::hours(i64::from(settings.free_time_reset_hours));
        if let Some(last) = store
            .free_time_claims
            .iter()
            .filter(|claim| claim.device_key == device_key)
            .max_by_key(|claim| claim.claimed_at)
        {
            if last.claimed_at > cutoff {
                let available =
                    last.claimed_at + Duration::hours(i64::from(settings.free_time_reset_hours));
                return Err(format!(
                    "Free time was already claimed by this device. Try again after {}",
                    available.format("%b %d, %I:%M %p UTC")
                ));
            }
        }
        let minutes = settings.free_time_minutes;
        if !(1..=1440).contains(&minutes) {
            return Err("The free-time duration is not configured correctly".into());
        }
        store.free_time_claims.push(FreeTimeClaim {
            id: Uuid::new_v4(),
            device_key: device_key.clone(),
            client_ip: context.client_ip.clone(),
            mac: context.client_mac.clone(),
            minutes,
            claimed_at: Utc::now(),
        });
        store.transactions.push(Transaction {
            id: Uuid::new_v4(),
            kind: "Free time".into(),
            amount: 0,
            minutes,
            client_ip: context.client_ip.clone(),
            mac: context.client_mac.clone(),
            station: "Customer portal".into(),
            created_at: Utc::now(),
        });
        upsert_session(
            &mut store,
            Some(device_key),
            &context.client_ip,
            &context.client_mac,
            minutes,
            settings.download_limit_mbps,
            settings.upload_limit_mbps,
        );
        Ok((
            minutes,
            settings.download_limit_mbps,
            settings.upload_limit_mbps,
        ))
    }
    .await;

    let (minutes, download_mbps, upload_mbps) = match result {
        Ok(result) => result,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(portal_message_page(
                    "Free time unavailable",
                    &error,
                    None,
                    "Return to portal",
                )),
            )
                .into_response()
        }
    };
    if let Err(error) = persist(&state).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Html(error)).into_response();
    }
    Html(opennds_connect_page(
        &context,
        &key,
        minutes,
        download_mbps,
        upload_mbps,
        "FREE TIME CLAIMED",
        "Your complimentary session is ready",
    ))
    .into_response()
}

fn human_minutes(minutes: u32) -> String {
    if minutes >= 1440 && minutes.is_multiple_of(1440) {
        format!(
            "{} day{}",
            minutes / 1440,
            if minutes == 1440 { "" } else { "s" }
        )
    } else if minutes >= 60 {
        let hours = minutes / 60;
        let remainder = minutes % 60;
        if remainder == 0 {
            format!("{} hour{}", hours, if hours == 1 { "" } else { "s" })
        } else {
            format!("{}h {}m", hours, remainder)
        }
    } else {
        format!("{} minutes", minutes)
    }
}

fn portal_message_page(title: &str, message: &str, href: Option<&str>, action: &str) -> String {
    let button = href
        .map(|target| format!("<a class=\"btn primary-btn portal-cta\" href=\"{}\">{}</a>", html_escape(target), html_escape(action)))
        .unwrap_or_else(|| format!("<a class=\"btn primary-btn portal-cta\" href=\"http://10.0.0.1/portal.html\">{}</a>", html_escape(action)));
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="theme-color" content="#06100c"><title>{}</title><link rel="stylesheet" href="/vendor/bootstrap.min.css"><link rel="stylesheet" href="/styles.css"></head><body class="portal-body fas-body" data-bs-theme="dark"><main class="portal-shell container"><section class="portal-card fas-result"><span class="brand-mark">C</span><span class="eyebrow">CHASSELFI WIFI</span><h1>{}</h1><p>{}</p>{}</section></main></body></html>"##,
        html_escape(title),
        html_escape(title),
        html_escape(message),
        button
    )
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupExport {
    schema_version: u32,
    created_at: String,
    #[serde(default)]
    checksum_sha256: String,
    store: Store,
}

fn store_checksum(store: &Store) -> Result<String, String> {
    let raw = serde_json::to_vec(store).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(raw)))
}

fn validate_backup(backup: &BackupExport) -> Result<(), String> {
    if !matches!(backup.schema_version, 1 | 2) {
        return Err("Unsupported backup schema version".into());
    }
    if backup.store.settings.shop_name.trim().is_empty() {
        return Err("Backup has an empty shop name".into());
    }
    if backup.schema_version >= 2 {
        let expected = store_checksum(&backup.store)?;
        if backup.checksum_sha256.is_empty() || backup.checksum_sha256 != expected {
            return Err(
                "Backup checksum does not match; the file may be damaged or modified".into(),
            );
        }
    }
    Ok(())
}

async fn download_backup(State(state): State<AppState>) -> Json<BackupExport> {
    let store = state.store.read().await.clone();
    let checksum_sha256 = store_checksum(&store).unwrap_or_default();
    Json(BackupExport {
        schema_version: 2,
        created_at: Utc::now().to_rfc3339(),
        checksum_sha256,
        store,
    })
}

async fn verify_backup(Json(backup): Json<BackupExport>) -> ApiResult<Value> {
    validate_backup(&backup).map_err(bad_request)?;
    Ok(Json(json!({
        "valid": true,
        "schemaVersion": backup.schema_version,
        "createdAt": backup.created_at,
        "checksumSha256": store_checksum(&backup.store).map_err(bad_request)?
    })))
}

async fn restore_backup(
    State(state): State<AppState>,
    Json(backup): Json<BackupExport>,
) -> ApiResult<Value> {
    validate_backup(&backup).map_err(bad_request)?;
    let current = state.store.read().await.clone();
    let recovery_dir = state
        .database_file
        .parent()
        .unwrap_or(FsPath::new("."))
        .join("recovery");
    fs::create_dir_all(&recovery_dir)
        .map_err(|error| bad_request(format!("Could not create recovery directory: {error}")))?;
    let recovery_path = recovery_dir.join(format!(
        "pre-restore-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let recovery_raw =
        serde_json::to_vec_pretty(&current).map_err(|error| bad_request(error.to_string()))?;
    fs::write(&recovery_path, recovery_raw)
        .map_err(|error| bad_request(format!("Could not write recovery snapshot: {error}")))?;
    let mut restored = backup.store;
    append_audit(
        &mut restored,
        "recovery",
        "restore",
        "admin",
        backup.created_at,
        format!("pre-restore snapshot={}", recovery_path.display()),
    );
    *state.store.write().await = restored;
    persist(&state).await.map_err(bad_request)?;
    if state.hardware_mode == HardwareMode::Linux {
        let _ = reconcile_gateway(&state).await;
    }
    Ok(Json(
        json!({ "restored": true, "recoverySnapshot": recovery_path }),
    ))
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
        "averageTransaction": total_sales.checked_div(transaction_count).unwrap_or(0),
        "coinSales": coin_sales,
        "voucherSales": voucher_sales,
        "readyInventoryValue": ready_inventory_value,
        "uniqueClients": unique_clients,
        "activeSessions": store.sessions.iter().filter(|session| session.status == SessionStatus::Online).count()
    }))
}

async fn overview(State(state): State<AppState>) -> Json<Overview> {
    let gateway_online = if state.hardware_mode == HardwareMode::Linux {
        router::opennds_clients().await.ok().map(|clients| {
            clients
                .iter()
                .filter(|client| client.state.eq_ignore_ascii_case("authenticated"))
                .count()
        })
    } else {
        None
    };
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
        online_users: gateway_online.unwrap_or_else(|| {
            store
                .sessions
                .iter()
                .filter(|s| s.status == SessionStatus::Online)
                .count()
        }),
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
    let saved_online = state
        .store
        .read()
        .await
        .sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Online)
        .count();
    let gateway_clients = if state.hardware_mode == HardwareMode::Linux {
        router::opennds_clients().await.ok()
    } else {
        None
    };
    let online = gateway_clients
        .as_ref()
        .map(|clients| {
            clients
                .iter()
                .filter(|client| client.state.eq_ignore_ascii_case("authenticated"))
                .count()
        })
        .unwrap_or(saved_online);
    let waiting_clients = gateway_clients
        .as_ref()
        .map(|clients| {
            clients
                .iter()
                .filter(|client| !client.state.eq_ignore_ascii_case("authenticated"))
                .count()
        })
        .unwrap_or(0);
    let now = Utc::now();
    let coin = state.coin.read().await;
    let online_nodes = coin
        .nodes
        .values()
        .filter(|node| node.last_seen_at > now - Duration::seconds(45))
        .count();
    let coin_slot_online = coin.socket_ready || online_nodes > 0;
    let coin_node_summary = coin
        .nodes
        .iter()
        .filter(|(_, node)| node.last_seen_at > now - Duration::seconds(45))
        .map(|(id, node)| {
            json!({
                "id": id,
                "ip": node.client_ip,
                "firmware": node.firmware,
                "lastSeenAt": node.last_seen_at
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "uptimeSeconds": state.started_at.elapsed().as_secs(),
        "cpuPercent": (cpu * 10.0).round() / 10.0,
        "memoryUsedMb": used_memory / 1024 / 1024,
        "memoryTotalMb": total_memory / 1024 / 1024,
        "onlineUsers": online,
        "waitingUsers": waiting_clients,
        "gatewayOnline": gateway_clients.is_some(),
        "serverOnline": true,
        "coinSlotOnline": coin_slot_online,
        "coinSlotMode": if online_nodes > 0 { "network-node" } else if coin.socket_ready { "local-socket" } else { "offline" },
        "coinNodes": coin_node_summary,
        "temperatureC": null,
        "hardwareMode": if state.hardware_mode == HardwareMode::Linux { "linux" } else { "simulated" }
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkInterfaceInfo {
    name: String,
    mac: Option<String>,
    state: String,
    rx_bytes: u64,
    tx_bytes: u64,
}

async fn network_interfaces() -> Json<Vec<NetworkInterfaceInfo>> {
    let mut interfaces = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Json(interfaces);
    };
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
        interfaces.push(NetworkInterfaceInfo {
            name,
            mac: read_text("address"),
            state: read_text("operstate").unwrap_or_else(|| "unknown".into()),
            rx_bytes: read_counter("statistics/rx_bytes"),
            tx_bytes: read_counter("statistics/tx_bytes"),
        });
    }
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    Json(interfaces)
}

async fn network_discovery() -> Json<network::DiscoveryResult> {
    Json(network::discover().await)
}

async fn network_plan(
    Json(input): Json<network::NetworkPlanRequest>,
) -> ApiResult<network::NetworkPlan> {
    network::plan(input).map(Json).map_err(bad_request)
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
    rate_id: Option<Uuid>,
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
    let selected_rate = if let Some(rate_id) = input.rate_id {
        Some(
            state
                .store
                .read()
                .await
                .rates
                .iter()
                .find(|rate| rate.id == rate_id && rate.active)
                .cloned()
                .ok_or_else(|| bad_request("Selected timer rate is missing or disabled"))?,
        )
    } else {
        None
    };
    let minutes = selected_rate
        .as_ref()
        .map(|rate| rate.minutes)
        .unwrap_or(input.minutes);
    let price = selected_rate
        .as_ref()
        .map(|rate| rate.price)
        .unwrap_or(input.price);
    let batch = batch_code();
    let now = Utc::now();
    let vouchers: Vec<_> = (0..input.quantity)
        .map(|_| Voucher {
            id: Uuid::new_v4(),
            rate_id: selected_rate.as_ref().map(|rate| rate.id),
            code: voucher_code(),
            minutes,
            price,
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

async fn gateway_client_mac(
    state: &AppState,
    client_ip: &str,
    simulated_fallback: Option<&str>,
) -> Result<String, (StatusCode, Json<Value>)> {
    if state.hardware_mode != HardwareMode::Linux {
        return Ok(simulated_fallback.unwrap_or(client_ip).to_string());
    }
    let clients = router::opennds_clients().await.map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": format!("The gateway client list is unavailable: {error}") })),
        )
    })?;
    clients
        .into_iter()
        .find(|client| client.ip == client_ip && client.mac.len() == 17)
        .map(|client| client.mac)
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "This device is not present in the customer VLAN neighbor table. Reconnect to WiFi and reopen the captive portal."
                })),
            )
        })
}

async fn redeem_voucher(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<RedeemInput>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    if client_ip == "unknown" {
        return Err(bad_request(
            "Could not identify this client through the gateway",
        ));
    }
    let client_mac = gateway_client_mac(&state, &client_ip, input.device_key.as_deref()).await?;
    let (minutes, _, _, session) =
        redeem_voucher_for_client(&state, &input.code, &client_ip, &client_mac, true)
            .await
            .map_err(bad_request)?;
    Ok(Json(
        json!({"redeemed": true, "code": input.code.trim().to_uppercase(), "minutes": minutes, "session": session}),
    ))
}

async fn redeem_voucher_for_client(
    state: &AppState,
    code: &str,
    client_ip: &str,
    client_mac: &str,
    sync_gateway: bool,
) -> Result<(u32, u32, u32, Value), String> {
    let mut store = state.store.write().await;
    if !store.settings.payment_mode.allows_voucher() {
        return Err("Voucher redemption is disabled by the operator".into());
    }
    let original = sync_gateway.then(|| store.clone());
    let index = store
        .vouchers
        .iter()
        .position(|v| v.code.eq_ignore_ascii_case(code.trim()))
        .ok_or_else(|| "Voucher code not found".to_string())?;
    if store.vouchers[index].status != VoucherStatus::Ready {
        return Err("This voucher is no longer available".into());
    }
    if store.vouchers[index]
        .expires_at
        .is_some_and(|expiry| expiry < Utc::now())
    {
        store.vouchers[index].status = VoucherStatus::Expired;
        return Err("This voucher has expired".into());
    }
    let (minutes, amount, rate_id) = {
        let voucher = &mut store.vouchers[index];
        voucher.status = VoucherStatus::Used;
        (voucher.minutes, voucher.price, voucher.rate_id)
    };
    store.transactions.push(Transaction {
        id: Uuid::new_v4(),
        kind: "Voucher".into(),
        amount,
        minutes,
        client_ip: client_ip.into(),
        mac: client_mac.into(),
        station: "Main vendo".into(),
        created_at: Utc::now(),
    });
    // Vouchers generated from a timer package inherit that package's speed.
    // Older/custom vouchers still use the configured per-user fallback.
    let matched_rate = store.rates.iter().find(|rate| {
        rate.active
            && rate_id
                .map(|id| rate.id == id)
                .unwrap_or(rate.minutes == minutes && rate.price == amount)
    });
    let download_limit = matched_rate
        .map(|rate| rate.download_mbps)
        .unwrap_or(store.settings.download_limit_mbps);
    let upload_limit = matched_rate
        .map(|rate| rate.upload_mbps)
        .unwrap_or(store.settings.upload_limit_mbps);
    let session = upsert_session(
        &mut store,
        Some(client_mac.into()),
        client_ip,
        client_mac,
        minutes,
        download_limit,
        upload_limit,
    );
    if sync_gateway {
        let remaining_seconds = session
            .get("remainingSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(i64::from(minutes) * 60)
            .max(1);
        let remaining_minutes = ((remaining_seconds + 59) / 60) as u32;
        if let Err(error) = router::opennds_authorize(
            client_ip,
            client_mac,
            remaining_minutes,
            download_limit,
            upload_limit,
        )
        .await
        {
            if let Some(original) = original {
                *store = original;
            }
            return Err(format!(
                "Voucher was not used because internet access could not be updated: {error}"
            ));
        }
    }
    drop(store);
    persist(state).await?;
    let session_value = session.clone();
    Ok((minutes, download_limit, upload_limit, session_value))
}

async fn list_transactions(State(state): State<AppState>) -> Json<Vec<Transaction>> {
    let mut items = state.store.read().await.transactions.clone();
    items.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));
    Json(items)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortalPurchaseInput {
    rate_id: Uuid,
    device_key: String,
}

async fn portal_purchase(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PortalPurchaseInput>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    if client_ip == "unknown" {
        return Err(bad_request(
            "Could not identify this client through the gateway",
        ));
    }
    if !(8..=80).contains(&input.device_key.len())
        || !input.device_key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        return Err(bad_request("Invalid device identifier"));
    }
    let client_mac = gateway_client_mac(&state, &client_ip, Some(&input.device_key)).await?;
    let (rate, payment_mode, enabled_nodes) = {
        let store = state.store.read().await;
        (
            store
                .rates
                .iter()
                .find(|rate| rate.id == input.rate_id && rate.active)
                .cloned()
                .ok_or_else(|| bad_request("That package is no longer available"))?,
            store.settings.payment_mode,
            store
                .coin_nodes
                .iter()
                .filter(|node| !node.disabled)
                .map(|node| node.id.clone())
                .collect::<std::collections::HashSet<_>>(),
        )
    };
    if !payment_mode.allows_coin() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Coin payments are disabled by the operator" })),
        ));
    }
    let now = Utc::now();
    let mut coin = state.coin.write().await;
    coin.nodes.retain(|id, node| {
        enabled_nodes.contains(id) && node.last_seen_at > now - Duration::seconds(45)
    });
    if !coin.socket_ready && coin.nodes.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "No authenticated coin node is online" })),
        ));
    }
    coin.completed
        .retain(|_, receipt| receipt.completed_at > now - Duration::minutes(10));
    if coin
        .active
        .as_ref()
        .is_some_and(|claim| claim.expires_at <= now)
    {
        coin.active = None;
        clear_coin_gate();
    }
    if let Some(claim) = coin.active.as_ref() {
        if claim.client_ip != client_ip {
            return Err((
                StatusCode::CONFLICT,
                Json(
                    json!({ "error": "The coin slot is currently being used by another customer" }),
                ),
            ));
        }
        if claim.rate.id != rate.id && claim.inserted_pesos > 0 {
            return Err((
                StatusCode::CONFLICT,
                Json(
                    json!({ "error": "Finish the current coin purchase before changing package" }),
                ),
            ));
        }
    }
    if let Some(claim) = coin.active.as_mut() {
        if claim.client_ip == client_ip && claim.rate.id == rate.id {
            claim.expires_at = now + Duration::minutes(5);
            let response = coin_claim_json(claim, "waiting");
            if claim.node_id.is_none() {
                if let Err(error) = write_coin_gate(claim) {
                    warn!(%error, "local coin gate is unavailable");
                }
            }
            return Ok(Json(response));
        }
    }
    let selected_node = coin
        .nodes
        .iter()
        .max_by_key(|(_, node)| node.last_seen_at)
        .map(|(id, _)| id.clone());
    let claim = CoinClaim {
        id: Uuid::new_v4(),
        client_ip,
        client_mac,
        device_key: input.device_key,
        node_id: selected_node,
        rate,
        inserted_pesos: 0,
        created_at: now,
        expires_at: now + Duration::minutes(5),
    };
    if claim.node_id.is_none() {
        if let Err(error) = write_coin_gate(&claim) {
            warn!(%error, "local coin gate is unavailable");
        }
    }
    let response = coin_claim_json(&claim, "waiting");
    coin.active = Some(claim);
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortalFreeInput {
    device_key: String,
    accepted_terms: bool,
}

async fn portal_free_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PortalFreeInput>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    if client_ip == "unknown" {
        return Err(bad_request(
            "Could not identify this client through the gateway",
        ));
    }
    if !(8..=80).contains(&input.device_key.len())
        || !input.device_key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        return Err(bad_request("Invalid device identifier"));
    }
    let client_mac = gateway_client_mac(&state, &client_ip, Some(&input.device_key)).await?;
    let device_key = if client_mac.len() == 17 {
        client_mac.to_ascii_lowercase()
    } else {
        input.device_key
    };
    let mut store = state.store.write().await;
    let original = store.clone();
    let settings = store.settings.clone();
    if !settings.free_time_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Free time is currently disabled" })),
        ));
    }
    if settings.require_terms && !input.accepted_terms {
        return Err(bad_request(
            "Read and accept the free-time terms before claiming",
        ));
    }
    let cutoff = Utc::now() - Duration::hours(i64::from(settings.free_time_reset_hours));
    if let Some(last) = store
        .free_time_claims
        .iter()
        .filter(|claim| claim.device_key == device_key)
        .max_by_key(|claim| claim.claimed_at)
    {
        if last.claimed_at > cutoff {
            let available =
                last.claimed_at + Duration::hours(i64::from(settings.free_time_reset_hours));
            return Err((
                StatusCode::CONFLICT,
                Json(json!({
                    "error": format!(
                        "Free time was already claimed by this device. Try again after {}",
                        available.format("%b %d, %I:%M %p UTC")
                    )
                })),
            ));
        }
    }
    let minutes = settings.free_time_minutes;
    if !(1..=1440).contains(&minutes) {
        return Err(bad_request(
            "The free-time duration is not configured correctly",
        ));
    }
    store.free_time_claims.push(FreeTimeClaim {
        id: Uuid::new_v4(),
        device_key: device_key.clone(),
        client_ip: client_ip.clone(),
        mac: client_mac.clone(),
        minutes,
        claimed_at: Utc::now(),
    });
    store.transactions.push(Transaction {
        id: Uuid::new_v4(),
        kind: "Free time".into(),
        amount: 0,
        minutes,
        client_ip: client_ip.clone(),
        mac: client_mac.clone(),
        station: "Customer portal".into(),
        created_at: Utc::now(),
    });
    let session = upsert_session(
        &mut store,
        Some(device_key),
        &client_ip,
        &client_mac,
        minutes,
        settings.download_limit_mbps,
        settings.upload_limit_mbps,
    );
    if let Err(error) = router::opennds_authorize(
        &client_ip,
        &client_mac,
        minutes,
        settings.download_limit_mbps,
        settings.upload_limit_mbps,
    )
    .await
    {
        *store = original;
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({ "error": format!("Free time was not consumed because the gateway rejected access: {error}") }),
            ),
        ));
    }
    let response = json!({ "claimed": true, "minutes": minutes, "session": session });
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinStatusQuery {
    claim_id: Uuid,
}

async fn portal_coin_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoinStatusQuery>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    let now = Utc::now();
    let mut coin = state.coin.write().await;
    if let Some(receipt) = coin.completed.get(&query.claim_id) {
        if receipt.client_ip != client_ip {
            return Err(not_found("Coin purchase not found"));
        }
        return Ok(Json(json!({
            "claimId": query.claim_id,
            "status": "completed",
            "session": receipt.session,
            "gatewayWarning": receipt.gateway_error
        })));
    }
    let Some(claim) = coin.active.as_ref() else {
        return Err(not_found("Coin purchase not found or expired"));
    };
    if claim.id != query.claim_id || claim.client_ip != client_ip {
        return Err(not_found("Coin purchase not found"));
    }
    if claim.expires_at <= now {
        coin.active = None;
        clear_coin_gate();
        return Err((
            StatusCode::GONE,
            Json(
                json!({ "error": "Coin purchase expired. Ask the operator if coins were already inserted." }),
            ),
        ));
    }
    Ok(Json(coin_claim_json(claim, "waiting")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinCancelInput {
    claim_id: Uuid,
}

async fn portal_coin_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CoinCancelInput>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    let mut coin = state.coin.write().await;
    let Some(claim) = coin.active.as_ref() else {
        return Err(not_found("Coin purchase not found"));
    };
    if claim.id != input.claim_id || claim.client_ip != client_ip {
        return Err(not_found("Coin purchase not found"));
    }
    if claim.inserted_pesos > 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "A purchase with inserted coins cannot be cancelled" })),
        ));
    }
    coin.active = None;
    clear_coin_gate();
    Ok(Json(json!({ "cancelled": true })))
}

fn coin_claim_json(claim: &CoinClaim, status: &str) -> Value {
    json!({
        "claimId": claim.id,
        "status": status,
        "insertedPesos": claim.inserted_pesos,
        "requiredPesos": claim.rate.price,
        "remainingPesos": claim.rate.price.saturating_sub(claim.inserted_pesos),
        "rate": claim.rate,
        "createdAt": claim.created_at,
        "expiresAt": claim.expires_at
    })
}

fn valid_coin_node_id(value: &str) -> bool {
    (3..=48).contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn coin_key_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn secure_key_match(expected: &str, provided: &str) -> bool {
    let message = b"chasselfi-network-coin-node";
    let mut expected_mac = Hmac::<Sha256>::new_from_slice(expected.as_bytes()).expect("HMAC key");
    expected_mac.update(message);
    let mut provided_mac = Hmac::<Sha256>::new_from_slice(provided.as_bytes()).expect("HMAC key");
    provided_mac.update(message);
    expected_mac
        .verify_slice(&provided_mac.finalize().into_bytes())
        .is_ok()
}

async fn coin_node_key_valid(
    state: &AppState,
    headers: &HeaderMap,
    node_id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let provided = headers
        .get("x-chasselfi-coin-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let paired_hash = state
        .store
        .read()
        .await
        .coin_nodes
        .iter()
        .find(|node| node.id == node_id)
        .map(|node| (node.key_hash.clone(), node.disabled));
    if paired_hash.as_ref().is_some_and(|(_, disabled)| *disabled) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Coin node is disabled" })),
        ));
    }
    let valid = if let Some((expected_hash, _)) = paired_hash {
        secure_key_match(&expected_hash, &coin_key_hash(provided))
    } else if let Ok(expected) = std::env::var("CHASSELFI_COIN_NODE_KEY") {
        expected.len() >= 16 && secure_key_match(&expected, provided)
    } else {
        false
    };
    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Coin node is not paired or its key is invalid" })),
        ));
    }
    Ok(())
}

fn coin_node_client_ip(headers: &HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
    let client_ip = client_key(headers);
    let allowed = client_ip.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| {
        let octets = ip.octets();
        octets[0] == 10 && octets[1] == 0 && octets[2] <= 15
    });
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Coin nodes may connect only from VLAN 799 (10.0.0.0/20)" })),
        ));
    }
    Ok(client_ip)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinNodeQuery {
    node_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinNodeHeartbeatInput {
    node_id: String,
    firmware: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinNodePulseInput {
    node_id: String,
    claim_id: Uuid,
    event_id: String,
    #[serde(default = "one_pulse")]
    count: u32,
    sequence: Option<u64>,
    timestamp: Option<i64>,
}

fn one_pulse() -> u32 {
    1
}

fn coin_node_view(coin: &CoinRuntime, node_id: &str) -> Value {
    let claim = coin
        .active
        .as_ref()
        .filter(|claim| claim.node_id.as_deref() == Some(node_id));
    json!({
        "ok": true,
        "nodeId": node_id,
        "ready": true,
        "accepting": claim.is_some(),
        "claim": claim.map(|claim| json!({
            "claimId": claim.id,
            "requiredPesos": claim.rate.price,
            "insertedPesos": claim.inserted_pesos,
            "remainingPesos": claim.rate.price.saturating_sub(claim.inserted_pesos),
            "expiresAt": claim.expires_at
        }))
    })
}

async fn register_coin_node(
    state: &AppState,
    headers: &HeaderMap,
    node_id: &str,
    firmware: Option<String>,
) -> Result<Value, (StatusCode, Json<Value>)> {
    if !valid_coin_node_id(node_id) {
        return Err(bad_request("Invalid coin node ID"));
    }
    coin_node_key_valid(state, headers, node_id).await?;
    let client_ip = coin_node_client_ip(headers)?;
    let mut coin = state.coin.write().await;
    coin.nodes.insert(
        node_id.to_string(),
        CoinNodeState {
            client_ip,
            last_seen_at: Utc::now(),
            firmware: firmware.filter(|value| value.len() <= 80),
        },
    );
    Ok(coin_node_view(&coin, node_id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairCoinNodeInput {
    name: String,
    node_id: Option<String>,
}

async fn list_coin_nodes(State(state): State<AppState>) -> Json<Value> {
    let profiles = state.store.read().await.coin_nodes.clone();
    let runtime = state.coin.read().await;
    let now = Utc::now();
    Json(json!(profiles
        .iter()
        .map(|profile| {
            let live = runtime.nodes.get(&profile.id);
            json!({
                "id": profile.id,
                "name": profile.name,
                "createdAt": profile.created_at,
                "online": !profile.disabled && live.is_some_and(|node| node.last_seen_at > now - Duration::seconds(45)),
                "clientIp": live.map(|node| node.client_ip.clone()),
                "lastSeenAt": live.map(|node| node.last_seen_at),
                "firmware": live.and_then(|node| node.firmware.clone())
                ,"disabled": profile.disabled,
                "lastSequence": profile.last_sequence,
                "acceptedPulses": profile.accepted_pulses
            })
        })
        .collect::<Vec<_>>()))
}

async fn pair_coin_node(
    State(state): State<AppState>,
    Json(input): Json<PairCoinNodeInput>,
) -> ApiResult<Value> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(bad_request(
            "Coin node name must be between 1 and 64 characters",
        ));
    }
    let node_id = input
        .node_id
        .unwrap_or_else(|| format!("vendo-{}", &Uuid::new_v4().simple().to_string()[..8]));
    if !valid_coin_node_id(&node_id) {
        return Err(bad_request(
            "Node ID must use 3-48 letters, numbers, dashes, or underscores",
        ));
    }
    let key: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();
    let mut store = state.store.write().await;
    if store.coin_nodes.iter().any(|node| node.id == node_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error":"That node ID already exists"})),
        ));
    }
    store.coin_nodes.push(CoinNodeProfile {
        id: node_id.clone(),
        name: name.into(),
        key_hash: coin_key_hash(&key),
        created_at: Utc::now(),
        disabled: false,
        last_sequence: 0,
        accepted_pulses: 0,
    });
    append_audit(
        &mut store,
        "hardware",
        "pair",
        "admin",
        node_id.clone(),
        format!("name={name}"),
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({
        "id": node_id,
        "name": name,
        "key": key,
        "heartbeatUrl": "http://10.0.0.1:2081/api/coin-node/heartbeat",
        "pulseUrl": "http://10.0.0.1:2081/api/coin-node/pulse",
        "note": "This key is shown once. Save it in the node firmware. The node can reach only ChasselFi on the customer LAN; pairing does not grant internet access."
    })))
}

async fn delete_coin_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let before = store.coin_nodes.len();
    store.coin_nodes.retain(|node| node.id != id);
    if store.coin_nodes.len() == before {
        return Err(not_found("Coin node not found"));
    }
    append_audit(
        &mut store,
        "hardware",
        "delete",
        "admin",
        id.clone(),
        "coin node removed",
    );
    drop(store);
    state.coin.write().await.nodes.remove(&id);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(json!({"deleted": true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCoinNodeInput {
    name: Option<String>,
    enabled: Option<bool>,
}

async fn update_coin_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<UpdateCoinNodeInput>,
) -> ApiResult<Value> {
    let mut store = state.store.write().await;
    let node = store
        .coin_nodes
        .iter_mut()
        .find(|node| node.id == id)
        .ok_or_else(|| not_found("Coin node not found"))?;
    if let Some(name) = input.name {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(bad_request(
                "Coin node name must be between 1 and 64 characters",
            ));
        }
        node.name = name.into();
    }
    if let Some(enabled) = input.enabled {
        node.disabled = !enabled;
    }
    let response = json!(node);
    append_audit(
        &mut store,
        "hardware",
        "update",
        "admin",
        id,
        response.to_string(),
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
}

async fn coin_node_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoinNodeQuery>,
) -> ApiResult<Value> {
    register_coin_node(&state, &headers, &query.node_id, None)
        .await
        .map(Json)
}

async fn coin_node_heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CoinNodeHeartbeatInput>,
) -> ApiResult<Value> {
    register_coin_node(&state, &headers, &input.node_id, input.firmware)
        .await
        .map(Json)
}

async fn coin_node_pulse(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CoinNodePulseInput>,
) -> ApiResult<Value> {
    register_coin_node(&state, &headers, &input.node_id, None).await?;
    if !(1..=100).contains(&input.count) {
        return Err(bad_request("Pulse count must be between 1 and 100"));
    }
    if !valid_coin_node_id(&input.event_id) {
        return Err(bad_request("Invalid pulse event ID"));
    }
    let event_key = format!("{}:{}", input.node_id, input.event_id);
    {
        let mut coin = state.coin.write().await;
        coin.processed_events
            .retain(|_, timestamp| *timestamp > Utc::now() - Duration::hours(24));
        if coin.processed_events.contains_key(&event_key) {
            return Ok(Json(json!({
                "accepted": true,
                "duplicate": true,
                "claimId": input.claim_id
            })));
        }
        let Some(claim) = coin.active.as_ref() else {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({ "error": "No customer coin claim is active", "accepted": false })),
            ));
        };
        if claim.id != input.claim_id {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({ "error": "Coin claim ID does not match", "accepted": false })),
            ));
        }
        if claim.node_id.as_deref() != Some(input.node_id.as_str()) {
            return Err((
                StatusCode::CONFLICT,
                Json(
                    json!({ "error": "This claim belongs to a different coin node", "accepted": false }),
                ),
            ));
        }
    }
    {
        let mut store = state.store.write().await;
        let require_signed = store.settings.require_signed_coin_requests;
        let node = store
            .coin_nodes
            .iter_mut()
            .find(|node| node.id == input.node_id)
            .ok_or_else(|| not_found("Coin node is not paired"))?;
        if require_signed && (input.sequence.is_none() || input.timestamp.is_none()) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(
                    json!({"error":"This server requires a timestamp and monotonic sequence for coin pulses"}),
                ),
            ));
        }
        if let Some(timestamp) = input.timestamp {
            let skew = (Utc::now().timestamp() - timestamp).abs();
            if skew > 300 {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(
                        json!({"error":"Coin pulse timestamp is outside the five-minute acceptance window"}),
                    ),
                ));
            }
        }
        if let Some(sequence) = input.sequence {
            if sequence == node.last_sequence {
                return Ok(Json(json!({
                    "accepted": true,
                    "duplicate": true,
                    "claimId": input.claim_id,
                    "sequence": sequence
                })));
            }
            if sequence < node.last_sequence {
                return Err((
                    StatusCode::CONFLICT,
                    Json(
                        json!({"error":"Coin pulse sequence is older than the last accepted event"}),
                    ),
                ));
            }
            node.last_sequence = sequence;
        }
        node.accepted_pulses = node.accepted_pulses.saturating_add(u64::from(input.count));
    }
    persist(&state).await.map_err(bad_request)?;
    state
        .coin
        .write()
        .await
        .processed_events
        .insert(event_key, Utc::now());
    process_coin_pulses(&state, input.count, Some(&input.node_id)).await;
    let coin = state.coin.read().await;
    if coin.completed.contains_key(&input.claim_id) {
        return Ok(Json(json!({
            "accepted": true,
            "completed": true,
            "claimId": input.claim_id
        })));
    }
    let claim = coin
        .active
        .as_ref()
        .filter(|claim| claim.id == input.claim_id);
    Ok(Json(json!({
        "accepted": claim.is_some(),
        "completed": false,
        "claimId": input.claim_id,
        "insertedPesos": claim.map(|claim| claim.inserted_pesos),
        "remainingPesos": claim.map(|claim| claim.rate.price.saturating_sub(claim.inserted_pesos))
    })))
}

fn coin_socket_path() -> PathBuf {
    let default = PathBuf::from("/run/chasselfi/coin.sock");
    let Some(configured) = std::env::var_os("CHASSELFI_COIN_SOCKET").map(PathBuf::from) else {
        return default;
    };
    if configured.parent() == Some(FsPath::new("/run/chasselfi")) {
        configured
    } else {
        warn!(path=%configured.display(), "ignoring coin socket outside /run/chasselfi");
        default
    }
}

fn coin_gate_path() -> PathBuf {
    PathBuf::from("/run/chasselfi/coin-claim.json")
}

fn write_coin_gate(claim: &CoinClaim) -> Result<(), String> {
    let path = coin_gate_path();
    let temporary = path.with_extension("json.pending");
    let body = serde_json::to_vec(&json!({
        "claimId": claim.id,
        "enabled": true,
        "requiredPesos": claim.rate.price,
        "insertedPesos": claim.inserted_pesos,
        "expiresAt": claim.expires_at
    }))
    .map_err(|error| error.to_string())?;
    fs::write(&temporary, body)
        .map_err(|error| format!("could not open coin acceptor gate: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("could not activate coin acceptor gate: {error}"))
}

fn clear_coin_gate() {
    let path = coin_gate_path();
    if let Err(error) = fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(%error, path=%path.display(), "could not close coin acceptor gate");
        }
    }
}

fn parse_coin_pulse(message: &str) -> Result<u32, String> {
    let message = message.trim();
    let count = if message == "PULSE" {
        1
    } else {
        message
            .strip_prefix("PULSE ")
            .ok_or_else(|| "expected PULSE or PULSE <count>".to_string())?
            .parse::<u32>()
            .map_err(|_| "invalid pulse count".to_string())?
    };
    if !(1..=100).contains(&count) {
        return Err("pulse count must be between 1 and 100".into());
    }
    Ok(count)
}

async fn coin_pulse_listener(state: AppState) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        use tokio::net::UnixDatagram;

        let path = coin_socket_path();
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.file_type().is_socket() {
                warn!(path=%path.display(), "coin socket path exists but is not a socket");
                return;
            }
            if let Err(error) = fs::remove_file(&path) {
                warn!(%error, path=%path.display(), "could not replace stale coin socket");
                return;
            }
        }
        let socket = match UnixDatagram::bind(&path) {
            Ok(socket) => socket,
            Err(error) => {
                warn!(%error, path=%path.display(), "physical coin pulse socket is unavailable");
                return;
            }
        };
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o660)) {
            warn!(%error, path=%path.display(), "could not restrict coin socket permissions");
            return;
        }
        state.coin.write().await.socket_ready = true;
        info!(path=%path.display(), "physical coin pulse socket is ready");
        let mut buffer = [0_u8; 128];
        loop {
            match socket.recv(&mut buffer).await {
                Ok(length) => match std::str::from_utf8(&buffer[..length])
                    .map_err(|_| "pulse message is not UTF-8".to_string())
                    .and_then(parse_coin_pulse)
                {
                    Ok(count) => process_coin_pulses(&state, count, None).await,
                    Err(error) => warn!(%error, "rejected coin pulse message"),
                },
                Err(error) => {
                    state.coin.write().await.socket_ready = false;
                    warn!(%error, "coin pulse socket stopped");
                    break;
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        warn!("physical coin mode is available only on Linux");
    }
}

async fn process_coin_pulses(state: &AppState, count: u32, node_id: Option<&str>) {
    let pulse_value = state.store.read().await.settings.coin_pulse_value.max(1);
    let completed = {
        let mut coin = state.coin.write().await;
        let now = Utc::now();
        coin.last_pulse_at = Some(now);
        let Some(claim) = coin.active.as_mut() else {
            warn!(
                count,
                "ignored coin pulses because no customer claim is active"
            );
            return;
        };
        if claim.node_id.as_deref() != node_id {
            warn!(claim_id=%claim.id, "ignored pulse from a coin adapter not assigned to this claim");
            return;
        }
        if claim.expires_at <= now {
            warn!(claim_id=%claim.id, inserted=claim.inserted_pesos, "ignored pulse for expired coin claim");
            coin.active = None;
            clear_coin_gate();
            return;
        }
        claim.inserted_pesos = claim
            .inserted_pesos
            .saturating_add(count.saturating_mul(pulse_value));
        claim.expires_at = now + Duration::minutes(5);
        if claim.inserted_pesos < claim.rate.price {
            if claim.node_id.is_none() {
                if let Err(error) = write_coin_gate(claim) {
                    warn!(%error, "could not update coin acceptor gate");
                }
            }
            return;
        }
        coin.active.take()
    };
    let Some(claim) = completed else { return };
    clear_coin_gate();

    let session = {
        let mut store = state.store.write().await;
        store.transactions.push(Transaction {
            id: Uuid::new_v4(),
            kind: "Coin".into(),
            amount: claim.inserted_pesos,
            minutes: claim.rate.minutes,
            client_ip: claim.client_ip.clone(),
            mac: claim.client_mac.clone(),
            station: "Main vendo".into(),
            created_at: Utc::now(),
        });
        upsert_session(
            &mut store,
            Some(claim.device_key.clone()),
            &claim.client_ip,
            &claim.client_mac,
            claim.rate.minutes,
            claim.rate.download_mbps,
            claim.rate.upload_mbps,
        )
    };
    if let Err(error) = persist(state).await {
        warn!(%error, claim_id=%claim.id, "coin sale could not be persisted");
    }
    let remaining_seconds = session
        .get("remainingSeconds")
        .and_then(Value::as_i64)
        .unwrap_or(i64::from(claim.rate.minutes) * 60)
        .max(1);
    let gateway_error = router::opennds_authorize(
        &claim.client_ip,
        &claim.client_mac,
        ((remaining_seconds + 59) / 60) as u32,
        claim.rate.download_mbps,
        claim.rate.upload_mbps,
    )
    .await
    .err();
    if let Some(error) = gateway_error.as_ref() {
        warn!(%error, claim_id=%claim.id, "coin was recorded but gateway authorization needs retry");
    }
    state.coin.write().await.completed.insert(
        claim.id,
        CoinReceipt {
            client_ip: claim.client_ip,
            session,
            completed_at: Utc::now(),
            gateway_error,
        },
    );
}

fn upsert_session(
    store: &mut Store,
    device_key: Option<String>,
    client_ip: &str,
    client_mac: &str,
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
    let has_stable_mac = router::normalize_mac(client_mac).is_ok();
    if let Some(session) = store.sessions.iter_mut().find(|session| {
        (session.device_key.as_deref() == Some(key.as_str())
            || (has_stable_mac && session.mac.eq_ignore_ascii_case(client_mac)))
            && matches!(
                session.status,
                SessionStatus::Online | SessionStatus::Paused
            )
    }) {
        account_online_session(session, now);
        session.remaining_seconds = session.remaining_seconds.saturating_add(seconds);
        session.status = SessionStatus::Online;
        session.ip = client_ip.into();
        session.mac = client_mac.into();
        session.download_mbps = download_mbps as f32;
        session.upload_mbps = upload_mbps as f32;
        session.last_seen_at = Some(now);
        if let Some(paused_at) = session.paused_at.take() {
            session.total_paused_seconds = session
                .total_paused_seconds
                .saturating_add(now.signed_duration_since(paused_at).num_seconds().max(0));
        }
        session.last_accounted_at = Some(now);
        return json!({
            "id": session.id,
            "token": session.access_token,
            "deviceKey": key,
            "remainingSeconds": session.remaining_seconds,
            "status": "online",
            "downloadMbps": session.download_mbps,
            "uploadMbps": session.upload_mbps
        });
    }
    let id = Uuid::new_v4();
    let token = Uuid::new_v4().to_string();
    store.sessions.push(Session {
        id,
        client_name: "Portal device".into(),
        ip: client_ip.into(),
        mac: client_mac.into(),
        remaining_seconds: seconds,
        status: SessionStatus::Online,
        download_mbps: download_mbps as f32,
        upload_mbps: upload_mbps as f32,
        started_at: now,
        access_token: Some(token.clone()),
        device_key: Some(key.clone()),
        last_seen_at: Some(now),
        last_accounted_at: Some(now),
        paused_at: None,
        pause_count: 0,
        total_paused_seconds: 0,
        source: "portal".into(),
    });
    json!({
        "id": id,
        "token": token,
        "deviceKey": key,
        "remainingSeconds": seconds,
        "status": "online",
        "downloadMbps": download_mbps,
        "uploadMbps": upload_mbps
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
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Session not found" })),
            )
        })?;
    if session.access_token.as_deref() != Some(input.token.as_str())
        || session.device_key.as_deref() != Some(input.device_key.as_str())
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid session token" })),
        ));
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
    let sessions = state.store.read().await.sessions.clone();
    let gateway_clients = if state.hardware_mode == HardwareMode::Linux {
        router::opennds_clients().await.unwrap_or_else(|error| {
            warn!(%error, "could not read live openNDS clients");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    for session in &sessions {
        let gateway = gateway_clients.iter().find(|client| {
            (!session.mac.is_empty() && client.mac.eq_ignore_ascii_case(&session.mac))
                || client.ip == session.ip
        });
        let identity_conflict = gateway.is_some_and(|client| {
            !session.mac.is_empty()
                && (client.ip != session.ip || !client.mac.eq_ignore_ascii_case(&session.mac))
        });
        let mut value = serde_json::to_value(session).unwrap_or_else(|_| json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("managed".into(), json!(true));
            object.insert("gatewayConnected".into(), json!(gateway.is_some()));
            object.insert(
                "gatewayAuthenticated".into(),
                json!(gateway
                    .is_some_and(|client| client.state.eq_ignore_ascii_case("authenticated"))),
            );
            object.insert("identityConflict".into(), json!(identity_conflict));
            object.insert(
                "enforcementMismatch".into(),
                json!(
                    gateway
                        .is_some_and(|client| client.state.eq_ignore_ascii_case("authenticated"))
                        != (session.status == SessionStatus::Online
                            && session.remaining_seconds > 0)
                ),
            );
            object.insert(
                "gatewayState".into(),
                json!(gateway
                    .map(|client| client.state.as_str())
                    .unwrap_or("not-seen")),
            );
            object.insert(
                "clientInterface".into(),
                json!(gateway
                    .map(|client| client.client_if.as_str())
                    .unwrap_or("")),
            );
            object.insert(
                "downloadKbps".into(),
                json!(gateway
                    .map(|client| client.average_download_kbps)
                    .unwrap_or(0.0)),
            );
            object.insert(
                "uploadKbps".into(),
                json!(gateway
                    .map(|client| client.average_upload_kbps)
                    .unwrap_or(0.0)),
            );
            object.insert(
                "downloadedBytes".into(),
                json!(gateway.map(|client| client.downloaded_bytes).unwrap_or(0)),
            );
            object.insert(
                "uploadedBytes".into(),
                json!(gateway.map(|client| client.uploaded_bytes).unwrap_or(0)),
            );
            object.insert(
                "gatewayLastActive".into(),
                json!(gateway.map(|client| client.last_active).unwrap_or(0)),
            );
        }
        rows.push(value);
    }
    for client in gateway_clients.iter().filter(|client| {
        !sessions
            .iter()
            .any(|session| client.ip == session.ip || client.mac.eq_ignore_ascii_case(&session.mac))
    }) {
        let authenticated = client.state.eq_ignore_ascii_case("authenticated");
        rows.push(json!({
            "id": Value::Null,
            "managed": false,
            "clientName": if authenticated { "Unmanaged gateway client" } else { "Waiting for sign-in" },
            "ip": client.ip,
            "mac": client.mac,
            "remainingSeconds": 0,
            "status": if authenticated { "unmanaged" } else { "waiting" },
            "downloadMbps": 0,
            "uploadMbps": 0,
            "gatewayConnected": true,
            "gatewayAuthenticated": authenticated,
            "gatewayState": client.state,
            "clientInterface": client.client_if,
            "downloadKbps": client.average_download_kbps,
            "uploadKbps": client.average_upload_kbps,
            "downloadedBytes": client.downloaded_bytes,
            "uploadedBytes": client.uploaded_bytes,
            "gatewayLastActive": client.last_active,
            "identityConflict": false,
            "enforcementMismatch": authenticated,
            "startedAt": if client.session_start > 0 { Value::from(client.session_start) } else { Value::Null }
        }));
    }
    Json(Value::Array(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionUpdateInput {
    client_name: String,
    remaining_minutes: u32,
    download_mbps: u32,
    upload_mbps: u32,
}

async fn update_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<SessionUpdateInput>,
) -> ApiResult<Value> {
    if input.client_name.trim().is_empty() || input.client_name.len() > 64 {
        return Err(bad_request(
            "Client name must be between 1 and 64 characters",
        ));
    }
    if input.remaining_minutes > 43_200 {
        return Err(bad_request("Remaining time cannot exceed 30 days"));
    }
    if !(1..=10_000).contains(&input.download_mbps) || !(1..=10_000).contains(&input.upload_mbps) {
        return Err(bad_request("Speed must be between 1 and 10000 Mbps"));
    }

    let mut store = state.store.write().await;
    let session = store
        .sessions
        .iter_mut()
        .find(|session| session.id == id)
        .ok_or_else(|| not_found("Session not found"))?;
    let original = session.clone();
    session.client_name = input.client_name.trim().to_string();
    session.remaining_seconds = i64::from(input.remaining_minutes) * 60;
    session.download_mbps = input.download_mbps as f32;
    session.upload_mbps = input.upload_mbps as f32;
    session.status = if input.remaining_minutes == 0 {
        SessionStatus::Ended
    } else {
        session.status.clone()
    };
    let client_ip = session.ip.clone();
    let client_mac = session.mac.clone();
    let is_online = session.status == SessionStatus::Online;
    let response = json!(session);

    let gateway_result = if is_online {
        router::opennds_authorize(
            &client_ip,
            &client_mac,
            input.remaining_minutes.max(1),
            input.download_mbps,
            input.upload_mbps,
        )
        .await
    } else if input.remaining_minutes == 0 {
        router::opennds_deauthorize(&client_ip, &client_mac).await
    } else {
        Ok(())
    };
    if let Err(error) = gateway_result {
        *session = original;
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        ));
    }
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
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
    let now = Utc::now();
    account_online_session(session, now);
    let next_status = match action.as_str() {
        "pause" => SessionStatus::Paused,
        "resume" if session.remaining_seconds > 0 => SessionStatus::Online,
        "resume" => SessionStatus::Ended,
        "stop" => SessionStatus::Ended,
        _ => return Err(bad_request("Unknown session action")),
    };
    let client_ip = session.ip.clone();
    let client_mac = session.mac.clone();
    let remaining_minutes = ((session.remaining_seconds.max(1) + 59) / 60) as u32;
    let download_mbps = session.download_mbps.max(1.0).round() as u32;
    let upload_mbps = session.upload_mbps.max(1.0).round() as u32;
    let gateway_result = if next_status == SessionStatus::Online {
        router::opennds_authorize(
            &client_ip,
            &client_mac,
            remaining_minutes,
            download_mbps,
            upload_mbps,
        )
        .await
    } else {
        router::opennds_deauthorize(&client_ip, &client_mac).await
    };
    gateway_result.map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        )
    })?;
    session.status = next_status;
    session.last_accounted_at = Some(now);
    match session.status {
        SessionStatus::Paused => {
            if session.paused_at.is_none() {
                session.paused_at = Some(now);
                session.pause_count = session.pause_count.saturating_add(1);
            }
        }
        _ => {
            if let Some(paused_at) = session.paused_at.take() {
                session.total_paused_seconds = session
                    .total_paused_seconds
                    .saturating_add(now.signed_duration_since(paused_at).num_seconds().max(0));
            }
        }
    }
    let response = json!(session);
    append_audit(
        &mut store,
        "session",
        &action,
        "admin",
        id.to_string(),
        format!("client={} status={:?}", client_ip, response["status"]),
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
}

async fn list_blocked_sites(State(state): State<AppState>) -> Json<Vec<BlockedSite>> {
    Json(state.store.read().await.blocked_sites.clone())
}

fn valid_blocked_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.contains("..")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn queue_site_block_sync(store: &Store) -> Result<(), String> {
    let runtime_dir = PathBuf::from("/run/chasselfi");
    fs::create_dir_all(&runtime_dir)
        .map_err(|error| format!("cannot access the ChasselFi runtime directory: {error}"))?;
    let pending = runtime_dir.join("site-blocks.pending");
    let request = runtime_dir.join("site-blocks.request");
    let mut hosts = store
        .blocked_sites
        .iter()
        .map(|site| site.host.as_str())
        .collect::<Vec<_>>();
    hosts.sort_unstable();
    let contents = hosts.join("\n") + "\n";
    fs::write(&pending, contents)
        .map_err(|error| format!("could not queue the DNS policy: {error}"))?;
    fs::rename(&pending, &request)
        .map_err(|error| format!("could not activate the DNS policy request: {error}"))
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
    if state.hardware_mode != HardwareMode::Linux {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({ "error": "Site blocking requires the production VLAN router installation" }),
            ),
        ));
    }
    let host = input
        .host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_lowercase();
    if !valid_blocked_hostname(&host) {
        return Err(bad_request(
            "Enter a valid DNS hostname such as example.com",
        ));
    }
    let item = BlockedSite {
        id: Uuid::new_v4(),
        host: host.clone(),
        note: input.note.unwrap_or_default(),
        created_at: Utc::now(),
    };
    let mut store = state.store.write().await;
    if store.blocked_sites.iter().any(|site| site.host == host) {
        return Err(bad_request("That hostname is already blocked"));
    }
    let original = store.clone();
    store.blocked_sites.push(item.clone());
    if let Err(error) = queue_site_block_sync(&store) {
        *store = original;
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        ));
    }
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(item))
}

async fn delete_blocked_site(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Value> {
    if state.hardware_mode != HardwareMode::Linux {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({ "error": "Site blocking requires the production VLAN router installation" }),
            ),
        ));
    }
    let mut store = state.store.write().await;
    let original = store.clone();
    let before = store.blocked_sites.len();
    store.blocked_sites.retain(|item| item.id != id);
    if before == store.blocked_sites.len() {
        return Err(not_found("Block rule not found"));
    }
    if let Err(error) = queue_site_block_sync(&store) {
        *store = original;
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        ));
    }
    drop(store);
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
    if !(1..=100).contains(&settings.coin_pulse_value) {
        return Err(bad_request(
            "Coin pulse value must be between 1 and 100 pesos",
        ));
    }
    if !(1..=1440).contains(&settings.free_time_minutes) {
        return Err(bad_request(
            "Free time must be between 1 minute and 24 hours",
        ));
    }
    if !(1..=8760).contains(&settings.free_time_reset_hours) {
        return Err(bad_request(
            "Free-time reset must be between 1 hour and 1 year",
        ));
    }
    if settings.pause_limit_count > 100 {
        return Err(bad_request("Pause limit cannot exceed 100 per session"));
    }
    if !(1..=10_000).contains(&settings.download_limit_mbps)
        || !(1..=10_000).contains(&settings.upload_limit_mbps)
        || !(1..=10_000).contains(&settings.wan_download_mbps)
        || !(1..=10_000).contains(&settings.wan_upload_mbps)
    {
        return Err(bad_request(
            "Bandwidth limits must be between 1 and 10000 Mbps",
        ));
    }
    if settings.max_pause_minutes > 43_200 || settings.inactivity_pause_minutes > 1440 {
        return Err(bad_request(
            "Pause timing settings are outside the supported range",
        ));
    }
    if !(1..=3650).contains(&settings.audit_retention_days)
        || !(1..=3650).contains(&settings.backup_retention_days)
    {
        return Err(bad_request("Retention must be between 1 and 3650 days"));
    }
    if !matches!(settings.ipv6_policy.as_str(), "block" | "managed" | "allow") {
        return Err(bad_request("IPv6 policy must be block, managed, or allow"));
    }
    let valid_color = settings.portal_accent.len() == 7
        && settings.portal_accent.starts_with('#')
        && settings.portal_accent[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit());
    if !valid_color {
        return Err(bad_request("Portal accent must be a six-digit hex color"));
    }
    if !matches!(
        settings.portal_template.as_str(),
        "aurora" | "midnight" | "sunset"
    ) {
        return Err(bad_request("Unknown portal template"));
    }
    if !matches!(
        settings.voucher_template.as_str(),
        "modern" | "compact" | "ticket"
    ) {
        return Err(bad_request("Unknown voucher template"));
    }
    if settings.terms_title.len() > 100
        || settings.terms_body.len() > 4000
        || settings.portal_eyebrow.len() > 80
        || settings.portal_headline.len() > 120
        || settings.portal_status_label.len() > 80
        || settings.portal_rates_label.len() > 80
        || settings.portal_voucher_label.len() > 80
        || settings.portal_coin_label.len() > 80
        || settings.portal_free_label.len() > 80
        || settings.voucher_footer.len() > 160
    {
        return Err(bad_request("One or more portal text fields are too long"));
    }
    for image in [&settings.portal_banner_image, &settings.portal_logo_image] {
        let valid_image = image.is_empty()
            || ((image.starts_with("data:image/")
                || image.starts_with("https://")
                || image.starts_with("http://")
                || image.starts_with('/'))
                && image.len() <= 2_500_000);
        if !valid_image {
            return Err(bad_request(
                "Portal images must be an image upload, a local path, or an HTTP(S) URL under 2.5 MB",
            ));
        }
    }
    let mut store = state.store.write().await;
    store.settings = settings;
    append_audit(
        &mut store,
        "configuration",
        "update",
        "admin",
        "settings",
        "operator settings saved",
    );
    drop(store);
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
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": format!("{} is disabled in safe simulation mode", action) })),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let request = format!("/run/chasselfi/{action}.request");
        std::fs::write(&request, format!("requested={}\n", Utc::now()))
            .map_err(|error| bad_request(format!("Could not create system request: {error}")))?;
        Ok(Json(json!({"accepted": true, "simulated": false})))
    }
    #[cfg(not(target_os = "linux"))]
    Err(bad_request("Live hardware mode is supported only on Linux"))
}

async fn portal_status(State(state): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let client_ip = client_key(&headers);
    let client_mac = if state.hardware_mode == HardwareMode::Linux {
        router::opennds_clients()
            .await
            .ok()
            .and_then(|clients| {
                clients
                    .into_iter()
                    .find(|client| client.ip == client_ip && client.mac.len() == 17)
            })
            .map(|client| client.mac)
    } else {
        None
    };
    let mut store = state.store.write().await;
    let pause_limit_count = store.settings.pause_limit_count;
    let customer_pause_enabled = store.settings.customer_pause_enabled;
    let low_time_warning_minutes = store.settings.low_time_warning_minutes;
    let session = store.sessions.iter_mut().find(|session| {
        session.ip == client_ip
            && matches!(
                session.status,
                SessionStatus::Online | SessionStatus::Paused
            )
            && session.remaining_seconds > 0
    });
    let response = match session {
        Some(session) => {
            let now = Utc::now();
            account_online_session(session, now);
            session.last_seen_at = Some(now);
            if session.status == SessionStatus::Ended || session.remaining_seconds == 0 {
                json!({ "connected": false, "clientIp": client_ip, "clientMac": client_mac })
            } else {
                json!({
                    "connected": true,
                    "clientIp": client_ip,
                    "clientMac": client_mac,
                    "session": {
                        "id": session.id,
                        "remainingSeconds": session.remaining_seconds,
                        "status": session.status,
                        "downloadMbps": session.download_mbps,
                        "uploadMbps": session.upload_mbps,
                        "startedAt": session.started_at,
                        "pauseCount": session.pause_count,
                        "pauseLimitCount": pause_limit_count,
                        "customerPauseEnabled": customer_pause_enabled,
                        "lowTimeWarningMinutes": low_time_warning_minutes
                    }
                })
            }
        }
        None => json!({ "connected": false, "clientIp": client_ip, "clientMac": client_mac }),
    };
    drop(store);
    if let Err(error) = persist(&state).await {
        warn!(%error, "could not persist portal heartbeat");
    }
    Json(response)
}

async fn portal_session_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(action): Path<String>,
) -> ApiResult<Value> {
    let client_ip = client_key(&headers);
    let mut store = state.store.write().await;
    let customer_pause_enabled = store.settings.customer_pause_enabled;
    let pause_limit_count = store.settings.pause_limit_count;
    let session = store
        .sessions
        .iter_mut()
        .find(|session| {
            session.ip == client_ip
                && session.remaining_seconds > 0
                && matches!(
                    session.status,
                    SessionStatus::Online | SessionStatus::Paused
                )
        })
        .ok_or_else(|| not_found("No active session was found for this device"))?;
    let now = Utc::now();
    account_online_session(session, now);
    let next_status = match action.as_str() {
        "pause" if customer_pause_enabled && session.pause_count < pause_limit_count => {
            SessionStatus::Paused
        }
        "pause" if customer_pause_enabled => {
            return Err(bad_request("This session has reached its pause limit"));
        }
        "pause" => return Err(bad_request("Customer pause is disabled by the operator")),
        "resume" => SessionStatus::Online,
        _ => return Err(bad_request("Unknown customer session action")),
    };
    let remaining_minutes = ((session.remaining_seconds.max(1) + 59) / 60) as u32;
    let download_mbps = session.download_mbps.max(1.0).round() as u32;
    let upload_mbps = session.upload_mbps.max(1.0).round() as u32;
    let client_mac = session.mac.clone();
    let result = if next_status == SessionStatus::Online {
        router::opennds_authorize(
            &client_ip,
            &client_mac,
            remaining_minutes,
            download_mbps,
            upload_mbps,
        )
        .await
    } else {
        router::opennds_deauthorize(&client_ip, &client_mac).await
    };
    result.map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":error})),
        )
    })?;
    session.status = next_status;
    session.last_accounted_at = Some(now);
    if session.status == SessionStatus::Paused {
        session.paused_at = Some(now);
        session.pause_count = session.pause_count.saturating_add(1);
    } else if let Some(paused_at) = session.paused_at.take() {
        session.total_paused_seconds = session
            .total_paused_seconds
            .saturating_add(now.signed_duration_since(paused_at).num_seconds().max(0));
    }
    let response = json!(session);
    append_audit(
        &mut store,
        "session",
        &action,
        "customer",
        response["id"].as_str().unwrap_or("unknown"),
        format!("client={client_ip}"),
    );
    drop(store);
    persist(&state).await.map_err(bad_request)?;
    Ok(Json(response))
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

    #[test]
    fn fas_state_is_signed_and_tamper_evident() {
        let context = FasContext {
            hid: "hid".into(),
            client_ip: "10.0.0.2".into(),
            client_mac: "AA:BB:CC:DD:EE:FF".into(),
            client_if: "enp2s0f0.799".into(),
            auth_action: "http://10.0.0.1:2050/opennds_auth/".into(),
            origin_url: "http://example.com/".into(),
        };
        let signed = sign_fas_state(&context, "test-key").expect("signed state");
        assert_eq!(
            verify_fas_state(&signed, "test-key").expect("verified").hid,
            "hid"
        );
        let tampered = format!("{}x", signed);
        assert!(verify_fas_state(&tampered, "test-key").is_err());
    }

    #[test]
    fn portal_formats_customer_time_cleanly() {
        assert_eq!(human_minutes(30), "30 minutes");
        assert_eq!(human_minutes(120), "2 hours");
        assert_eq!(human_minutes(150), "2h 30m");
        assert_eq!(human_minutes(1440), "1 day");
    }

    #[test]
    fn dns_blocking_accepts_hostnames_not_paths_or_shell_input() {
        assert!(valid_blocked_hostname("example.com"));
        assert!(valid_blocked_hostname("video-cdn.example.com"));
        assert!(!valid_blocked_hostname("https://example.com"));
        assert!(!valid_blocked_hostname("example.com/path"));
        assert!(!valid_blocked_hostname("example.com;reboot"));
        assert!(!valid_blocked_hostname("bad..example.com"));
    }

    #[test]
    fn payment_modes_gate_the_expected_methods() {
        assert!(!model::PaymentMode::None.allows_voucher());
        assert!(!model::PaymentMode::None.allows_coin());
        assert!(model::PaymentMode::Voucher.allows_voucher());
        assert!(!model::PaymentMode::Voucher.allows_coin());
        assert!(model::PaymentMode::Coin.allows_coin());
        assert!(!model::PaymentMode::Coin.allows_voucher());
        assert!(model::PaymentMode::Both.allows_coin());
        assert!(model::PaymentMode::Both.allows_voucher());
    }

    #[test]
    fn physical_pulse_messages_are_bounded() {
        assert_eq!(parse_coin_pulse("PULSE").expect("single pulse"), 1);
        assert_eq!(parse_coin_pulse("PULSE 7").expect("pulse batch"), 7);
        assert!(parse_coin_pulse("PULSE 0").is_err());
        assert!(parse_coin_pulse("PULSE 101").is_err());
        assert!(parse_coin_pulse("CREDIT 10").is_err());
    }

    #[test]
    fn coin_node_identifiers_reject_protocol_characters() {
        assert!(valid_coin_node_id("vendo-01"));
        assert!(valid_coin_node_id("boot123-pulse42"));
        assert!(!valid_coin_node_id("node/../secret"));
        assert!(!valid_coin_node_id("x"));
    }

    #[test]
    fn session_accounting_uses_elapsed_wall_time() {
        let now = Utc::now();
        let mut session = Session {
            id: Uuid::new_v4(),
            client_name: "test".into(),
            ip: "10.0.0.100".into(),
            mac: "AA:BB:CC:DD:EE:FF".into(),
            remaining_seconds: 120,
            status: SessionStatus::Online,
            download_mbps: 10.0,
            upload_mbps: 5.0,
            started_at: now - Duration::minutes(5),
            access_token: None,
            device_key: None,
            last_seen_at: Some(now),
            last_accounted_at: Some(now - Duration::seconds(31)),
            paused_at: None,
            pause_count: 0,
            total_paused_seconds: 0,
            source: "test".into(),
        };
        assert!(!account_online_session(&mut session, now));
        assert_eq!(session.remaining_seconds, 89);
        assert_eq!(session.last_accounted_at, Some(now));
    }

    #[test]
    fn expired_sessions_end_exactly_once() {
        let now = Utc::now();
        let mut session = Session {
            id: Uuid::new_v4(),
            client_name: "test".into(),
            ip: "10.0.0.2".into(),
            mac: "unknown".into(),
            remaining_seconds: 5,
            status: SessionStatus::Online,
            download_mbps: 1.0,
            upload_mbps: 1.0,
            started_at: now,
            access_token: None,
            device_key: None,
            last_seen_at: Some(now),
            last_accounted_at: Some(now - Duration::seconds(10)),
            paused_at: None,
            pause_count: 0,
            total_paused_seconds: 0,
            source: "test".into(),
        };
        assert!(account_online_session(&mut session, now));
        assert_eq!(session.remaining_seconds, 0);
        assert_eq!(session.status, SessionStatus::Ended);
        assert!(!account_online_session(
            &mut session,
            now + Duration::seconds(10)
        ));
    }

    #[test]
    fn backup_checksum_detects_tampering() {
        let store = Store::production();
        let checksum = store_checksum(&store).expect("checksum");
        let mut backup = BackupExport {
            schema_version: 2,
            created_at: Utc::now().to_rfc3339(),
            checksum_sha256: checksum,
            store,
        };
        assert!(validate_backup(&backup).is_ok());
        backup.store.settings.shop_name.push_str(" changed");
        assert!(validate_backup(&backup).is_err());
    }
}

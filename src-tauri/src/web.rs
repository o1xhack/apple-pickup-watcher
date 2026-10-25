//! Tailscale 私有手机控制台。仅监听 loopback，访问权限由现有 Serve 网络规则控制。
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::{AppState, Settings, Target};

pub const PORT: u16 = 17842;
const CONFLICT: &str = "列表已在另一端修改，请刷新后重试";

pub fn revision(settings: &Settings) -> String {
    let mut targets: Vec<_> = settings.targets.iter().collect();
    targets.sort_by_key(|target| target.key().to_string());
    let canonical: Vec<_> = targets
        .into_iter()
        .map(|target| {
            (
                &target.locale,
                &target.store_number,
                &target.part_number,
                target.critical_alert,
                &target.pickup_location,
            )
        })
        .collect();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).unwrap_or_default())
    )
}

#[derive(Clone)]
struct WebState {
    app: AppHandle,
}

/// 保留本机/Tailscale 访问；远端访问默认关闭，启用后 API 仍须设备访问令牌。
pub fn start(app: AppHandle) -> Result<(), String> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, PORT))
        .map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let state = WebState { app: app.clone() };
    let router = router(state);
    tauri::async_runtime::spawn(async move {
        let result = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .map_err(|e| e.to_string()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = result {
            let _ = app.emit(super::NOTICE_CHANNEL, format!("手机网页已停止：{error}"));
        }
    });
    Ok(())
}

fn router(state: WebState) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../web/index.html")) }),
        )
        .route(
            "/i18n.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    concat!(
                        "const translations = ",
                        include_str!("../../web/translations.json"),
                        ";\n",
                        include_str!("../../web/i18n.js")
                    ),
                )
            }),
        )
        .route(
            "/statistics.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../../web/statistics.js"),
                )
            }),
        )
        .route(
            "/LICENSE",
            get(|| async {
                (
                    [("content-type", "text/plain; charset=utf-8")],
                    include_str!("../../LICENSE"),
                )
            }),
        )
        .route(
            "/NOTICE",
            get(|| async {
                (
                    [("content-type", "text/plain; charset=utf-8")],
                    include_str!("../../NOTICE"),
                )
            }),
        )
        .route("/api/state", get(snapshot))
        .route("/api/statistics", get(statistics))
        .route("/api/catalog", get(catalog))
        .route("/api/nearby", get(nearby))
        .route("/api/targets", post(targets))
        .route("/api/run", post(run))
        .route("/api/interval", post(interval))
        .route("/api/repeat-alerts", post(repeat_alerts))
        .route("/api/language", post(language))
        .route("/api/notification-options", post(notification_options))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), lan_guard))
        .layer(middleware::from_fn(guard))
        .with_state(state)
}

async fn statistics(
    State(web): State<WebState>,
    Query(query): Query<super::statistics::Query>,
) -> Response {
    let state = web.app.state::<AppState>();
    let Some(history) = &state.history else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "History unavailable / 历史记录不可用",
        );
    };
    let status = history.status();
    let keys = state
        .settings_snapshot()
        .targets
        .iter()
        .map(|t| t.key().to_string())
        .collect::<Vec<_>>();
    match tauri::async_runtime::spawn_blocking(move || {
        super::statistics::read(&status.path, status.session_id, &keys, query)
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(message)) => error(StatusCode::BAD_REQUEST, &message),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Statistics unavailable / 统计不可用",
        ),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}

/// Same-machine proxies (including the existing Tailscale Serve path) retain their
/// current behavior. Direct LAN reads and writes require an enabled share and token.
fn allow_remote(
    peer: Option<std::net::IpAddr>,
    api: bool,
    enabled: bool,
    expected: &str,
    provided: Option<&str>,
) -> bool {
    use subtle::ConstantTimeEq;
    if peer.is_some_and(|ip| ip.is_loopback()) {
        return true;
    }
    if !peer.is_some_and(|ip| matches!(ip, std::net::IpAddr::V4(ip) if ip.is_private())) || !enabled
    {
        return false;
    }
    if !api {
        return true;
    }
    expected.len() == 64
        && provided.is_some_and(|value| bool::from(value.as_bytes().ct_eq(expected.as_bytes())))
}
async fn lan_guard(
    State(web): State<WebState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let api = request.uri().path().starts_with("/api/");
    let provided = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let settings = web.app.state::<AppState>().settings_snapshot();
    if !allow_remote(
        peer,
        api,
        settings.lan_enabled,
        &settings.lan_access_token,
        provided,
    ) {
        return error(
            StatusCode::UNAUTHORIZED,
            "请从电脑 App 复制局域网访问链接 / Copy the LAN access link from the desktop app",
        );
    }
    next.run(request).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAccess {
    local_url: String,
    lan_enabled: bool,
    lan_urls: Vec<LanUrl>,
}
#[derive(serde::Serialize)]
pub struct LanUrl {
    label: String,
    url: String,
}
#[tauri::command]
pub fn web_access(app: AppHandle) -> WebAccess {
    let settings = app.state::<AppState>().settings_snapshot();
    let mut lan_urls = Vec::new();
    if settings.lan_enabled && settings.lan_access_token.len() == 64 {
        for interface in if_addrs::get_if_addrs().unwrap_or_default() {
            if interface.name.starts_with("utun") {
                continue;
            }
            if let std::net::IpAddr::V4(ip) = interface.ip()
                && ip.is_private()
            {
                let label = format!("http://{ip}:{PORT}");
                let url = format!("{label}/#access={}", settings.lan_access_token);
                if !lan_urls.iter().any(|entry: &LanUrl| entry.label == label) {
                    lan_urls.push(LanUrl { label, url });
                }
            }
        }
    }
    WebAccess {
        local_url: format!("http://127.0.0.1:{PORT}"),
        lan_enabled: settings.lan_enabled,
        lan_urls,
    }
}
#[tauri::command]
pub async fn set_lan_enabled(app: AppHandle, enabled: bool) -> Result<WebAccess, String> {
    {
        let state = app.state::<AppState>();
        let _operation = state.operations.lock().await;
        let mut settings = state.settings_snapshot();
        settings.lan_enabled = enabled;
        if enabled && settings.lan_access_token.len() != 64 {
            settings.lan_access_token = rand::random::<[u8; 32]>()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        }
        state.put_settings(settings)?;
    }
    let _ = app.emit("watcher://remote-change", ());
    Ok(web_access(app))
}

async fn guard(request: axum::extract::Request, next: Next) -> Response {
    let path = request.uri().path();
    let mut response = if path.starts_with("/api/")
        && request.method() != axum::http::Method::GET
        && request
            .headers()
            .get("x-apw-request")
            .and_then(|v| v.to_str().ok())
            != Some("1")
    {
        error(StatusCode::FORBIDDEN, "请求来源校验失败")
    } else {
        next.run(request).await
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            axum::http::HeaderValue::from_static(value),
        );
    }
    response
}

async fn snapshot(State(web): State<WebState>) -> Response {
    let state = web.app.state::<AppState>();
    let _operation = state.operations.lock().await;
    let settings = state.settings_snapshot();
    Json(json!({"rows":state.watcher.snapshot().await, "running":state.watcher.is_running().await,
        "uiAssetMode":if tauri::is_dev() {"development"} else {"embedded"}, "queryTransport":state.client.transport().label(), "locale":settings.locale, "intervalSeconds":settings.interval_seconds, "revision":revision(&settings),
        "colorNames": &*apw_core::display::COLOR_NAMES, "pushConfigured":!settings.bark_url.is_empty(),
        "language":settings.language, "repeatInStock":settings.repeat_in_stock, "notificationLinkMode":settings.notification_link_mode, "paymentPreference":settings.payment_preference,
        "history":state.history.as_ref().map(|h|h.status())})).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NotificationOptions {
    notification_link_mode: apw_core::purchase_link::LinkMode,
    payment_preference: apw_core::purchase_link::PaymentPreference,
}

async fn notification_options(
    State(web): State<WebState>,
    Json(options): Json<NotificationOptions>,
) -> Response {
    let state = web.app.state::<AppState>();
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.notification_link_mode = options.notification_link_mode;
    next.payment_preference = options.payment_preference;
    match state.put_settings(next) {
        Ok(()) => {
            let _ = web.app.emit("watcher://remote-change", ());
            Json(json!({"ok":true})).into_response()
        }
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, &message),
    }
}

#[derive(Deserialize)]
struct Locale {
    locale: String,
}
async fn catalog(State(web): State<WebState>, Query(query): Query<Locale>) -> Response {
    let state = web.app.state::<AppState>();
    match (state.catalog.stores(&query.locale), state.catalog.products(&query.locale)) {
        (Ok(stores), Ok(products)) => Json(json!({"stores":stores,"products":products,"regions":super::list_regions(),"categories":super::list_categories()})).into_response(),
        _ => error(StatusCode::BAD_REQUEST, "地区目录不可用"),
    }
}

#[derive(Deserialize)]
struct Zip {
    zip: String,
}
async fn nearby(State(web): State<WebState>, Query(query): Query<Zip>) -> Response {
    match super::nearby_stores(web.app.state(), query.zip).await {
        Ok(stores) => Json(json!(stores)).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[derive(Deserialize)]
struct TargetUpdate {
    revision: String,
    targets: Vec<Target>,
}

fn validate_targets(state: &AppState, targets: &mut [Target]) -> Result<(), String> {
    if targets.len() > 256 {
        return Err("最多支持 256 条监控目标".into());
    }
    let mut seen = std::collections::HashSet::new();
    for target in targets {
        if !seen.insert(target.key().to_string()) {
            return Err("存在重复目标".into());
        }
        let store = state
            .catalog
            .stores(&target.locale)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.number == target.store_number)
            .ok_or("找不到此门店")?;
        let product = state
            .catalog
            .products(&target.locale)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|p| p.part_number == target.part_number)
            .ok_or("找不到此型号")?;
        if let Some(zip) = target.pickup_location.as_deref()
            && target.locale == "en_US"
            && !(zip.len() == 5 && zip.bytes().all(|b| b.is_ascii_digit())
                || zip.len() == 10
                    && zip.as_bytes()[5] == b'-'
                    && zip[..5].bytes().all(|b| b.is_ascii_digit())
                    && zip[6..].bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("ZIP 格式不正确".into());
        }
        target.store_title = store.title;
        target.product_name = product.title;
        target.companion_part = product.companion_part;
    }
    Ok(())
}

async fn targets(State(web): State<WebState>, Json(mut update): Json<TargetUpdate>) -> Response {
    let state = web.app.state::<AppState>();
    if let Err(message) = validate_targets(&state, &mut update.targets) {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    match super::replace_targets(&state, update.targets, Some(&update.revision)).await {
        Ok(_) => {
            let _ = web.app.emit("watcher://remote-change", ());
            Json(json!({"ok":true})).into_response()
        }
        Err(message) => error(
            if message == CONFLICT {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            &message,
        ),
    }
}

#[derive(Deserialize)]
struct Run {
    running: bool,
}
async fn run(State(web): State<WebState>, Json(request): Json<Run>) -> Response {
    let result = if request.running {
        super::start_watching(web.app.state()).await
    } else {
        super::stop_watching(web.app.state()).await
    };
    match result {
        Ok(()) => {
            let _ = web.app.emit("watcher://remote-change", ());
            Json(json!({"ok":true})).into_response()
        }
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageUpdate {
    language: apw_core::config::Language,
}
async fn language(State(web): State<WebState>, Json(request): Json<LanguageUpdate>) -> Response {
    match super::set_language(web.app, request.language).await {
        Ok(language) => Json(json!({"language":language})).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[derive(Deserialize)]
struct RepeatAlerts {
    enabled: bool,
}
async fn repeat_alerts(State(web): State<WebState>, Json(request): Json<RepeatAlerts>) -> Response {
    match super::set_repeat_in_stock(web.app.state(), request.enabled).await {
        Ok(enabled) => {
            let _ = web.app.emit("watcher://remote-change", ());
            Json(json!({"enabled":enabled})).into_response()
        }
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, &message),
    }
}

#[derive(Deserialize)]
struct Interval {
    seconds: u64,
}
async fn interval(State(web): State<WebState>, Json(request): Json<Interval>) -> Response {
    if !(5..=3600).contains(&request.seconds) {
        return error(StatusCode::BAD_REQUEST, "间隔需为 5–3600 秒");
    }
    match super::set_interval(web.app.state(), request.seconds).await {
        Ok(seconds) => {
            let _ = web.app.emit("watcher://remote-change", ());
            Json(json!({"seconds" : seconds})).into_response()
        }
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn 无需登录可访问但跨站写入仍被拒绝() {
        let router = Router::new()
            .route(
                "/api/state",
                get(|| async { Json(json!({"private":true})) }),
            )
            .route("/api/targets", post(|| async { Json(json!({"ok":true})) }))
            .layer(middleware::from_fn(guard));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::new();
        let allowed = client.get(format!("{url}/api/state")).send().await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(allowed.headers()["cache-control"], "no-store");
        let csrf = client
            .post(format!("{url}/api/targets"))
            .send()
            .await
            .unwrap();
        assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
        assert!(!csrf.headers().contains_key("access-control-allow-origin"));
        let written = client
            .post(format!("{url}/api/targets"))
            .header("x-apw-request", "1")
            .send()
            .await
            .unwrap();
        assert_eq!(written.status(), StatusCode::OK);
        server.abort();
    }

    #[test]
    fn 版本号只跟目标列表有关且修改重要警告会变化() {
        let mut settings = Settings::default();
        let original = revision(&settings);
        settings.bark_url = "private-key".into();
        assert_eq!(revision(&settings), original);
        let mut target: Target = serde_json::from_value(json!({"locale":"en_US","storeNumber":"R105","storeTitle":"Oakridge","partNumber":"SKU","productName":"iPhone"})).unwrap();
        settings.targets.push(target.clone());
        let ordinary = revision(&settings);
        assert_eq!(
            ordinary,
            "0c4aee7dc4077ded9c809d20c791b7b71461e9c2e1836dbe5cf9acccbfb45e30"
        );
        target.critical_alert = true;
        settings.targets = vec![target];
        assert_ne!(revision(&settings), ordinary);
    }
}

#[cfg(test)]
mod lan_tests {
    use super::allow_remote;
    #[test]
    fn lan_access_requires_explicit_enable_and_matching_token_for_every_api_request() {
        let local = Some("127.0.0.1".parse().unwrap());
        let lan = Some("192.168.1.20".parse().unwrap());
        let public = Some("8.8.8.8".parse().unwrap());
        let token = "a".repeat(64);
        assert!(allow_remote(local, true, false, "", None));
        assert!(!allow_remote(lan, false, false, &token, Some(&token)));
        assert!(!allow_remote(lan, true, false, &token, Some(&token)));
        assert!(allow_remote(lan, false, true, &token, None));
        assert!(!allow_remote(lan, true, true, &token, None));
        assert!(!allow_remote(lan, true, true, &token, Some("wrong")));
        assert!(allow_remote(lan, true, true, &token, Some(&token)));
        assert!(!allow_remote(public, true, true, &token, Some(&token)));
        assert!(!allow_remote(None, true, true, &token, Some(&token)));
        assert!(!allow_remote(lan, true, true, "", Some("")));
    }
}

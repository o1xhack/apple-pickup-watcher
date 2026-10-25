//! App-owned Chromium session. No access to the user's browser profile or login.
//! All inventory calls still pass through AppleClient's pacing and cooldown.
use crate::apple::ApiError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
    time::{Instant, sleep, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const START_TIMEOUT: Duration = Duration::from_secs(60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone)]
pub(crate) struct Browser {
    state: Arc<Mutex<Option<Session>>>,
    cookies: Arc<StdMutex<Vec<String>>>,
}
pub(crate) struct Response {
    pub status: u16,
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
}
struct Session {
    child: Child,
    profile: PathBuf,
    socket: Socket,
    sequence: u64,
    origin: Option<String>,
    navigation_status: Option<u16>,
}
impl Drop for Session {
    fn drop(&mut self) {
        // Chrome owns the complete process tree. Never kill a user's browser.
        let _ = self.child.start_kill();
        // Profiles are deliberately small and contain no user account data.
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}
fn failure(message: &str) -> ApiError {
    // Do not propagate CDP errors: they can contain page contents or cookies.
    ApiError::Transport(format!("后台浏览器：{message}"))
}
pub(crate) fn executable() -> Option<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/usr/bin/chromium",
        "/usr/bin/google-chrome",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}
fn validate_url(raw: &str) -> Result<reqwest::Url, ApiError> {
    let url = reqwest::Url::parse(raw).map_err(|_| failure("查询地址无效"))?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("www.apple.com" | "www.apple.com.cn"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(failure("只允许 Apple 商店 HTTPS 地址"));
    }
    Ok(url)
}
async fn profile_directory() -> Result<PathBuf, ApiError> {
    let root = dirs::cache_dir()
        .ok_or_else(|| failure("没有可用缓存目录"))?
        .join("apple-pickup-watcher/browser");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let profile = root.join(format!("session-{}-{unique}", std::process::id()));
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|_| failure("浏览器目录不可写"))?;
    cleanup_orphans(&root).await;
    tokio::fs::create_dir_all(&profile)
        .await
        .map_err(|_| failure("浏览器目录不可写"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| failure("无法保护浏览器目录"))?;
    }
    Ok(profile)
}
/// Only reclaim app-owned scratch profiles whose recorded owner is no longer alive.
async fn cleanup_orphans(root: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let profile = entry.path();
        let Ok(bytes) = tokio::fs::read(profile.join("owner.json")).await else {
            continue;
        };
        let Ok(owner) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let (Some(app), Some(browser)) = (owner["app"].as_u64(), owner["browser"].as_u64()) else {
            continue;
        };
        if app == 0 || browser == 0 || app > i32::MAX as u64 || browser > i32::MAX as u64 {
            continue;
        }
        let live = Command::new("/bin/kill")
            .args(["-0", &app.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if live.is_ok_and(|status| status.success()) {
            continue;
        }
        let args = Command::new("/bin/ps")
            .args(["-p", &browser.to_string(), "-o", "command="])
            .output()
            .await;
        if let Ok(args) = args
            && String::from_utf8_lossy(&args.stdout)
                .contains(&format!("--user-data-dir={}", profile.display()))
        {
            let _ = Command::new("/bin/kill")
                .args(["-TERM", &browser.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            // Chrome may still be flushing files. Leave profile cleanup to the next start.
        } else {
            let _ = tokio::fs::remove_dir_all(profile).await;
        }
    }
}
impl Browser {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            cookies: Arc::new(StdMutex::new(Vec::new())),
        }
    }
    pub fn cookie_names(&self) -> Vec<String> {
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub async fn reset(&self) {
        *self.state.lock().await = None;
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    pub async fn fetch(
        &self,
        raw: &str,
        query: &[(String, String)],
        max_body: usize,
        landing_page: Option<&str>,
    ) -> Result<Response, ApiError> {
        let mut url = validate_url(raw)?;
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k, v)));
        }
        let mut guard = self.state.lock().await;
        // Take ownership so cancellation/timeout always discards a possibly busy CDP socket.
        let existing = guard.take();
        let operation = async {
            let mut session = match existing {
                Some(s) => s,
                None => Session::start().await?,
            };
            let origin = url.origin().ascii_serialization();
            if session.origin.as_deref() != Some(&origin) {
                let landing = if query.is_empty() {
                    raw.to_owned()
                } else {
                    let landing = landing_page
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{origin}/shop/buy-iphone"));
                    if validate_url(&landing)?.origin().ascii_serialization() != origin {
                        return Err(failure("商品页与库存接口不是同一站点"));
                    }
                    landing
                };
                session.navigate(&landing).await?;
                session.origin = Some(origin);
            } else if query.is_empty() {
                session.navigate(raw).await?;
            }
            let result = if query.is_empty() {
                // Navigation itself performed warm-up. Do not issue a second HTTP request.
                Response {
                    status: session
                        .navigation_status
                        .ok_or_else(|| failure("商品页缺少 HTTP 响应记录"))?,
                    url: raw.into(),
                    content_type: "text/html".into(),
                    body: Vec::new(),
                }
            } else {
                let expression = fetch_expression(url.as_str(), max_body);
                let value = session.evaluate(&expression).await?;
                decode_response(value, max_body)?
            };
            // Cookie diagnostics are auxiliary: never discard a valid inventory response.
            let retained = match session
                .command("Network.getCookies", json!({"urls":[raw]}))
                .await
            {
                Ok(cookies) => {
                    let names = cookies["cookies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["name"].as_str().map(str::to_owned))
                        .collect();
                    *self.cookies.lock().unwrap_or_else(|e| e.into_inner()) = names;
                    Some(session)
                }
                Err(_) => None,
            };
            Ok((retained, result))
        };
        match timeout(START_TIMEOUT + COMMAND_TIMEOUT, operation).await {
            Ok(Ok((session, result))) => {
                *guard = session;
                Ok(result)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(failure("操作超时，会话已关闭")),
        }
    }
}
fn fetch_expression(url: &str, limit: usize) -> String {
    format!(
        r#"(async()=>{{
      const r=await fetch({},{{credentials:'same-origin',cache:'no-store',headers:{{Accept:'application/json, text/javascript, */*; q=0.01'}},signal:AbortSignal.timeout(20000)}});
      if(!r.ok) {{await r.body?.cancel(); return {{status:r.status,url:r.url,contentType:r.headers.get('content-type')||'',body:''}};}}
      const reader=r.body.getReader(); let size=0; const decoder=new TextDecoder(); let text='';
      try {{ while(true) {{const chunk=await reader.read(); if(chunk.done) break; size+=chunk.value.byteLength;
        if(size>{limit}) {{await reader.cancel(); throw new Error('response too large');}} text+=decoder.decode(chunk.value,{{stream:true}});}}
        text+=decoder.decode(); }} finally {{reader.releaseLock();}}
      return {{status:r.status,url:r.url,contentType:r.headers.get('content-type')||'',body:text}};
    }})()"#,
        serde_json::to_string(url).expect("URL serializes")
    )
}
fn decode_response(value: Value, limit: usize) -> Result<Response, ApiError> {
    let status = value["status"]
        .as_u64()
        .filter(|s| (100..600).contains(s))
        .ok_or_else(|| failure("响应没有有效状态码"))? as u16;
    let body = value["body"]
        .as_str()
        .ok_or_else(|| failure("响应缺少内容"))?
        .as_bytes()
        .to_vec();
    if body.len() > limit {
        return Err(failure("响应超过读取上限"));
    }
    let raw = value["url"]
        .as_str()
        .ok_or_else(|| failure("响应缺少地址"))?;
    validate_url(raw)?;
    Ok(Response {
        status,
        url: raw.into(),
        content_type: value["contentType"].as_str().unwrap_or_default().into(),
        body,
    })
}
impl Session {
    async fn start() -> Result<Self, ApiError> {
        let binary = executable().ok_or_else(|| failure("未找到 Chrome 或 Chromium"))?;
        let profile = profile_directory().await?;
        let child = Command::new(binary)
            .args([
                "--headless=new",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-sync",
                "--disable-extensions",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                let _ = std::fs::remove_dir_all(&profile);
                failure("启动失败")
            })?;
        // A guard owns the child even if any startup step is cancelled or errors.
        let mut pending = Pending {
            child: Some(child),
            profile: Some(profile.clone()),
        };
        let owner =
            json!({"app":std::process::id(),"browser":pending.child.as_ref().and_then(|c|c.id())});
        tokio::fs::write(profile.join("owner.json"), owner.to_string())
            .await
            .map_err(|_| failure("无法保存浏览器进程归属"))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        let port = loop {
            if let Ok(s) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await
                && let Some(port) = s.lines().next().and_then(|v| v.parse::<u16>().ok())
            {
                break port;
            }
            if pending
                .child
                .as_mut()
                .expect("owned")
                .try_wait()
                .map_err(|_| failure("进程状态不可读"))?
                .is_some()
                || Instant::now() >= deadline
            {
                return Err(failure("等待启动超时"));
            }
            sleep(Duration::from_millis(100)).await;
        };
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| failure("本地调试连接失败"))?;
        let targets: Value = http
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .send()
            .await
            .map_err(|_| failure("调试端口不可用"))?
            .json()
            .await
            .map_err(|_| failure("调试响应无效"))?;
        let ws = targets
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["type"] == "page")
            .and_then(|v| v["webSocketDebuggerUrl"].as_str())
            .ok_or_else(|| failure("没有可用页面"))?;
        if !ws.starts_with(&format!("ws://127.0.0.1:{port}/devtools/page/")) {
            return Err(failure("调试地址不是本地页面"));
        }
        let (socket, _) = tokio_tungstenite::connect_async(ws)
            .await
            .map_err(|_| failure("调试握手失败"))?;
        Ok(Self {
            child: pending.child.take().expect("owned"),
            profile: pending.profile.take().expect("owned"),
            socket,
            sequence: 0,
            origin: None,
            navigation_status: None,
        })
    }
    async fn command(&mut self, method: &str, params: Value) -> Result<Value, ApiError> {
        self.sequence += 1;
        let id = self.sequence;
        timeout(COMMAND_TIMEOUT, async {
            self.socket
                .send(Message::Text(
                    json!({"id":id,"method":method,"params":params})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|_| failure("调试连接写入失败"))?;
            while let Some(message) = self.socket.next().await {
                let message = message.map_err(|_| failure("调试连接中断"))?;
                if let Message::Ping(bytes) = message {
                    self.socket
                        .send(Message::Pong(bytes))
                        .await
                        .map_err(|_| failure("调试连接中断"))?;
                    continue;
                }
                let Message::Text(text) = message else {
                    continue;
                };
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| failure("调试响应无效"))?;
                if value["method"] == "Network.responseReceived"
                    && value["params"]["type"] == "Document"
                {
                    self.navigation_status = value["params"]["response"]["status"]
                        .as_f64()
                        .map(|n| n as u16);
                }
                if value["id"].as_u64() != Some(id) {
                    continue;
                }
                if value.get("error").is_some() {
                    return Err(failure("页面命令执行失败"));
                }
                return Ok(value["result"].clone());
            }
            Err(failure("调试连接已关闭"))
        })
        .await
        .map_err(|_| failure("页面命令超时"))?
    }
    async fn evaluate(&mut self, expression: &str) -> Result<Value, ApiError> {
        let result = self
            .command(
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(failure("页面查询失败"));
        }
        result["result"]
            .get("value")
            .cloned()
            .ok_or_else(|| failure("页面没有返回结果"))
    }
    async fn navigate(&mut self, url: &str) -> Result<(), ApiError> {
        self.command("Network.enable", json!({})).await?;
        self.navigation_status = None;
        let result = self.command("Page.navigate", json!({"url":url})).await?;
        if result.get("errorText").is_some() {
            return Err(failure("商品页无法加载"));
        }
        let expected = validate_url(url)?.origin().ascii_serialization();
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            // Navigation can destroy the old execution context between two CDP calls.
            // Retry readiness observation, not navigation or the inventory request.
            let observed=self.evaluate("({origin:location.origin,ready:document.readyState,cookies:document.cookie.split(';').map(v=>v.trim().split('=')[0])})").await;
            let value = match observed {
                Ok(value) => value,
                Err(_)
                    if Instant::now() < deadline
                        && self.child.try_wait().ok().flatten().is_none() =>
                {
                    sleep(Duration::from_millis(250)).await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if value["origin"] == expected
                && value["ready"] == "complete"
                && value["cookies"]
                    .as_array()
                    .is_some_and(|cookies| cookies.iter().any(|v| v == "as_atb"))
            {
                if self
                    .navigation_status
                    .is_some_and(|s| matches!(s, 403 | 429 | 541))
                {
                    return Err(ApiError::Blocked("浏览器商品页被拒绝".into()));
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ApiError::Blocked("浏览器商品页未完成会话初始化".into()));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }
}
struct Pending {
    child: Option<Child>,
    profile: Option<PathBuf>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(profile) = self.profile.as_ref() {
            let _ = std::fs::remove_dir_all(profile);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires local Chromium; only opens about:blank, never contacts Apple"]
    async fn local_browser_cdp_smoke() {
        let mut session = Session::start().await.unwrap();
        let profile = session.profile.clone();
        assert!(profile.is_dir());
        assert_eq!(session.evaluate("6*7").await.unwrap(), 42);
        assert_eq!(
            session.evaluate("location.href").await.unwrap(),
            "about:blank"
        );
        // Exercise the production fetch script against local Response objects, not Apple.
        session.evaluate("globalThis.fetch=async(url,options)=>{if(options.cache!=='no-store'||options.credentials!=='same-origin')throw Error('bad options');const r=new Response('{\"body\":{\"stores\":[]}}',{headers:{'content-type':'application/json'}});Object.defineProperty(r,'url',{value:url});return r;};true").await.unwrap();
        let value = session
            .evaluate(&fetch_expression(
                "https://www.apple.com/shop/retail/pickup-message",
                1024,
            ))
            .await
            .unwrap();
        let response = decode_response(value, 1024).unwrap();
        assert_eq!(response.status, 200);
        assert!(
            serde_json::from_slice::<Value>(&response.body).unwrap()["body"]["stores"].is_array()
        );
        session.evaluate("globalThis.fetch=async(url)=>{const r=new Response('blocked body',{status:541});Object.defineProperty(r,'url',{value:url});return r;};true").await.unwrap();
        let blocked = session
            .evaluate(&fetch_expression(
                "https://www.apple.com/shop/retail/pickup-message",
                1,
            ))
            .await
            .unwrap();
        assert_eq!(blocked["status"], 541);
        assert_eq!(blocked["body"], "");
        session.evaluate("globalThis.fetch=async(url)=>{const r=new Response('oversized');Object.defineProperty(r,'url',{value:url});return r;};true").await.unwrap();
        assert!(
            session
                .evaluate(&fetch_expression(
                    "https://www.apple.com/shop/retail/pickup-message",
                    1
                ))
                .await
                .is_err()
        );

        assert!(
            session
                .evaluate("throw new Error('PRIVATE_COOKIE_VALUE')")
                .await
                .unwrap_err()
                .to_string()
                .find("PRIVATE_COOKIE_VALUE")
                .is_none()
        );
        drop(session);
    }
    #[test]
    fn only_apple_https_is_accepted() {
        for url in [
            "http://www.apple.com/shop",
            "https://apple.com.evil/shop",
            "https://user:secret@www.apple.com/shop",
            "https://127.0.0.1/",
            "https://www.apple.com:1234/",
        ] {
            assert!(validate_url(url).is_err());
        }
        assert!(validate_url("https://www.apple.com/shop/retail/pickup-message").is_ok());
    }
    #[test]
    fn response_limit_is_bytes_and_does_not_discard_541() {
        let value = json!({"status":541,"url":"https://www.apple.com/shop/retail/pickup-message","contentType":"text/html","body":"拦截"});
        assert!(decode_response(value.clone(), 5).is_err());
        assert_eq!(decode_response(value, 6).unwrap().status, 541);
    }
    #[test]
    fn query_is_serialized_and_reader_is_capped() {
        let script = fetch_expression("https://www.apple.com/?test='\n", 42);
        assert!(script.contains("size>42"));
        assert!(script.contains("credentials:'same-origin'"));
        assert!(script.contains("AbortSignal.timeout"));
        assert!(script.contains("\\n"));
    }
}

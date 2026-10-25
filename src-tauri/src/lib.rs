//! Tauri 应用外壳。
//!
//! 这一层只做装配和转译：起引擎、把命令转成引擎消息、把引擎事件转发给前端、
//! 在有货时发提醒。**所有业务判断都在 `apw-core` 里**，这里不许出现任何
//! 「什么算有货」之类的逻辑 —— 一旦让界面层参与判断，那条核心不变量就多了
//! 一处可以被绕开的地方。
//!
//! 提醒也刻意放在这一层而不是前端：托盘模式下窗口是隐藏的，WebView 可能被
//! 系统节流甚至挂起，把「及时提醒」挂在一个会被挂起的执行环境上是不能接受的。

use std::sync::{Arc, RwLock};
use std::time::Duration;

use apw_core::apple::{AppleClient, ClientConfig, Transport, WarmPage};
use apw_core::catalog::Catalog;
use apw_core::config::{MIN_INTERVAL_SECONDS, Settings, SettingsStore};
use apw_core::model::{Category, Product, REGIONS, Store, Target, region_by_locale};
use apw_core::notify::{Bark, Feishu, Multi, Notification, Notifier, Sound};
use apw_core::watcher::{Event, TargetState, Watcher, WatcherConfig};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

mod auto_open;
mod history;
mod notification_queue;
mod statistics;
mod watch_state;
mod web;

use auto_open::AutoOpenBag;

/// 前端事件通道名。前端用 `listen("watcher://event", ...)` 订阅。
const EVENT_CHANNEL: &str = "watcher://event";
/// 启动过程中的降级说明通道：配置读不出来之类的事必须让用户看见。
const NOTICE_CHANNEL: &str = "watcher://notice";

/// 地区的可序列化形式。
///
/// `model::Region` 的字段都是 `&'static str`，而且界面不需要知道 `base_url`
/// 这类内部细节。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegionDto {
    title: &'static str,
    locale: &'static str,
}

/// 品类的可序列化形式。
///
/// 界面上的品类下拉框由这里驱动，而不是在前端另抄一份常量：抄一份就迟早会有
/// 一边先加了品类、另一边还蒙在鼓里。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryDto {
    value: Category,
    title: &'static str,
}

struct AppState {
    history: Option<Arc<history::History>>,
    operations: tokio::sync::Mutex<()>,
    watcher: Watcher,
    /// 和引擎共用的 Apple 客户端句柄，改代理设置时用它换线路，不用重启。
    client: AppleClient,
    catalog: Catalog,
    http: reqwest::Client,
    /// 设置的内存副本。写盘失败不该让界面卡住，所以内存副本是权威的展示来源。
    settings: RwLock<Settings>,
    /// 为 `None` 表示配置不可持久化（目录不可写，或上次读取失败已放弃写盘）。
    store: Option<SettingsStore>,
}

impl AppState {
    fn settings_snapshot(&self) -> Settings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// 更新内存副本并尝试落盘。落盘失败只报错，不回滚内存 ——
    /// 用户的操作已经生效了，没道理因为磁盘问题把界面弹回去。
    fn put_settings(&self, next: Settings) -> Result<(), String> {
        let mut guard = self.settings.write().unwrap_or_else(|e| e.into_inner());
        *guard = next;
        let to_save = guard.clone();
        drop(guard);

        match &self.store {
            Some(store) => store.save(&to_save).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

#[tauri::command]
fn list_regions() -> Vec<RegionDto> {
    REGIONS
        .iter()
        .map(|r| RegionDto {
            title: r.title,
            locale: r.locale,
        })
        .collect()
}

#[tauri::command]
fn list_categories() -> Vec<CategoryDto> {
    Category::ALL
        .iter()
        .map(|c| CategoryDto {
            value: *c,
            title: c.title(),
        })
        .collect()
}

#[tauri::command]
fn list_stores(state: tauri::State<'_, AppState>, locale: String) -> Result<Vec<Store>, String> {
    state.catalog.stores(&locale).map_err(|e| e.to_string())
}

#[tauri::command]
async fn nearby_stores(
    state: tauri::State<'_, AppState>,
    zip: String,
) -> Result<Vec<apw_core::apple::NearbyStore>, String> {
    let zip = zip.trim();
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !(zip.len() == 5 && digits(zip)
        || zip.len() == 10 && zip.as_bytes()[5] == b'-' && digits(&zip[..5]) && digits(&zip[6..]))
    {
        return Err("请输入 5 位 ZIP 或 ZIP+4".into());
    }
    let region = region_by_locale("en_US").expect("美国地区应存在");
    let product = state
        .catalog
        .products("en_US")
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|p| p.family == "iphone18pro" && p.capacity == "256GB")
        .ok_or("缺少用于查询附近门店的在售商品，请刷新美国型号目录")?;
    state
        .client
        .nearby_stores(region, zip, &product.part_number)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_products(
    state: tauri::State<'_, AppState>,
    locale: String,
) -> Result<Vec<Product>, String> {
    state.catalog.products(&locale).map_err(|e| e.to_string())
}

/// 从 Apple 官网抓最新型号，替换该地区该品类的内存副本，返回抓到的型号数。
///
/// `category` 为 `None` 时抓该地区的全部购买页。界面传的是当前选中的品类：
/// 一次只抓那几页，用户想看新出的 Mac 不必等 iPhone、iPad、Watch 一起抓完。
#[tauri::command]
async fn refresh_products(
    state: tauri::State<'_, AppState>,
    locale: String,
    category: Option<Category>,
) -> Result<usize, String> {
    let region = region_by_locale(&locale).ok_or_else(|| format!("认不出地区 {locale}"))?;
    state
        .catalog
        .refresh_products(region, category, &state.http)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Settings {
    state.settings_snapshot()
}

/// 保存独立语言字段，避免跨端改动覆盖监控和推送设置。
#[tauri::command]
async fn set_language(
    app: AppHandle,
    language: apw_core::config::Language,
) -> Result<apw_core::config::Language, String> {
    let state = app.state::<AppState>();
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.language = language;
    state.put_settings(next)?;
    let _ = app.emit("watcher://remote-change", ());
    Ok(language)
}

#[tauri::command]
async fn save_settings(
    state: tauri::State<'_, AppState>,
    mut settings: Settings,
) -> Result<Settings, String> {
    let _operation = state.operations.lock().await;
    let current = state.settings_snapshot();
    settings.targets = current.targets;
    settings.watching_enabled = current.watching_enabled;
    settings.language = current.language;
    settings.lan_enabled = current.lan_enabled;
    settings.lan_access_token = current.lan_access_token;
    let mut next = settings;
    next.normalize();
    state.catalog.attach_companions(&mut next.targets);
    state.catalog.attach_locations(&mut next.targets);

    // 设置里的目标列表和查询间隔要同步给引擎，否则改完设置监控还按旧的跑。
    state.watcher.set_targets(next.targets.clone()).await;
    state.watcher.set_interval(next.interval()).await;
    state
        .watcher
        .set_repeat_in_stock(next.repeat_in_stock)
        .await;
    // 代理变了就换线路，立即生效；地址解析不了就把错误报回界面，其余设置照存。
    if next.proxies != state.settings_snapshot().proxies {
        state
            .client
            .set_proxies(&next.proxies)
            .await
            .map_err(|e| format!("代理设置未生效：{e}"))?;
    }

    state.put_settings(next.clone())?;
    Ok(next)
}

/// 用界面上的推送地址列表整体替换现有的。
///
/// 每个地址是 Bark 还是飞书由 [`Settings::set_push_urls`] 按长相判断，界面不自己猜。
/// 只动推送地址：不经过 [`save_settings`]，免得顺带把引擎的目标、间隔和代理
/// 也重新下发一遍。
#[tauri::command]
async fn set_push_urls(
    state: tauri::State<'_, AppState>,
    urls: Vec<String>,
) -> Result<Settings, String> {
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.set_push_urls(&urls);
    next.normalize();
    state.put_settings(next.clone())?;
    Ok(next)
}

#[tauri::command]
async fn get_snapshot(state: tauri::State<'_, AppState>) -> Result<Vec<TargetState>, String> {
    Ok(state.watcher.snapshot().await)
}

#[tauri::command]
fn target_revision(targets: Vec<Target>) -> String {
    web::revision(&Settings {
        targets,
        ..Settings::default()
    })
}

#[tauri::command]
async fn set_targets(
    state: tauri::State<'_, AppState>,
    targets: Vec<Target>,
    revision: Option<String>,
) -> Result<Vec<TargetState>, String> {
    replace_targets(&state, targets, revision.as_deref()).await
}

async fn replace_targets(
    state: &AppState,
    targets: Vec<Target>,
    revision: Option<&str>,
) -> Result<Vec<TargetState>, String> {
    let _operation = state.operations.lock().await;
    if revision.is_some_and(|expected| expected != web::revision(&state.settings_snapshot())) {
        return Err("列表已在另一端修改，请刷新后重试".into());
    }
    let mut targets = targets;
    state.catalog.attach_companions(&mut targets);
    state.catalog.attach_locations(&mut targets);
    let mut next = state.settings_snapshot();
    next.watching_enabled =
        watch_state::after_target_change(&next.targets, &targets, next.watching_enabled);
    let enabled = next.watching_enabled;
    next.targets = targets.clone();
    state.put_settings(next)?;
    state.watcher.set_targets(targets).await;
    if enabled {
        state.watcher.start().await;
    } else {
        state.watcher.stop().await;
    }
    Ok(state.watcher.snapshot().await)
}

#[tauri::command]
async fn set_interval(state: tauri::State<'_, AppState>, seconds: u64) -> Result<u64, String> {
    let _operation = state.operations.lock().await;
    let secs = seconds.max(MIN_INTERVAL_SECONDS);
    state.watcher.set_interval(Duration::from_secs(secs)).await;
    let mut next = state.settings_snapshot();
    next.interval_seconds = secs;
    state.put_settings(next)?;
    Ok(secs)
}

#[tauri::command]
async fn set_repeat_in_stock(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.repeat_in_stock = enabled;
    state.put_settings(next)?;
    state.watcher.set_repeat_in_stock(enabled).await;
    Ok(enabled)
}

#[tauri::command]
async fn start_watching(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.watching_enabled = !next.targets.is_empty();
    let enabled = next.watching_enabled;
    state.put_settings(next)?;
    if enabled {
        state.watcher.start().await;
    }
    Ok(())
}

#[tauri::command]
async fn stop_watching(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let _operation = state.operations.lock().await;
    let mut next = state.settings_snapshot();
    next.watching_enabled = false;
    state.put_settings(next)?;
    state.watcher.stop().await;
    Ok(())
}

#[tauri::command]
async fn is_running(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.watcher.is_running().await)
}

/// 手动试一次提醒，让用户在真正抢购之前确认铃声和推送都通。
#[tauri::command]
async fn test_notify(app: AppHandle, critical: Option<bool>) -> Result<(), String> {
    let language = app.state::<AppState>().settings_snapshot().language;
    let (title, body) = match language {
        apw_core::config::Language::Zh => (
            "有货了（测试）",
            "Apple 库存监控：这是一条测试提醒，非实际到货。",
        ),
        apw_core::config::Language::En => (
            "In stock (test)",
            "Apple Inventory Monitor: test notification only, not actual stock.",
        ),
    };
    dispatch_notification(
        &app,
        // 标题刻意和真实到货提醒一样带「有货了」：飞书自定义机器人的关键词
        // 安全模式只放行含关键词的消息，测试文案若不含，用户会误以为配置坏了。
        Notification::new(title, body).with_critical_alert(critical.unwrap_or(false)),
    )
    .await
    .map_err(|e| e.to_string())
}

/// 发出一条提醒：系统通知 + 提示音 + Bark，按用户设置取舍。
async fn dispatch_notification(
    app: &AppHandle,
    notification: Notification,
) -> Result<(), apw_core::notify::NotifyError> {
    let settings = match app.try_state::<AppState>() {
        Some(state) => state.settings_snapshot(),
        None => return Ok(()),
    };

    // 系统通知总是发。它是最轻量也最可靠的一条，没有理由让用户关掉它之后
    // 就完全收不到东西。
    emit_system_notification(app, &notification);

    let mut channels = Multi::new();
    if settings.sound_enabled {
        channels.push(Sound::embedded());
    }
    if !settings.bark_url.trim().is_empty() {
        let http = app
            .try_state::<AppState>()
            .map(|s| s.http.clone())
            .unwrap_or_default();
        // Bark 每次现构造：地址是用户随时可改的设置项，缓存实例会在改完地址后
        // 继续往旧地址推。共享的 http 客户端一并传进去，连接池仍然复用。
        // 用户可以填多个地址（分号分隔），每个地址一条渠道，并发推送、各自报错。
        for bark in Bark::from_list(&settings.bark_url, http) {
            channels.push(bark);
        }
    }
    if !settings.feishu_webhook.trim().is_empty() {
        let http = app
            .try_state::<AppState>()
            .map(|s| s.http.clone())
            .unwrap_or_default();
        // 飞书与 Bark 同样的构造规则：地址随时可改、支持分号分隔多个、
        // 共享连接池，渠道各自报错。
        for feishu in Feishu::from_list(&settings.feishu_webhook, http) {
            channels.push(feishu);
        }
    }
    if channels.is_empty() {
        return Ok(());
    }
    channels.notify(&notification).await
}

fn emit_system_notification(app: &AppHandle, n: &Notification) {
    use tauri_plugin_notification::NotificationExt;
    // 通知发不出去（用户在系统里关了权限）不该影响其他渠道，也不该让流程中断。
    if let Err(err) = app
        .notification()
        .builder()
        .title(&n.title)
        .body(&n.body)
        .show()
    {
        let _ = app.emit(NOTICE_CHANNEL, format!("系统通知发送失败：{err}"));
    }
}

/// 消费引擎事件：转发给前端，并在有货时发提醒。
async fn pump_events(app: AppHandle, mut events: tokio::sync::mpsc::Receiver<Event>) {
    let mut auto_open_bag = AutoOpenBag::default();
    let history = app.state::<AppState>().history.clone();
    let (notifications, pending) = notification_queue::channel();
    let notification_app = app.clone();
    let delivery_history = history.clone();
    tauri::async_runtime::spawn(notification_queue::run(
        pending,
        move |(notification, history_id)| {
            let app = notification_app.clone();
            let history = delivery_history.clone();
            async move {
                let result = dispatch_notification(&app, notification).await;
                if let (Some(history), Some(id)) = (history, history_id) {
                    history.record_delivery(id, result.is_ok());
                }
                if let Err(err) = result {
                    let _ = app.emit(NOTICE_CHANNEL, format!("发送提醒时出错：{err}"));
                }
            }
        },
    ));

    while let Some(event) = events.recv().await {
        if let Some(history) = &history {
            history.record_event(&event);
        }
        // 先原样转发。前端拿到的事件流应当与引擎发出的完全一致，
        // 中间少一层可能出错的翻译。
        let _ = app.emit(EVENT_CHANNEL, &event);

        if let Event::RunStateChanged { running } = &event {
            auto_open_bag.on_run_state_changed(*running);
        }

        if let Event::InStock { state } = &event {
            let target = &state.target;
            let product = app.try_state::<AppState>().and_then(|state| {
                state
                    .catalog
                    .products(&target.locale)
                    .ok()
                    .and_then(|products| {
                        products
                            .into_iter()
                            .find(|p| p.part_number == target.part_number)
                    })
            });
            let settings = app
                .try_state::<AppState>()
                .map(|s| s.settings_snapshot())
                .unwrap_or_default();
            let (title, body) = apw_core::display::stock_notice_in_language(
                target,
                product.as_ref(),
                settings.language,
            );
            let notification = Notification::new(title, body)
                .with_critical_alert(target.critical_alert)
                .with_bark_json();
            let notification = match apw_core::purchase_link::notification_url(
                target,
                product.as_ref(),
                &settings,
            ) {
                Some(url) => notification.with_url(url),
                None => notification,
            };

            use tauri_plugin_opener::OpenerExt;
            if let Err(err) =
                auto_open_bag.open_if_needed(settings.open_bag_on_hit, &target.locale, |url| {
                    app.opener().open_url(url, None::<&str>)
                })
            {
                let _ = app.emit(NOTICE_CHANNEL, format!("打开购物袋失败：{err}"));
            }

            // 不逐项等待网络推送，否则15个Bark超时会让快照和暂停状态迟到150秒。
            // 队列满时保留背压，不丢提醒，也不无限创建后台任务。
            let history_id = history.as_ref().and_then(|history| {
                history.record_notice(state, &notification, !settings.push_urls().is_empty())
            });
            if notifications
                .send((notification, history_id.clone()))
                .await
                .is_err()
            {
                if let (Some(history), Some(id)) = (&history, history_id) {
                    history.record_delivery(id, false);
                }
                let _ = app.emit(NOTICE_CHANNEL, "提醒发送任务已停止，无法发送到货提醒");
            }
        }
    }
}

/// 建系统托盘。
///
/// 关窗口时不退出而是收进托盘：这个工具的正常用法就是挂上几个小时等发售，
/// 让它一直占着一个窗口和 Dock 图标没有道理。
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon().cloned().ok_or_else(|| {
                tauri::Error::AssetNotFound("默认窗口图标缺失，无法建立托盘".into())
            })?,
        )
        .tooltip("Apple 取货监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => reveal_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn reveal_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 载入设置。读不出来时放弃写盘并留档原文件。
///
/// 这一条是刻意的：读不到旧配置**不等于**用户没有配置。若照常写盘，界面初始化
/// 时的几次控件赋值就会把仅存的那份原子替换成一份空的默认配置，监控列表再也
/// 找不回来。Go 版正是这么丢过数据。
fn load_settings(notices: &mut Vec<String>) -> (Settings, Option<SettingsStore>) {
    let store = match SettingsStore::new() {
        Ok(s) => s,
        Err(err) => {
            notices.push(format!("配置目录不可用，本次运行的设置不会被保存：{err}"));
            return (Settings::default(), None);
        }
    };

    // 「新版配置文件还不存在」才是首次运行的判据。
    //
    // 不能用「目标列表为空」代替：用户删光目标后保存的是一份合法的空目标配置，
    // 而旧版 settings.json 是刻意保留不删的（为了能回退），于是下次启动会把他
    // 亲手删掉的目标连同 locale、Bark 地址一起原样倒回来。
    let first_run = store.path().symlink_metadata().is_err();

    match store.load() {
        Ok(settings) => {
            if first_run
                && let Some(legacy) = store.import_legacy()
                && !legacy.targets.is_empty()
            {
                notices.push(format!(
                    "已从旧版设置迁移了 {} 条监控目标。",
                    legacy.targets.len()
                ));
                // 立刻落盘。否则用户不改任何设置时新版文件一直不存在，
                // 每次启动都要重迁一遍，用户删掉的目标也会一直复活。
                if let Err(err) = store.save(&legacy) {
                    notices.push(format!("迁移结果暂时没能保存：{err}"));
                }
                return (legacy, Some(store));
            }
            (settings, Some(store))
        }
        Err(err) => {
            match store.preserve_corrupted() {
                Ok(Some(path)) => {
                    notices.push(format!("读取设置失败，原文件已备份到 {}", path.display()))
                }
                Ok(None) => {}
                Err(backup_err) => {
                    notices.push(format!("读取设置失败，且无法备份原文件：{backup_err}"));
                }
            }
            notices.push(format!(
                "读取设置失败，已回退到默认设置，并且本次运行不会覆盖磁盘上的配置：{err}"
            ));
            (Settings::default(), None)
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            reveal_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let mut notices = Vec::new();
            let (mut settings, store) = load_settings(&mut notices);
            let history_path = SettingsStore::new()
                .map(|store| store.path().with_file_name("inventory-history.sqlite3"));
            let history = match history_path.map_err(|e| e.to_string()).and_then(|path| {
                let app = app.handle().clone();
                history::History::open(path, move |message| {
                    let _ = app.emit(NOTICE_CHANNEL, message);
                })
            }) {
                Ok(history) => Some(Arc::new(history)),
                Err(error) => {
                    notices.push(format!("历史记录未启用：{error}；现有数据库不会被覆盖"));
                    None
                }
            };

            // 旧版本保存的 Apple Watch 目标没有搭档表带，查出来会是假的
            // 「无货」；启动时按目录补齐，下次写盘就带上了。
            let catalog = Catalog::new();
            catalog.attach_companions(&mut settings.targets);
            catalog.attach_locations(&mut settings.targets);

            let browser_enabled = settings.proxies.is_empty() && AppleClient::browser_available();
            let client = AppleClient::new(ClientConfig {
                transport: if browser_enabled {
                    Transport::Browser
                } else {
                    Transport::default_for_build()
                },
                warm_page: if browser_enabled {
                    // Real browser initializes the product page inside the query, once.
                    WarmPage::None
                } else {
                    WarmPage::Bag
                },
                proxies: settings.proxies.clone(),
                ..ClientConfig::default()
            })
            .map_err(|e| format!("构造 Apple 客户端失败：{e}"))?;

            if let Some(history) = history.clone() {
                history.record_network(apw_core::polling::NetworkEvent {
                    at_ms: history::now_ms() as u64,
                    kind: "transport_selected",
                    locale: settings.locale.clone(),
                    route: "direct".into(),
                    request_kind: None,
                    status: None,
                    parts: 0,
                    duration_ms: 0,
                    outcome: client.transport().label(),
                    cooldown_until_ms: None,
                    interval_floor_secs: 0,
                });
                client.set_network_observer(apw_core::polling::NetworkObserver(Arc::new(
                    move |event| {
                        history.record_network(event);
                    },
                )));
            }

            // 用 Watcher::new 而不是 Watcher::spawn：setup 回调跑在主线程上，
            // 并不处在 tokio 运行时上下文里，在这里 tokio::spawn 会 panic，
            // 而且因为发生在不可展开的回调中，进程会直接 abort。
            // 引擎任务交给 Tauri 自己的运行时去驱动。
            let (watcher, events, engine) = Watcher::new(client.clone(), WatcherConfig::default());
            tauri::async_runtime::spawn(engine);

            {
                let watcher = watcher.clone();
                let targets = settings.targets.clone();
                let interval = settings.interval();
                let repeat_in_stock = settings.repeat_in_stock;
                let resume = settings.watching_enabled && !targets.is_empty();
                let checkpoint = history.as_ref().map(|h| h.polling_checkpoint()).transpose();
                let restore_client = client.clone();
                tauri::async_runtime::spawn(async move {
                    match checkpoint {
                        Ok(Some(saved)) => restore_client.restore_polling(&saved).await,
                        Err(error) => eprintln!("无法恢复查询冷却记录：{error}"),
                        _ => {}
                    }
                    watcher.set_targets(targets).await;
                    watcher.set_interval(interval).await;
                    watcher.set_repeat_in_stock(repeat_in_stock).await;
                    if resume {
                        watcher.start().await;
                    }
                });
            }

            app.manage(AppState {
                history,
                operations: tokio::sync::Mutex::new(()),
                watcher,
                client,
                catalog,
                http: reqwest::Client::new(),
                settings: RwLock::new(settings),
                store,
            });

            let handle: AppHandle = app.handle().clone();
            tauri::async_runtime::spawn(pump_events(handle.clone(), events));
            if let Err(error) = web::start(handle.clone()) {
                notices.push(format!("手机网页未启动：{error}"));
            }

            if let Err(err) = setup_tray(&handle) {
                // 托盘建不起来只是少一项能力，不该让程序起不来。
                notices.push(format!("系统托盘不可用：{err}"));
            }

            if !notices.is_empty() {
                // 界面还没订阅上，稍等一下再发。这些是降级说明，用户必须看见 ——
                // 只写到 stderr 是没用的，用户是双击图标启动的。
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    for notice in notices {
                        let _ = handle.emit(NOTICE_CHANNEL, notice);
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 收进托盘而不是退出。真要退出走托盘菜单里的「退出」。
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_regions,
            list_categories,
            list_stores,
            nearby_stores,
            list_products,
            refresh_products,
            get_settings,
            save_settings,
            set_push_urls,
            get_snapshot,
            set_targets,
            target_revision,
            set_interval,
            set_repeat_in_stock,
            set_language,
            web::web_access,
            web::set_lan_enabled,
            start_watching,
            stop_watching,
            is_running,
            test_notify,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 应用启动失败")
        .run(|app, event| {
            if matches!(&event, tauri::RunEvent::Exit)
                && let Some(state) = app.try_state::<AppState>()
                && let Some(history) = &state.history
            {
                history.shutdown();
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                reveal_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

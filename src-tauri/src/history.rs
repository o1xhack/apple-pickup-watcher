//! SQLite 库存历史。独立写入线程不等待推送网络，也不阻塞查询和通知。
//! 仅记录库存与公开商品资料，不记录 Bark 地址、代理、凭证或原始网络错误。
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use apw_core::{
    model::{Availability, Target, UnknownReason},
    notify::Notification,
    polling::NetworkEvent,
    watcher::{Event, TargetState},
};
use rusqlite::{Connection, Transaction, params};
use serde::Serialize;

const SCHEMA_VERSION: i64 = 2;
const QUEUE_CAPACITY: usize = 256;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub path: PathBuf,
    pub session_id: i64,
    pub last_write_ms: Option<i64>,
    /// 保留最近错误，提示该时段历史可能不完整；不会把历史写入失败当作无货。
    pub error: Option<String>,
}

type Report = Arc<dyn Fn(String) + Send + Sync>;

pub struct History {
    sender: mpsc::SyncSender<Job>,
    status: Arc<Mutex<Status>>,
    report: Report,
    sequence: AtomicU64,
    session_id: i64,
}

enum Job {
    Network(NetworkEvent),
    Event {
        event: Box<Event>,
        at_ms: i64,
    },
    Notice {
        id: String,
        state: Box<TargetState>,
        notification: Box<Notification>,
        push_configured: bool,
        at_ms: i64,
    },
    Delivery {
        id: String,
        success: bool,
        at_ms: i64,
    },
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
    Stop(mpsc::Sender<()>),
}

fn report_error(status: &Mutex<Status>, report: &Report, message: String) {
    let mut guard = status.lock().unwrap_or_else(|e| e.into_inner());
    if guard.error.as_ref() != Some(&message) {
        guard.error = Some(message.clone());
        drop(guard);
        report(message);
    }
}

impl History {
    pub fn open(
        path: PathBuf,
        report: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut db = Database::open(&path).map_err(|e| e.to_string())?;
        let session_id = db.begin_session(now_ms()).map_err(|e| e.to_string())?;
        let status = Arc::new(Mutex::new(Status {
            path,
            session_id,
            last_write_ms: None,
            error: None,
        }));
        let report: Report = Arc::new(report);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker_status = status.clone();
        let worker_report = report.clone();
        std::thread::Builder::new()
            .name("inventory-history".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let result = match job {
                        Job::Network(event) => db.record_network(session_id, &event),
                        Job::Event { event, at_ms } => db.record_event(session_id, &event, at_ms),
                        Job::Notice {
                            id,
                            state,
                            notification,
                            push_configured,
                            at_ms,
                        } => db.record_notice(
                            session_id,
                            &id,
                            &state,
                            &notification,
                            push_configured,
                            at_ms,
                        ),
                        Job::Delivery { id, success, at_ms } => db.delivery(&id, success, at_ms),
                        #[cfg(test)]
                        Job::Flush(reply) => {
                            let _ = reply.send(());
                            continue;
                        }
                        Job::Stop(reply) => {
                            if let Err(error) = db.end_session(session_id, now_ms()) {
                                report_error(
                                    &worker_status,
                                    &worker_report,
                                    format!("历史记录结束会话失败：{error}"),
                                );
                            }
                            drop(db);
                            let _ = reply.send(());
                            break;
                        }
                    };
                    match result {
                        Ok(()) => {
                            worker_status
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .last_write_ms = Some(now_ms())
                        }
                        Err(error) => report_error(
                            &worker_status,
                            &worker_report,
                            format!("历史记录保存失败，该时段数据可能不完整：{error}"),
                        ),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            status,
            report,
            sequence: AtomicU64::new(0),
            session_id,
        })
    }

    fn enqueue(&self, job: Job) -> bool {
        match self.sender.try_send(job) {
            Ok(()) => true,
            Err(error) => {
                let reason = match error {
                    mpsc::TrySendError::Full(_) => "写入队列已满",
                    mpsc::TrySendError::Disconnected(_) => "写入线程已停止",
                };
                report_error(
                    &self.status,
                    &self.report,
                    format!("历史记录未保存：{reason}，该时段数据可能不完整"),
                );
                false
            }
        }
    }

    pub fn record_network(&self, event: NetworkEvent) {
        self.enqueue(Job::Network(event));
    }

    pub fn record_event(&self, event: &Event) {
        // StateChanged 可丢，不能作为完整历史来源。每轮可靠快照保存全部目标。
        if matches!(
            event,
            Event::CycleComplete { .. } | Event::RunStateChanged { .. } | Event::Trouble { .. }
        ) {
            self.enqueue(Job::Event {
                event: Box::new(event.clone()),
                at_ms: now_ms(),
            });
        }
    }

    pub fn record_notice(
        &self,
        state: &TargetState,
        notification: &Notification,
        push_configured: bool,
    ) -> Option<String> {
        let id = format!(
            "{}:{}",
            self.session_id,
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        self.enqueue(Job::Notice {
            id: id.clone(),
            state: Box::new(state.clone()),
            notification: Box::new(notification.clone()),
            push_configured,
            at_ms: now_ms(),
        })
        .then_some(id)
    }

    pub fn record_delivery(&self, id: String, success: bool) {
        self.enqueue(Job::Delivery {
            id,
            success,
            at_ms: now_ms(),
        });
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Read only committed lifecycle records. Never load credentials or raw request URLs.
    pub fn polling_checkpoint(&self) -> Result<Vec<apw_core::polling::SavedPolling>, String> {
        use std::collections::HashMap;
        let path = self.status().path;
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        db.busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        let mut stmt = db.prepare("SELECT locale,route,kind,occurred_at_ms,cooldown_until_ms,interval_floor_secs FROM network_events WHERE occurred_at_ms >= ?1 AND kind IN ('cooldown_started','probe_recovered','recovery_step','probe_started') ORDER BY id").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([now_ms() - 24 * 3600 * 1000], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?.max(0) as u64,
                    r.get::<_, Option<i64>>(4)?.map(|v| v.max(0) as u64),
                    r.get::<_, i64>(5)?.max(0) as u64,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut saved: HashMap<(String, String), apw_core::polling::SavedPolling> = HashMap::new();
        for row in rows {
            let (locale, route, kind, at, until, floor) = row.map_err(|e| e.to_string())?;
            let entry = saved
                .entry((locale.clone(), route.clone()))
                .or_insert_with(|| apw_core::polling::SavedPolling {
                    locale,
                    route,
                    ..Default::default()
                });
            entry.interval_floor_secs = floor;
            if kind == "cooldown_started" {
                entry.cooldown_until_ms = until;
                entry.last_block_ms = Some(at);
                entry.cooldown_secs =
                    until.unwrap_or(at).saturating_sub(at).saturating_add(500) / 1000;
            }
            if kind == "probe_recovered" {
                entry.cooldown_until_ms = None;
            }
        }
        Ok(saved.into_values().collect())
    }

    /// 正常退出时先处理已排队的数据。强制杀进程时，已提交事务仍由 SQLite 保护。
    pub fn shutdown(&self) {
        let (sender, receiver) = mpsc::channel();
        match self.sender.try_send(Job::Stop(sender)) {
            Ok(()) if receiver.recv_timeout(Duration::from_secs(5)).is_err() => {
                report_error(
                    &self.status,
                    &self.report,
                    "历史记录退出写入超时，未提交的末尾记录可能缺失".into(),
                );
            }
            Err(mpsc::TrySendError::Full(_)) => {
                report_error(
                    &self.status,
                    &self.report,
                    "历史记录退出时队列已满，未提交的末尾记录可能缺失".into(),
                );
            }
            _ => {}
        }
    }

    #[cfg(test)]
    fn flush(&self) {
        let (sender, receiver) = mpsc::channel();
        self.sender.send(Job::Flush(sender)).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}

fn sql_u64(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

struct Database {
    connection: Connection,
}
impl Database {
    fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidQuery);
        }
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        if version == 0 {
            connection.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE sessions(id INTEGER PRIMARY KEY, started_at_ms INTEGER NOT NULL, ended_at_ms INTEGER, app_version TEXT NOT NULL);
                CREATE TABLE targets(target_key TEXT PRIMARY KEY, locale TEXT NOT NULL, store_number TEXT NOT NULL, part_number TEXT NOT NULL, store_title TEXT NOT NULL, product_name TEXT NOT NULL);
                CREATE TABLE cycles(id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), completed_at_ms INTEGER NOT NULL, healthy INTEGER NOT NULL, next_check_in_secs INTEGER NOT NULL, paced INTEGER NOT NULL, target_count INTEGER NOT NULL);
                CREATE TABLE observations(cycle_id INTEGER NOT NULL REFERENCES cycles(id), target_key TEXT NOT NULL REFERENCES targets(target_key), checked_at_ms INTEGER, availability TEXT NOT NULL CHECK(availability IN ('in_stock','out_of_stock','unknown')), reason TEXT, message TEXT, consecutive_failures INTEGER NOT NULL, PRIMARY KEY(cycle_id,target_key)) WITHOUT ROWID;
                CREATE INDEX observations_target_time ON observations(target_key,checked_at_ms);
                CREATE TABLE events(id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), occurred_at_ms INTEGER NOT NULL, kind TEXT NOT NULL, detail TEXT);
                CREATE TABLE notifications(id TEXT PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), target_key TEXT NOT NULL REFERENCES targets(target_key), checked_at_ms INTEGER, queued_at_ms INTEGER NOT NULL, completed_at_ms INTEGER, title TEXT NOT NULL, body TEXT NOT NULL, product_url TEXT, critical_alert INTEGER NOT NULL, push_configured INTEGER NOT NULL, dispatch_status TEXT NOT NULL CHECK(dispatch_status IN ('queued','dispatch_succeeded','dispatch_failed')));
                CREATE INDEX notifications_target_time ON notifications(target_key,queued_at_ms);
                PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            connection.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE network_events(id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), occurred_at_ms INTEGER NOT NULL, kind TEXT NOT NULL, locale TEXT NOT NULL, route TEXT NOT NULL, request_kind TEXT, http_status INTEGER, parts INTEGER NOT NULL, duration_ms INTEGER NOT NULL, outcome TEXT NOT NULL, cooldown_until_ms INTEGER, interval_floor_secs INTEGER NOT NULL);
                CREATE INDEX network_events_time ON network_events(occurred_at_ms);
                PRAGMA user_version=2; COMMIT;")?;
        }
        Ok(Self { connection })
    }

    fn record_network(&self, session: i64, event: &NetworkEvent) -> rusqlite::Result<()> {
        self.connection.execute("INSERT INTO network_events(session_id,occurred_at_ms,kind,locale,route,request_kind,http_status,parts,duration_ms,outcome,cooldown_until_ms,interval_floor_secs) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![session, sql_u64(event.at_ms)?, event.kind, event.locale, event.route, event.request_kind, event.status, event.parts as i64, sql_u64(event.duration_ms)?, event.outcome, event.cooldown_until_ms.map(sql_u64).transpose()?, sql_u64(event.interval_floor_secs)?])?;
        Ok(())
    }

    fn begin_session(&mut self, at: i64) -> rusqlite::Result<i64> {
        self.connection.execute(
            "INSERT INTO sessions(started_at_ms,app_version) VALUES (?1,?2)",
            params![at, env!("CARGO_PKG_VERSION")],
        )?;
        Ok(self.connection.last_insert_rowid())
    }
    fn end_session(&self, session: i64, at: i64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE sessions SET ended_at_ms=?1 WHERE id=?2",
            params![at, session],
        )?;
        Ok(())
    }
    fn target(transaction: &Transaction<'_>, target: &Target) -> rusqlite::Result<String> {
        let key = target.key().to_string();
        transaction.execute("INSERT INTO targets(target_key,locale,store_number,part_number,store_title,product_name) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(target_key) DO UPDATE SET store_title=excluded.store_title,product_name=excluded.product_name", params![key,target.locale,target.store_number,target.part_number,target.store_title,target.product_name])?;
        Ok(key)
    }
    fn record_event(&mut self, session: i64, event: &Event, at: i64) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        match event {
            Event::CycleComplete {
                next_check_in_secs,
                paced,
                healthy,
                snapshot,
            } => {
                transaction.execute("INSERT INTO cycles(session_id,completed_at_ms,healthy,next_check_in_secs,paced,target_count) VALUES (?1,?2,?3,?4,?5,?6)", params![session,at,healthy,sql_u64(*next_check_in_secs)?,paced,snapshot.len() as i64])?;
                let cycle = transaction.last_insert_rowid();
                for state in snapshot {
                    let key = Self::target(&transaction, &state.target)?;
                    let (availability, reason, message) = match &state.availability {
                        Availability::InStock => ("in_stock", None, None),
                        Availability::OutOfStock => ("out_of_stock", None, None),
                        Availability::Unknown(reason) => {
                            let value =
                                serde_json::to_value(&state.availability).unwrap_or_default();
                            let code = value
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let message = match reason {
                                UnknownReason::PickupNotOpen { message } => Some(message.clone()),
                                _ => None,
                            };
                            ("unknown", Some(code), message)
                        }
                    };
                    transaction.execute("INSERT INTO observations(cycle_id,target_key,checked_at_ms,availability,reason,message,consecutive_failures) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![cycle,key,state.last_checked_ms.map(sql_u64).transpose()?,availability,reason,message,state.consecutive_failures])?;
                }
            }
            Event::RunStateChanged { running } => {
                transaction.execute(
                    "INSERT INTO events(session_id,occurred_at_ms,kind) VALUES (?1,?2,?3)",
                    params![session, at, if *running { "started" } else { "paused" }],
                )?;
            }
            Event::Trouble { advice, .. } => {
                // 网络错误可能含私有代理信息，只保留结构化建议，库存原因见 observations。
                transaction.execute("INSERT INTO events(session_id,occurred_at_ms,kind,detail) VALUES (?1,?2,'trouble',?3)", params![session,at,serde_json::to_string(advice).unwrap_or_default()])?;
            }
            _ => {}
        }
        transaction.commit()
    }
    fn record_notice(
        &mut self,
        session: i64,
        id: &str,
        state: &TargetState,
        notice: &Notification,
        push_configured: bool,
        at: i64,
    ) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        let key = Self::target(&transaction, &state.target)?;
        let url = notice.url.as_deref().filter(|url| {
            url.starts_with("https://www.apple.com/")
                || url.starts_with("https://www.apple.com.cn/")
                || url.starts_with("applestore://www.apple.com/")
                || url.starts_with("applestore://www.apple.com.cn/")
        });
        transaction.execute("INSERT INTO notifications(id,session_id,target_key,checked_at_ms,queued_at_ms,title,body,product_url,critical_alert,push_configured,dispatch_status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'queued')", params![id,session,key,state.last_checked_ms.map(sql_u64).transpose()?,at,notice.title,notice.body,url,notice.critical_alert,push_configured])?;
        transaction.commit()
    }
    fn delivery(&self, id: &str, success: bool, at: i64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE notifications SET completed_at_ms=?1,dispatch_status=?2 WHERE id=?3",
            params![
                at,
                if success {
                    "dispatch_succeeded"
                } else {
                    "dispatch_failed"
                },
                id
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(name: &str) -> Self {
            // 本机 CARGO_TARGET_DIR 指向 StudioSSD BuildScratch，CI 则使用项目 target。
            let root = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
            std::fs::create_dir_all(&root).unwrap();
            Self(root.join(format!(
                "history-{name}-{}-{}.sqlite3",
                std::process::id(),
                now_ms()
            )))
        }
        fn read(&self) -> Connection {
            Connection::open_with_flags(&self.0, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }
    fn state(availability: Availability, checked: u64) -> TargetState {
        TargetState {
            target: serde_json::from_value(serde_json::json!({"locale":"en_US","storeNumber":"R105","storeTitle":"California-Oakridge","partNumber":"SKU","productName":"iPhone 18 Pro Max 1TB Burgundy"})).unwrap(),
            availability, last_checked_ms: Some(checked), consecutive_failures: 0,
        }
    }
    fn cycle(snapshot: Vec<TargetState>, healthy: bool) -> Event {
        Event::CycleComplete {
            snapshot,
            healthy,
            next_check_in_secs: 60,
            paced: false,
        }
    }
    fn count(connection: &Connection, table: &str) -> i64 {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn checkpoint_preserves_block_until_confirmed_recovery() {
        let file = Fixture::new("restart-checkpoint");
        let history = History::open(file.0.clone(), |_| {}).unwrap();
        let at = now_ms() as u64;
        for kind in ["cooldown_started", "probe_started"] {
            history.record_network(NetworkEvent {
                at_ms: at,
                kind,
                locale: "en_US".into(),
                route: "direct".into(),
                request_kind: None,
                status: None,
                parts: 0,
                duration_ms: 0,
                outcome: kind,
                cooldown_until_ms: if kind == "cooldown_started" {
                    Some(at + 600_000)
                } else {
                    None
                },
                interval_floor_secs: 600,
            });
        }
        let (sender, receiver) = mpsc::channel();
        history.sender.send(Job::Flush(sender)).unwrap();
        receiver.recv().unwrap();
        let saved = history.polling_checkpoint().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].cooldown_until_ms, Some(at + 600_000));
        assert_eq!(saved[0].cooldown_secs, 600);
        history.record_network(NetworkEvent {
            at_ms: at + 601_000,
            kind: "probe_recovered",
            locale: "en_US".into(),
            route: "direct".into(),
            request_kind: None,
            status: None,
            parts: 0,
            duration_ms: 0,
            outcome: "probe_recovered",
            cooldown_until_ms: None,
            interval_floor_secs: 600,
        });
        let (sender, receiver) = mpsc::channel();
        history.sender.send(Job::Flush(sender)).unwrap();
        receiver.recv().unwrap();
        assert_eq!(
            history.polling_checkpoint().unwrap()[0].cooldown_until_ms,
            None
        );
        history.shutdown();
    }

    #[test]
    fn v1_migration_keeps_inventory_and_persists_network_lifecycle() {
        let file = Fixture::new("migration-network");
        let original = History::open(file.0.clone(), |_| {}).unwrap();
        original.record_event(&cycle(vec![state(Availability::InStock, 1000)], true));
        original.shutdown();
        let db = Connection::open(&file.0).unwrap();
        db.execute_batch("DROP TABLE network_events; PRAGMA user_version=1;")
            .unwrap();
        drop(db);
        let history = History::open(file.0.clone(), |_| {}).unwrap();
        for (kind, status, until) in [
            ("request", Some(541), None),
            ("cooldown_started", None, Some(302000)),
            ("probe_started", None, None),
            ("probe_recovered", None, None),
        ] {
            history.record_network(NetworkEvent {
                at_ms: 2000,
                kind,
                locale: "us".into(),
                route: "direct".into(),
                request_kind: Some("pickup"),
                status,
                parts: 6,
                duration_ms: 120,
                outcome: kind,
                cooldown_until_ms: until,
                interval_floor_secs: 300,
            });
        }
        history.shutdown();
        let db = file.read();
        assert_eq!(count(&db, "observations"), 1);
        assert_eq!(count(&db, "network_events"), 4);
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT http_status FROM network_events WHERE kind='request'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            541
        );
        assert_eq!(
            db.query_row(
                "SELECT cooldown_until_ms FROM network_events WHERE kind='cooldown_started'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            302000
        );
    }

    #[test]
    fn 重启保留逐轮结果未知原因启停和通知且区分会话() {
        let file = Fixture::new("restart");
        let history = History::open(file.0.clone(), |_| {}).unwrap();
        history.record_event(&Event::RunStateChanged { running: true });
        history.record_event(&cycle(vec![state(Availability::OutOfStock, 1000)], true));
        let unknown = Availability::Unknown(UnknownReason::Blocked {
            detail: "private-proxy-secret".into(),
        });
        history.record_event(&cycle(vec![state(unknown, 2000)], false));
        let available = state(Availability::InStock, 3000);
        let notice = Notification::new(
            "iPhone 18 Pro Max 1TB",
            "勃艮第红色 · Burgundy\nCalifornia-Oakridge",
        )
        .with_url("https://www.apple.com/us/xc/product/SKU")
        .with_critical_alert(true);
        let id = history.record_notice(&available, &notice, true).unwrap();
        history.record_delivery(id, true);
        history.record_event(&cycle(vec![available], true));
        history.record_event(&Event::RunStateChanged { running: false });
        history.shutdown();
        let restarted = History::open(file.0.clone(), |_| {}).unwrap();
        restarted.record_event(&cycle(vec![state(Availability::InStock, 4000)], true));
        restarted.shutdown();
        let db = file.read();
        assert_eq!(count(&db, "sessions"), 2);
        assert_eq!(count(&db, "cycles"), 4);
        assert_eq!(count(&db, "observations"), 4);
        assert_eq!(count(&db, "targets"), 1);
        assert_eq!(count(&db, "notifications"), 1);
        let states: Vec<String> = db
            .prepare("SELECT availability FROM observations ORDER BY cycle_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(states, ["out_of_stock", "unknown", "in_stock", "in_stock"]);
        let reason: String = db
            .query_row(
                "SELECT reason FROM observations WHERE availability='unknown'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "blocked");
        let message: Option<String> = db
            .query_row(
                "SELECT message FROM observations WHERE availability='unknown'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(message, None, "不保存可能含代理凭证的错误原文");
        let notice: (String, bool, String) = db
            .query_row(
                "SELECT dispatch_status,critical_alert,product_url FROM notifications",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            notice,
            (
                "dispatch_succeeded".into(),
                true,
                "https://www.apple.com/us/xc/product/SKU".into()
            )
        );
        assert_eq!(count(&db, "events"), 2);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sessions WHERE ended_at_ms IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn 失败整轮回滚并显式报告但后续通知仍可记录() {
        let file = Fixture::new("rollback");
        let messages = Arc::new(Mutex::new(Vec::new()));
        let report = messages.clone();
        let history = History::open(file.0.clone(), move |message| {
            report.lock().unwrap().push(message)
        })
        .unwrap();
        let available = state(Availability::InStock, 1000);
        history.record_event(&cycle(vec![available.clone(), available.clone()], true));
        let id = history
            .record_notice(
                &available,
                &Notification::new("phone", "store").with_url("https://api.day.app/secret"),
                true,
            )
            .unwrap();
        history.record_delivery(id, false);
        history.flush();
        assert!(history.status().error.is_some());
        assert!(!messages.lock().unwrap().is_empty());
        history.shutdown();
        let db = file.read();
        assert_eq!(count(&db, "cycles"), 0);
        assert_eq!(count(&db, "observations"), 0);
        assert_eq!(count(&db, "notifications"), 1);
        let notice: (String, Option<String>) = db
            .query_row(
                "SELECT dispatch_status,product_url FROM notifications",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(notice, ("dispatch_failed".into(), None));
    }

    #[test]
    fn 异常终止的会话不影响下次继续且不覆盖坏数据库或未来版本() {
        let file = Fixture::new("abrupt");
        {
            let mut db = Database::open(&file.0).unwrap();
            let session = db.begin_session(1000).unwrap();
            db.record_event(
                session,
                &cycle(vec![state(Availability::OutOfStock, 1000)], true),
                1000,
            )
            .unwrap();
            // 不写 ended_at_ms 模拟异常退出：已提交结果应保留。
        }
        let history = History::open(file.0.clone(), |_| {}).unwrap();
        history.shutdown();
        let db = file.read();
        assert_eq!(count(&db, "observations"), 1);
        assert_eq!(count(&db, "sessions"), 2);
        drop(db);
        let future = Fixture::new("future");
        let db = Connection::open(&future.0).unwrap();
        db.pragma_update(None, "user_version", 99).unwrap();
        drop(db);
        assert!(History::open(future.0.clone(), |_| {}).is_err());
        assert_eq!(
            future
                .read()
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            99
        );
        let broken = Fixture::new("broken");
        std::fs::write(&broken.0, b"not a SQLite database").unwrap();
        assert!(History::open(broken.0.clone(), |_| {}).is_err());
        assert_eq!(std::fs::read(&broken.0).unwrap(), b"not a SQLite database");
    }
}

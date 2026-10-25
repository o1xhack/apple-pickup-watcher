//! 只读历史统计。检查数按门店/SKU/实际采样时间去重，不把持续有货当成多次补货。
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub all_targets: Option<bool>,
    pub offset: Option<usize>,
}

pub fn read(path: &Path, session: i64, keys: &[String], query: Query) -> Result<Value, String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    db.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    let started:i64=db.query_row("SELECT coalesce((SELECT max(occurred_at_ms) FROM events WHERE session_id=?1 AND kind='started'),(SELECT started_at_ms FROM sessions WHERE id=?1))",[session],|r|r.get(0)).map_err(|e|e.to_string())?;
    let from = query.from_ms.unwrap_or(started);
    let to = query
        .to_ms
        .unwrap_or_else(crate::history::now_ms)
        .min(crate::history::now_ms());
    if from < 0 || to < from || query.offset.unwrap_or(0) > 10_000 {
        return Err("Invalid statistics range / 统计时间范围无效".into());
    }
    let offset = query.offset.unwrap_or(0);
    let key_json = serde_json::to_string(keys).map_err(|e| e.to_string())?;
    let all = query.all_targets.unwrap_or(false);
    let mut stmt=db.prepare("SELECT o.target_key,o.checked_at_ms,o.availability,o.reason,o.message,t.product_name,t.store_title,t.part_number,t.locale,c.session_id,c.next_check_in_secs,min(o.cycle_id) FROM observations o JOIN targets t USING(target_key) JOIN cycles c ON c.id=o.cycle_id WHERE o.checked_at_ms>=?1 AND o.checked_at_ms<=?2 AND (?3 OR o.target_key IN (SELECT value FROM json_each(?4))) GROUP BY o.target_key,o.checked_at_ms ORDER BY o.target_key,o.checked_at_ms").map_err(|e|e.to_string())?;
    let mut rows = stmt
        .query(params![from, to, all, key_json])
        .map_err(|e| e.to_string())?;
    let mut first_checked: Option<i64> = None;
    let mut last_checked: Option<i64> = None;
    let mut checks = 0u64;
    let mut positive = 0u64;
    let mut out = 0u64;
    let mut pending = 0u64;
    let mut unknown = 0u64;
    let mut candidates = 0u64;
    let mut sightings = 0u64;
    let mut rounds = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut products: BTreeMap<String, Value> = BTreeMap::new();
    let mut stores: BTreeMap<String, Value> = BTreeMap::new();
    let mut recent = BTreeMap::new();
    let mut breaks_stmt=db.prepare("SELECT occurred_at_ms FROM events WHERE kind IN ('started','paused') AND occurred_at_ms>=?1 AND occurred_at_ms<=?2").map_err(|e|e.to_string())?;
    let breaks = breaks_stmt
        .query_map(params![from, to], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut previous: Option<(String, String, i64, i64, i64)> = None;
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let key: String = r.get(0).map_err(|e| e.to_string())?;
        let at: i64 = r.get(1).map_err(|e| e.to_string())?;
        first_checked = Some(first_checked.map_or(at, |v| v.min(at)));
        last_checked = Some(last_checked.map_or(at, |v| v.max(at)));
        let state: String = r.get(2).map_err(|e| e.to_string())?;
        let reason: Option<String> = r.get(3).map_err(|e| e.to_string())?;
        let message: Option<String> = r.get(4).map_err(|e| e.to_string())?;
        let name: String = r.get(5).map_err(|e| e.to_string())?;
        let store: String = r.get(6).map_err(|e| e.to_string())?;
        let part: String = r.get(7).map_err(|e| e.to_string())?;
        let locale: String = r.get(8).map_err(|e| e.to_string())?;
        let sid: i64 = r.get(9).map_err(|e| e.to_string())?;
        let wait: i64 = r.get(10).map_err(|e| e.to_string())?;
        rounds.insert(r.get::<_, i64>(11).map_err(|e| e.to_string())?);
        targets.insert(key.clone());
        checks += 1;
        let continuous = previous.as_ref().is_some_and(|p| {
            p.0 == key
                && p.3 == sid
                && at - p.2 <= p.4.max(1) * 2000
                && breaks
                    .range((
                        std::ops::Bound::Excluded(p.2),
                        std::ops::Bound::Included(at),
                    ))
                    .next()
                    .is_none()
        });
        let mut discovery = None;
        match state.as_str() {
            "in_stock" => {
                positive += 1;
                if !continuous || previous.as_ref().is_none_or(|p| p.1 != "in_stock") {
                    sightings += 1;
                    discovery = Some(
                        if continuous && previous.as_ref().is_some_and(|p| p.1 == "out_of_stock") {
                            candidates += 1;
                            "restock_candidate"
                        } else {
                            "first_or_recovered"
                        },
                    );
                }
            }
            "out_of_stock" => out += 1,
            _ => {
                if reason.as_deref() == Some("pickup_not_open") {
                    pending += 1
                } else {
                    unknown += 1
                }
            }
        }
        let p=products.entry(format!("{locale}|{part}")).or_insert_with(||json!({"productName":name,"partNumber":part,"locale":locale,"checks":0,"inStockChecks":0}));
        p["checks"] = json!(p["checks"].as_u64().unwrap_or(0) + 1);
        if state == "in_stock" {
            p["inStockChecks"] = json!(p["inStockChecks"].as_u64().unwrap_or(0) + 1);
        }
        let st = stores.entry(format!("{locale}|{store}")).or_insert_with(
            || json!({"storeTitle":store,"locale":locale,"checks":0,"inStockChecks":0}),
        );
        st["checks"] = json!(st["checks"].as_u64().unwrap_or(0) + 1);
        if state == "in_stock" {
            st["inStockChecks"] = json!(st["inStockChecks"].as_u64().unwrap_or(0) + 1);
        }
        recent.insert((std::cmp::Reverse(at),key.clone()),json!({"targetKey":key,"checkedAtMs":at,"availability":state,"reason":reason,"message":message,"productName":name,"storeTitle":store,"discovery":discovery}));
        if recent.len() > offset + 100 {
            recent.pop_last();
        }
        previous = Some((key, state, at, sid, wait));
    }
    let history: Vec<_> = recent.into_values().skip(offset).take(100).collect();
    let network:Value=db.query_row("SELECT count(*),coalesce(sum(http_status=541),0),coalesce(sum(http_status>=400),0) FROM network_events WHERE kind='request' AND occurred_at_ms>=?1 AND occurred_at_ms<=?2",params![from,to],|r|Ok(json!({"requests":r.get::<_,i64>(0)?,"blocked541":r.get::<_,i64>(1)?,"errors":r.get::<_,i64>(2)?}))).map_err(|e|e.to_string())?;
    Ok(
        json!({"fromMs":from,"toMs":to,"startedAtMs":started,"timeZone":"America/Los_Angeles","firstCheckedMs":first_checked,"lastCheckedMs":last_checked,"checks":checks,"rounds":rounds.len(),"targets":targets.len(),"inStockChecks":positive,"outOfStockChecks":out,"notOpenChecks":pending,"unknownChecks":unknown,"sightings":sightings,"restockCandidates":candidates,"network":network,"products":products.into_values().collect::<Vec<_>>(),"stores":stores.into_values().collect::<Vec<_>>(),"history":history,"offset":offset,"hasMore":(offset as u64+100)<checks}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deduplicates_samples_and_separates_restock_unknown_pause_and_date_boundaries() {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.build")
            });
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "statistics-{}-{}.sqlite3",
            std::process::id(),
            crate::history::now_ms()
        ));
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE sessions(id INTEGER,started_at_ms INTEGER);INSERT INTO sessions VALUES(1,100);
CREATE TABLE events(session_id INTEGER,occurred_at_ms INTEGER,kind TEXT);INSERT INTO events VALUES(1,100,'started'),(1,6500,'paused'),(1,6600,'started');
CREATE TABLE targets(target_key TEXT,product_name TEXT,store_title TEXT,part_number TEXT,locale TEXT);INSERT INTO targets VALUES('A','iPhone Duo 1TB Star White','Los Gatos','SKU-A','en_US'),('B','iPhone 18 Pro Max 2TB Burgundy','Oakridge','SKU-B','en_US');
CREATE TABLE cycles(id INTEGER,session_id INTEGER,next_check_in_secs INTEGER);
CREATE TABLE observations(cycle_id INTEGER,target_key TEXT,checked_at_ms INTEGER,availability TEXT,reason TEXT,message TEXT);
CREATE TABLE network_events(kind TEXT,occurred_at_ms INTEGER,http_status INTEGER);INSERT INTO network_events VALUES('request',1000,541),('request',2000,200),('cooldown_started',3000,NULL);").unwrap();
            for (i, (at, state)) in [
                (1000, "out_of_stock"),
                (2000, "in_stock"),
                (2000, "in_stock"),
                (3000, "in_stock"),
                (4000, "unknown"),
                (5000, "in_stock"),
                (6000, "out_of_stock"),
                (7000, "in_stock"),
                (8000, "out_of_stock"),
                (9000, "in_stock"),
            ]
            .into_iter()
            .enumerate()
            {
                db.execute("INSERT INTO cycles VALUES(?1,1,90)", [i as i64 + 1])
                    .unwrap();
                db.execute(
                    "INSERT INTO observations VALUES(?1,'A',?2,?3,?4,NULL)",
                    params![
                        i as i64 + 1,
                        at,
                        state,
                        if state == "unknown" {
                            Some("blocked")
                        } else {
                            None
                        }
                    ],
                )
                .unwrap();
            }
            db.execute(
                "INSERT INTO observations VALUES(1,'B',1000,'in_stock',NULL,NULL)",
                [],
            )
            .unwrap();
        }
        let keys = vec!["A".to_string()];
        let d = read(
            &path,
            1,
            &keys,
            Query {
                from_ms: Some(0),
                to_ms: Some(10000),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(d["checks"], 9);
        assert_eq!(d["rounds"], 9);
        assert_eq!(d["inStockChecks"], 5);
        assert_eq!(d["unknownChecks"], 1);
        assert_eq!(d["restockCandidates"], 2);
        assert_eq!(d["sightings"], 4);
        assert_eq!(d["stores"].as_array().unwrap().len(), 1);
        assert_eq!(d["network"]["requests"], 2);
        assert_eq!(d["network"]["blocked541"], 1);
        assert_eq!(d["network"]["errors"], 1);
        let all = read(
            &path,
            1,
            &keys,
            Query {
                from_ms: Some(0),
                all_targets: Some(true),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(all["checks"], 10);
        let run = read(&path, 1, &keys, Query::default()).unwrap();
        assert_eq!(run["fromMs"], 6600);
        assert_eq!(run["checks"], 3);
        let range = read(
            &path,
            1,
            &keys,
            Query {
                from_ms: Some(2500),
                to_ms: Some(5500),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(range["checks"], 3);
        assert_eq!(range["restockCandidates"], 0);
        assert!(
            read(
                &path,
                1,
                &keys,
                Query {
                    from_ms: Some(5000),
                    to_ms: Some(1000),
                    ..Query::default()
                }
            )
            .is_err()
        );
        let empty = read(&path, 1, &[], Query::default()).unwrap();
        assert_eq!(empty["checks"], 0);
        std::fs::remove_file(path).unwrap();
    }
}

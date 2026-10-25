use apw_core::model::Target;

/// 新增目标立即运行；单纯调整提醒、删除部分目标保留暂停意图；清空后停止。
pub fn after_target_change(previous: &[Target], next: &[Target], enabled: bool) -> bool {
    !next.is_empty()
        && (enabled
            || next
                .iter()
                .any(|target| !previous.iter().any(|old| old.key() == target.key())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(part: &str) -> Target {
        serde_json::from_value(serde_json::json!({
            "locale":"en_US", "storeNumber":"R105", "storeTitle":"Oakridge",
            "partNumber":part, "productName":"iPhone"
        }))
        .unwrap()
    }

    #[test]
    fn 添加自动开始而暂停意图在修改提醒时保留() {
        let old = target("one");
        assert!(after_target_change(&[], std::slice::from_ref(&old), false));
        let mut edited = old.clone();
        edited.critical_alert = true;
        assert!(!after_target_change(
            std::slice::from_ref(&old),
            std::slice::from_ref(&edited),
            false
        ));
        assert!(after_target_change(
            std::slice::from_ref(&old),
            &[edited, target("two")],
            false
        ));
    }

    #[test]
    fn 清空停止而删除部分目标保持运行() {
        let old = vec![target("one"), target("two")];
        assert!(!after_target_change(&old, &[], true));
        assert!(after_target_change(&old, &old[..1], true));
        assert!(!after_target_change(&old, &old[..1], false));
    }
}

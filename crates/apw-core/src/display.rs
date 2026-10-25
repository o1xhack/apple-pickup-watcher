//! 仅用于显示的中英颜色名，保留 Apple 原始目录与零件号。
use std::{collections::BTreeMap, sync::LazyLock};

pub static COLOR_NAMES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/color-names.json")).expect("内嵌颜色映射必须有效")
});

pub fn product_label(title: &str) -> String {
    let color = COLOR_NAMES
        .keys()
        .filter(|color| title == color.as_str() || title.ends_with(&format!(" {color}")))
        .max_by_key(|color| color.len());
    match color {
        Some(color) => {
            let prefix = &title[..title.len() - color.len()];
            format!("{prefix}{} · {color}", COLOR_NAMES[color])
        }
        None => title.into(),
    }
}

/// 通知标题优先突出机型与容量，正文按颜色、门店排列；链接由通知渠道独立呈现。
pub fn stock_notice(
    target: &crate::model::Target,
    product: Option<&crate::model::Product>,
) -> (String, String) {
    let color = product
        .map(|p| p.color.as_str())
        .filter(|c| !c.is_empty())
        .or_else(|| {
            COLOR_NAMES
                .keys()
                .filter(|c| target.product_name.ends_with(&format!(" {c}")))
                .max_by_key(|c| c.len())
                .map(String::as_str)
        });
    let mut title = target.product_name.clone();
    let mut lines = Vec::new();
    if let Some(color) = color {
        if let Some(prefix) = title.strip_suffix(&format!(" {color}")) {
            title = prefix.to_string();
        }
        let label = COLOR_NAMES
            .get(color)
            .map_or_else(|| color.to_string(), |zh| format!("{zh} · {color}"));
        lines.push(label);
    }
    lines.push(target.store_title.clone());
    (title, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn 到货通知机型容量作标题颜色门店分行且保留重要警告目标() {
        let target: crate::model::Target = serde_json::from_value(serde_json::json!({
            "locale":"en_US","storeNumber":"R105","storeTitle":"California-Los Gatos",
            "partNumber":"SKU","productName":"iPhone 18 Pro Max 1TB Burgundy","criticalAlert":true
        }))
        .unwrap();
        let (title, body) = stock_notice(&target, None);
        assert_eq!(title, "iPhone 18 Pro Max 1TB");
        assert_eq!(body, "勃艮第红色 · Burgundy\nCalifornia-Los Gatos");
        assert!(target.critical_alert);
        let mut unknown = target.clone();
        unknown.product_name = "iPhone Air 512GB Future Color".into();
        let product: crate::model::Product = serde_json::from_value(serde_json::json!({
            "partNumber":"SKU","category":"iphone","family":"iphoneair","capacity":"512GB",
            "color":"Future Color","title":unknown.product_name
        }))
        .unwrap();
        assert_eq!(
            stock_notice(&unknown, Some(&product)),
            (
                "iPhone Air 512GB".into(),
                "Future Color\nCalifornia-Los Gatos".into()
            )
        );
        assert_eq!(stock_notice(&unknown, None).0, unknown.product_name);
    }

    #[test]
    fn 美国通知指向对应零件号而其他地区保留购物袋() {
        let us = crate::model::region_by_locale("en_US").unwrap();
        assert_eq!(
            us.notification_url("MG194LL/A"),
            "https://www.apple.com/us/xc/product/MG194LL/A"
        );
        let cn = crate::model::region_by_locale("zh_CN").unwrap();
        assert_eq!(cn.notification_url("SKU"), cn.bag_url());
    }

    #[test]
    fn 保留完整颜色名且未知与中文不改写() {
        assert_eq!(
            product_label("iPhone Air 256GB Cloud White"),
            "iPhone Air 256GB 云白色 · Cloud White"
        );
        assert_eq!(
            product_label("iPhone Duo 1TB Star White"),
            "iPhone Duo 1TB 星白色 · Star White"
        );
        assert_eq!(product_label("iPhone Air 黑色"), "iPhone Air 黑色");
        assert_eq!(product_label("iPhone Future Color"), "iPhone Future Color");
    }
}

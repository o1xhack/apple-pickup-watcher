//! 通知商品链接。不把付款预选当作换购、运营商或 checkout 已完成。
use serde::{Deserialize, Serialize};

use crate::{
    config::Settings,
    model::{Category, Product, Target, region_by_locale},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkMode {
    App,
    #[default]
    Web,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentPreference {
    #[default]
    None,
    AppleCardMonthlyInstallments,
}

pub fn notification_url(
    target: &Target,
    product: Option<&Product>,
    settings: &Settings,
) -> Option<String> {
    let region = region_by_locale(&target.locale)?;
    let mut url = region.notification_url(&target.part_number);
    if settings.notification_link_mode == LinkMode::App {
        return Some(url.replacen("https://", "applestore://", 1));
    }
    let iphone = product.map_or_else(
        || target.product_name.starts_with("iPhone "),
        |p| p.category == Category::Iphone,
    );
    if target.locale == "en_US"
        && iphone
        && settings.payment_preference == PaymentPreference::AppleCardMonthlyInstallments
    {
        url.push_str("?purchaseOption=bfi&purchaseOptionId=bfi");
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target() -> Target {
        serde_json::from_value(serde_json::json!({"locale":"en_US","storeNumber":"R105","storeTitle":"Oakridge","partNumber":"MG194LL/A","productName":"iPhone Air 256GB Cloud White"})).unwrap()
    }
    #[test]
    fn 旧配置保持网页且不预选付款并能往返保存选择() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            notification_url(&target(), None, &settings).unwrap(),
            "https://www.apple.com/us/xc/product/MG194LL/A"
        );
        let configured = Settings {
            notification_link_mode: LinkMode::App,
            payment_preference: PaymentPreference::AppleCardMonthlyInstallments,
            ..settings
        };
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&configured).unwrap()).unwrap();
        assert_eq!(configured, restored);
        assert_eq!(
            notification_url(&target(), None, &restored).unwrap(),
            "applestore://www.apple.com/us/xc/product/MG194LL/A"
        );
    }
    #[test]
    fn 网页分期只用于美国手机且不伪造换购() {
        let settings = Settings {
            payment_preference: PaymentPreference::AppleCardMonthlyInstallments,
            ..Settings::default()
        };
        assert_eq!(
            notification_url(&target(), None, &settings).unwrap(),
            "https://www.apple.com/us/xc/product/MG194LL/A?purchaseOption=bfi&purchaseOptionId=bfi"
        );
        let mut other = target();
        other.product_name = "MacBook Pro".into();
        assert!(
            !notification_url(&other, None, &settings)
                .unwrap()
                .contains("purchaseOption")
        );
        other.product_name = target().product_name;
        other.locale = "zh_CN".into();
        assert_eq!(
            notification_url(&other, None, &settings).unwrap(),
            region_by_locale("zh_CN").unwrap().bag_url()
        );
        other.locale = "unknown".into();
        assert_eq!(notification_url(&other, None, &settings), None);
    }
}

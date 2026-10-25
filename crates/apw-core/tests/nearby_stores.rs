//! 门店发现按距离排序，不能被商品是否有货影响。
use apw_core::apple::{ApiError, parse_nearby_stores};

#[test]
fn 数值距离排序而不是按库存顺序或字符串排序() {
    let stores = parse_nearby_stores(br#"{"head":{"status":"200"},"body":{"stores":[
      {"storeNumber":"R3","storeName":"Far","storedistance":"12.2","storeDistanceWithUnit":"12.2 mi"},
      {"storeNumber":"R1","storeName":"Near","city":"San Jose","storedistance":3.52,"storeDistanceWithUnit":"3.52 mi","partsAvailability":{}},
      {"storeNumber":"R2","storeName":"Middle","storedistance":"3.58"},
      {"storeNumber":"R4","storeName":"Unknown"}
    ]}}"#).unwrap();
    assert_eq!(
        stores.iter().map(|s| s.number.as_str()).collect::<Vec<_>>(),
        ["R1", "R2", "R3", "R4"]
    );
    assert_eq!(stores[0].distance, Some(3.52));
    assert_eq!(stores[0].title, "San Jose-Near");
    assert_eq!(stores[0].distance_label.as_deref(), Some("3.52 mi"));
    assert_eq!(stores[3].distance, None);
}

#[test]
fn 错误或空结果不能显示成邮编附近没有门店() {
    assert!(matches!(
        parse_nearby_stores(br#"{"head":{"status":"200"},"body":{"stores":[]}}"#),
        Err(ApiError::Apple(_))
    ));
    assert!(parse_nearby_stores(br#"{"head":{"status":"541"},"body":{"stores":[]}}"#).is_err());
    assert!(
        parse_nearby_stores(br#"{"body":{"stores":[{"storeNumber":"","storeName":"Missing"}]}}"#)
            .is_err()
    );
}

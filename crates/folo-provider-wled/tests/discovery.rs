//! Discovery: what an mDNS answer means, the kind's discover call with the network replaced,
//! and manual entry.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use folo_protocol::generated::ProviderInstanceKind;
use folo_provider::{BoxFuture, ProviderError, ProviderKind, Registry};
use folo_provider_wled::testing::FakeDevice;
use folo_provider_wled::{
    Browse, ServiceAnswer, WledKind, device_from_answer, instance_name, probe,
};
use serde_json::json;

fn answer(instance: &str, port: u16, addresses: Vec<IpAddr>, mac: Option<&str>) -> ServiceAnswer {
    ServiceAnswer {
        instance: instance.to_owned(),
        port,
        addresses,
        properties: mac
            .map(|m| vec![("mac".to_owned(), m.to_owned())])
            .unwrap_or_default(),
    }
}

fn v4(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 168, 1, last))
}

#[test]
fn an_answer_becomes_a_device_identified_by_its_mac() {
    let device = device_from_answer(&answer(
        "wled-aabbcc",
        80,
        vec![v4(20)],
        Some("AABBCCDDEEFF"),
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&device).unwrap(),
        json!({"kind": "wled", "id": "aabbccddeeff", "name": "wled-aabbcc", "address": "192.168.1.20"})
    );
}

#[test]
fn without_a_mac_the_instance_name_is_the_id() {
    let device = device_from_answer(&answer("wled-aabbcc", 80, vec![v4(20)], None)).unwrap();
    assert_eq!(device.id.as_str(), "wled-aabbcc");
}

#[test]
fn a_port_other_than_80_is_part_of_the_address_and_ipv4_comes_first() {
    let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let device = device_from_answer(&answer("w", 8080, vec![v6, v4(7)], None)).unwrap();
    assert_eq!(device.address.as_str(), "192.168.1.7:8080");
    let device = device_from_answer(&answer("w", 80, vec![v6], None)).unwrap();
    assert_eq!(device.address.as_str(), "[::1]");
}

#[test]
fn an_answer_without_an_address_is_no_device() {
    assert!(device_from_answer(&answer("w", 80, vec![], None)).is_none());
}

#[test]
fn instance_names_come_out_of_full_names() {
    assert_eq!(
        instance_name("wled-aabbcc._wled._tcp.local."),
        "wled-aabbcc"
    );
    assert_eq!(instance_name("plain"), "plain");
}

struct Stub(Vec<ServiceAnswer>);

impl Browse for Stub {
    fn browse(&self) -> BoxFuture<'_, Result<Vec<ServiceAnswer>, ProviderError>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

struct Failing;

impl Browse for Failing {
    fn browse(&self) -> BoxFuture<'_, Result<Vec<ServiceAnswer>, ProviderError>> {
        Box::pin(async { Err(ProviderError::Failed("no multicast".into())) })
    }
}

#[tokio::test]
async fn the_kind_discovers_what_the_browser_hears() {
    let kind = WledKind::with_browser(Arc::new(Stub(vec![
        answer("a", 80, vec![v4(1)], Some("01")),
        answer("no-address", 80, vec![], None),
        answer("b", 80, vec![v4(2)], Some("02")),
    ])));
    let found = kind.discover().await.unwrap();
    let ids: Vec<&str> = found.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["01", "02"]);
}

#[tokio::test]
async fn the_registry_gathers_wled_devices_and_logs_a_failing_browser() {
    let mut registry = Registry::new();
    registry.register(Arc::new(WledKind::with_browser(Arc::new(Stub(vec![
        answer("a", 80, vec![v4(1)], None),
    ])))));
    let found = registry.discover(&[ProviderInstanceKind::Wled]).await;
    assert_eq!(found.len(), 1);

    let mut registry = Registry::new();
    registry.register(Arc::new(WledKind::with_browser(Arc::new(Failing))));
    assert!(
        registry
            .discover(&[ProviderInstanceKind::Wled])
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn manual_entry_asks_the_host_what_it_is() {
    let device = FakeDevice::fixture("rgb-strip");
    let found = probe(&device).await.unwrap();
    assert_eq!(
        serde_json::to_value(&found).unwrap(),
        json!({"kind": "wled", "id": "aabbccddeeff", "name": "WLED Kitchen", "address": "wled.test"})
    );
}

#[tokio::test]
async fn a_host_that_is_not_wled_is_refused() {
    let device = FakeDevice::fixture("rgb-strip");
    device.edit(|info, _, _| info["brand"] = json!("Other"));
    assert!(probe(&device).await.is_err());
}

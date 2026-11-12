//! The iroh types Holon persists (`device.key`, ticket strings, the peers
//! sidecar) keep their bytes across iroh upgrades. The fixtures below were
//! written by iroh 0.96.1; a round trip alone cannot see a format change, so
//! each fixture must also decode, and re-encode byte for byte.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use holon_conditions::condition_bus::ConditionBus;
use iroh::EndpointAddr;
use iroh::RelayUrl;
use iroh::SecretKey;
use iroh::TransportAddr;
use proptest::prelude::*;
use tempfile::TempDir;

use crate::device_key_store::load_or_create_device_key;
use crate::share_enrollment::CapabilitySecret;
use crate::share_enrollment::ExpiryTime;
use crate::shared_snapshot_store::SharedSnapshotStore;
use crate::ticket::Ticket;

const DEVICE_KEY_HEX: &str = "030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dc";
const NODE_ID_HEX: &str = "755c4cb9256ca7cdc4acfdc6cfeeda849017e5b9f9514e99191bd67e0b0d4276";
const OTHER_ID_HEX: &str = "274ed6b26805d2fe8d060529ee9a2f28763b6976d9c5fd1dee38ff36b3b76939";
const PEERS_JSON: &str = r#"[{"id":"755c4cb9256ca7cdc4acfdc6cfeeda849017e5b9f9514e99191bd67e0b0d4276","addrs":[{"Relay":"https://relay.example.org./"},{"Ip":"192.168.1.20:41641"},{"Ip":"[fd00::1:2]:41642"}]},{"id":"274ed6b26805d2fe8d060529ee9a2f28763b6976d9c5fd1dee38ff36b3b76939","addrs":[]}]"#;
const TICKET: &str = "eyJ2IjoyLCJzaGFyZWRfdHJlZV9pZCI6InNoYXJlLWZpeHR1cmUiLCJhZGRyIjp7ImlkIjoiNzU1YzRjYjkyNTZjYTdjZGM0YWNmZGM2Y2ZlZWRhODQ5MDE3ZTViOWY5NTE0ZTk5MTkxYmQ2N2UwYjBkNDI3NiIsImFkZHJzIjpbeyJSZWxheSI6Imh0dHBzOi8vcmVsYXkuZXhhbXBsZS5vcmcuLyJ9LHsiSXAiOiIxOTIuMTY4LjEuMjA6NDE2NDEifSx7IklwIjoiW2ZkMDA6OjE6Ml06NDE2NDIifV19LCJhbHBuIjoibG9yby1zeW5jL3NoYXJlLWZpeHR1cmUiLCJjYXBhYmlsaXR5IjoiQ1FrSkNRa0pDUWtKQ1FrSkNRa0pDUWtKQ1FrSkNRa0pDUWtKQ1FrSkNRayIsImV4cGlyZXNfYXQiOjE5MDAwMDAwMDB9";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> [u8; 32] {
    let v: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    v.try_into().unwrap()
}

fn fixture_dial_addrs() -> BTreeSet<TransportAddr> {
    BTreeSet::from([
        TransportAddr::Relay("https://relay.example.org./".parse().unwrap()),
        TransportAddr::Ip("192.168.1.20:41641".parse().unwrap()),
        TransportAddr::Ip("[fd00::1:2]:41642".parse().unwrap()),
    ])
}

fn store_in(dir: &TempDir) -> SharedSnapshotStore {
    SharedSnapshotStore::new(dir.path().to_path_buf(), Arc::new(ConditionBus::new()))
}

#[test]
fn a_device_key_written_by_the_previous_iroh_keeps_its_node_id() {
    let dir = TempDir::new().unwrap();
    let bytes = unhex32(DEVICE_KEY_HEX);
    std::fs::write(dir.path().join("device.key"), bytes).unwrap();

    let key = load_or_create_device_key(dir.path()).unwrap();

    assert_eq!(key.to_bytes(), bytes);
    assert_eq!(hex(key.public().as_bytes()), NODE_ID_HEX);
}

#[test]
fn a_ticket_written_by_the_previous_iroh_decodes_and_re_encodes_byte_equal() {
    let ticket = Ticket::decode(TICKET).unwrap();

    assert_eq!(hex(ticket.addr.id.as_bytes()), NODE_ID_HEX);
    assert_eq!(ticket.addr.addrs, fixture_dial_addrs());
    assert_eq!(ticket.capability, CapabilitySecret::from_bytes([9u8; 32]));
    assert_eq!(ticket.expires_at, ExpiryTime(1_900_000_000));
    assert_eq!(ticket.encode().unwrap(), TICKET);
}

#[test]
fn a_peers_sidecar_written_by_the_previous_iroh_loads_and_re_saves_byte_equal() {
    let dir = TempDir::new().unwrap();
    let store = store_in(&dir);
    let path = store.peers_path("share-fixture");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, PEERS_JSON).unwrap();

    let peers = store.load_peers("share-fixture").unwrap();

    assert_eq!(peers.len(), 2);
    assert_eq!(hex(peers[0].id.as_bytes()), NODE_ID_HEX);
    assert_eq!(peers[0].addrs, fixture_dial_addrs());
    assert_eq!(hex(peers[1].id.as_bytes()), OTHER_ID_HEX);
    assert!(peers[1].addrs.is_empty());

    store.save_peers("share-fixture", &peers).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PEERS_JSON);
}

fn endpoint_addr(key_bytes: [u8; 32], addrs: Vec<TransportAddr>) -> EndpointAddr {
    EndpointAddr::from_parts(SecretKey::from_bytes(&key_bytes).public(), addrs)
}

/// Every stored writer round-trips the key and the address set it was given.
fn assert_stored_formats_round_trip(key_bytes: [u8; 32], addr: EndpointAddr) {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("device.key"), key_bytes).unwrap();
    let key = load_or_create_device_key(dir.path()).unwrap();
    assert_eq!(key.to_bytes(), key_bytes);
    assert_eq!(key.public(), SecretKey::from_bytes(&key_bytes).public());

    let ticket = Ticket::new(
        "share-prop".into(),
        addr.clone(),
        "loro-sync/share-prop".into(),
        CapabilitySecret::from_bytes(key_bytes),
        ExpiryTime(7),
    );
    let decoded = Ticket::decode(&ticket.encode().unwrap()).unwrap();
    assert_eq!(decoded, ticket);

    let store = store_in(&dir);
    let peers = vec![addr.clone(), EndpointAddr::new(key.public())];
    store.save_peers("share-prop", &peers).unwrap();
    assert_eq!(store.load_peers("share-prop").unwrap(), peers);
}

fn arb_transport_addr() -> impl Strategy<Value = TransportAddr> {
    prop_oneof![
        any::<(std::net::Ipv4Addr, u16)>().prop_map(|a| TransportAddr::Ip(SocketAddr::from(a))),
        any::<(std::net::Ipv6Addr, u16)>().prop_map(|a| TransportAddr::Ip(SocketAddr::from(a))),
        (0u8..4).prop_map(|n| {
            let url: RelayUrl = format!("https://relay{n}.example.org./").parse().unwrap();
            TransportAddr::Relay(url)
        }),
    ]
}

#[test]
fn stored_formats_round_trip_for_each_address_shape() {
    let v4 = TransportAddr::Ip("10.0.0.1:1".parse().unwrap());
    let v6 = TransportAddr::Ip("[::1]:65535".parse().unwrap());
    let relay = TransportAddr::Relay("https://relay0.example.org./".parse().unwrap());
    let shapes = [
        vec![],
        vec![v4.clone()],
        vec![v6.clone()],
        vec![relay.clone()],
        vec![v4, v6, relay],
    ];
    for (i, addrs) in shapes.into_iter().enumerate() {
        let key_bytes = [i as u8; 32];
        assert_stored_formats_round_trip(key_bytes, endpoint_addr(key_bytes, addrs));
    }
}

proptest! {
    #[test]
    fn stored_formats_round_trip(
        key_bytes in any::<[u8; 32]>(),
        addrs in proptest::collection::vec(arb_transport_addr(), 0..6),
    ) {
        assert_stored_formats_round_trip(key_bytes, endpoint_addr(key_bytes, addrs));
    }
}

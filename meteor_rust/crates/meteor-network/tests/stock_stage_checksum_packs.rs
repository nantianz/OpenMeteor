#![cfg(feature = "original-assets")]
#![cfg(not(target_arch = "wasm32"))]
use meteor_network::stock_game::{
    StockGameClient, StockGameEvent, StockGameServer, StockHeroCatalog,
};
use meteor_network::stock_session::{StockClientTransport, StockServerTransport};
use meteor_network::stock_udp::StockConnectRecord;

#[test]
fn packed_stage_checksums_admit_production_clients() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
    meteor_asset::runtime_assets::initialize_native(&root).unwrap();
    let checksums = meteor_asset::runtime_assets::stock_stage_checksums().unwrap();
    assert_eq!(checksums.len(), 22);
    for stage in ["sn01", "sn03", "sn22"] {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "packed-stage").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::packed_revision().unwrap());
        server.set_stage_id(stage);
        let transport =
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap();
        let mut client = StockGameClient::for_packed_revision(transport, 0, 0, 6).unwrap();
        let transport =
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap();
        let mut observer = StockGameClient::for_packed_revision(transport, 1, 0, 6).unwrap();
        for now in 0..300 {
            observer.tick(now).unwrap();
            let events = client.tick(now).unwrap();
            assert!(!events.iter().any(|e| matches!(
                e,
                StockGameEvent::StageChecksumRejected(_) | StockGameEvent::PeerDisconnected { .. }
            )));
            server.tick(now).unwrap();
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert!(client.actor_id().is_some(), "{stage}");
        assert_eq!(server.ready_peer_count(), 2);
        let rejected_actor = client.transport_id().unwrap();
        client
            .transport_mut()
            .send_application(
                meteor_network::stock_protocol::STOCK_REQUEST_STAGE_CHECKSUM,
                (checksums[stage] ^ 1).to_le_bytes(),
                1,
                300,
            )
            .unwrap();
        let mut saw_broadcast = false;
        for now in 300..600 {
            server.tick(now).unwrap();
            client.tick(now).unwrap();
            for event in observer.tick(now).unwrap() {
                match event {
                    StockGameEvent::StageChecksumRejected(rejected) => {
                        assert_eq!(rejected.actor_id, rejected_actor);
                        saw_broadcast = true;
                    }
                    StockGameEvent::PeerDisconnected { .. } => {
                        panic!("must not disconnect the observer")
                    }
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert!(saw_broadcast);
        assert_eq!(server.ready_peer_count(), 1);
        assert_eq!(server.transport().peer_addresses().len(), 1);
    }
}

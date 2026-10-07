use super::{redb::RedbBlockStore, sqlite::SqLiteBlockStore, BlockStore};
use crate::nodeclient::sync::BlockHeader;
use pallas_crypto::{hash::Hash, nonce::generate_rolling_nonce};
use pallas_network::miniprotocols::Point;

pub(crate) fn header(number: u64, slot: u64, hash: u8, parent: u8) -> BlockHeader {
    BlockHeader {
        block_number: number,
        slot_number: slot,
        hash: vec![hash; 32],
        prev_hash: vec![parent; 32],
        node_vkey: vec![4; 32],
        node_vrf_vkey: vec![5; 32],
        block_vrf_0: vec![],
        block_vrf_1: vec![],
        eta_vrf_0: vec![hash; 64],
        eta_vrf_1: vec![6; 80],
        leader_vrf_0: vec![7; 64],
        leader_vrf_1: vec![8; 80],
        block_size: 1,
        block_body_hash: vec![9; 32],
        pool_opcert: vec![10; 32],
        unknown_0: 0,
        unknown_1: 0,
        unknown_2: vec![11; 64],
        protocol_major_version: 2,
        protocol_minor_version: 0,
    }
}

pub(crate) struct Fixture(pub(crate) std::path::PathBuf);
impl Fixture {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cncli-canonical-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn canonical_chain_matches_independent_state() {
    for redb in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.0.join("chain.db");
        let mut store: Box<dyn BlockStore + Send> = if redb {
            Box::new(RedbBlockStore::new(&path).unwrap())
        } else {
            Box::new(SqLiteBlockStore::new(&path).unwrap())
        };
        let genesis = "00".repeat(32);
        assert_eq!(store.get_tip_slot_number().unwrap(), 0);
        assert!(store.load_blocks().unwrap().is_empty());
        store.save_block(&[], "invalid").unwrap();
        assert!(store.save_block(&[header(0, 10, 1, 0)], &genesis).is_err());
        let a = header(1, 10, 1, 0);
        let b = header(2, 20, 2, 1);
        let c = header(3, 100, 3, 2);
        store.save_block(&[a.clone(), b.clone(), c], &genesis).unwrap();
        let before = store.load_blocks().unwrap();
        assert!(store.rollback_to(&Point::Specific(20, vec![99; 32])).is_err());
        assert_eq!(store.load_blocks().unwrap(), before);
        store.rollback_to(&Point::Specific(20, vec![2; 32])).unwrap();
        assert_eq!(store.get_tip_slot_number().unwrap(), 20);
        assert_eq!(store.load_blocks().unwrap(), vec![(20, vec![2; 32]), (10, vec![1; 32])]);
        assert!(
            store
                .find_block_by_hash(&hex::encode([3; 32]))
                .unwrap()
                .unwrap()
                .orphaned
        );
        assert!(store.save_block(&[header(4, 110, 4, 3)], &genesis).is_err());
        assert_eq!(store.get_tip_slot_number().unwrap(), 20);
        let replacement = header(3, 90, 12, 2);
        let expected = generate_rolling_nonce(
            generate_rolling_nonce(generate_rolling_nonce(Hash::from([0; 32]), &a.eta_vrf_0), &b.eta_vrf_0),
            &replacement.eta_vrf_0,
        );
        store.save_block(&[replacement], &genesis).unwrap();
        assert_eq!(store.get_tip_slot_number().unwrap(), 90);
        assert_eq!(store.get_eta_v_before_slot(91).unwrap(), expected);
        assert_eq!(store.get_prev_hash_before_slot(91).unwrap(), Hash::from([2; 32]));
        assert!(store.get_eta_v_before_slot(10).is_err());
        store.rollback_to(&Point::Origin).unwrap();
        assert_eq!(store.get_tip_slot_number().unwrap(), 0);
        assert!(store.load_blocks().unwrap().is_empty());
        assert!(
            store
                .find_block_by_hash(&hex::encode([1; 32]))
                .unwrap()
                .unwrap()
                .orphaned
        );
        store.save_block(&[header(42, 200, 42, 41)], &genesis).unwrap();
        assert_eq!(
            store.get_eta_v_before_slot(201).unwrap(),
            generate_rolling_nonce(Hash::from([0; 32]), &[42; 64])
        );
        drop(store);
    }
}

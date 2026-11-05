use exa_protocol::{ClientMessage, ServerMessage};

#[test]
fn streaming_messages_preserve_signed_coordinates() {
    for m in [
        ServerMessage::UnloadColumn {
            column: [-2048, 2047],
        },
        ServerMessage::Chunk3d {
            origin: [-16, 112, -32],
            blocks: (0..4096)
                .map(|i| (i % exa_protocol::BLOCK_IDS.len()) as u8)
                .collect(),
        },
    ] {
        assert_eq!(
            rmp_serde::from_slice::<ServerMessage>(&rmp_serde::to_vec(&m).unwrap()).unwrap(),
            m
        );
    }
    assert_eq!(exa_protocol::column_at([-0.1, 7.0, -16.1]), [-1, -2]);
}

#[test]
fn block_edits_and_item_mapping_roundtrip() {
    use exa_protocol::*;
    for place in [false, true] {
        let message = ClientMessage::EditBlock(BlockEdit {
            target: [15, 7, 16],
            expected: STONE,
            yaw: 0.0,
            pitch: -0.5,
            place,
            slot: 8,
            inventory_revision: 42,
        });
        assert_eq!(
            rmp_serde::from_slice::<ClientMessage>(&rmp_serde::to_vec(&message).unwrap()).unwrap(),
            message
        );
    }
    for block in GRASS..BLOCK_IDS.len() as u8 {
        if let Some(name) = block_item_name(block) {
            assert_eq!(item_block(name), Some(block));
        }
    }
    assert_eq!(block_item_name(AIR), None);
    assert_eq!(block_item_name(WATER), None);
    assert_eq!(item_block("Field supplies"), None);
}

#[test]
fn inventory_slot_commands_roundtrip() {
    use exa_protocol::*;
    for command in [
        InventoryCommand::Select { slot: 8 },
        InventoryCommand::Move {
            revision: 5,
            from: 0,
            to: 35,
            quantity: 32,
        },
        InventoryCommand::QuickMove {
            revision: 7,
            slot: 35,
        },
    ] {
        let message = ClientMessage::Inventory(command);
        assert_eq!(
            rmp_serde::from_slice::<ClientMessage>(&rmp_serde::to_vec(&message).unwrap()).unwrap(),
            message
        );
    }
    let mut inventory = PlayerInventory::default();
    inventory.slots[35] = Some(InventoryEntry {
        name: "Supplies".into(),
        quantity: 64,
    });
    inventory.selected = 8;
    inventory.revision = 17;
    assert_eq!(
        rmp_serde::from_slice::<PlayerInventory>(&rmp_serde::to_vec(&inventory).unwrap()).unwrap(),
        inventory
    );
}

#[test]
fn container_commands_and_snapshot_roundtrip() {
    use exa_protocol::*;
    for command in [
        ContainerCommand::Open {
            position: [26, 7, 30],
        },
        ContainerCommand::Close,
        ContainerCommand::Transfer {
            position: [26, 7, 30],
            revision: 9,
            inventory_revision: 4,
            slot: 2,
            target: None,
            quantity: 2,
            deposit: true,
        },
    ] {
        let message = ClientMessage::Container(command);
        let bytes = rmp_serde::to_vec(&message).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ClientMessage>(&bytes).unwrap(),
            message
        );
    }
    let snapshot = ContainerSnapshot {
        position: [26, 7, 30],
        revision: 10,
        items: vec![InventoryEntry {
            name: "Supplies".into(),
            quantity: 2,
        }],
    };
    assert_eq!(
        rmp_serde::from_slice::<ContainerSnapshot>(&rmp_serde::to_vec(&snapshot).unwrap()).unwrap(),
        snapshot
    );
}

#[test]
fn active_messages_keep_named_wire_tags() {
    // These variants follow removed 2D variants in the old enums. Their wire
    // tags must remain names, independent of their position in the Rust enum.
    let logout = b"\xa6Logout";
    assert_eq!(rmp_serde::to_vec(&ClientMessage::Logout).unwrap(), logout);
    assert_eq!(
        rmp_serde::from_slice::<ClientMessage>(logout).unwrap(),
        ClientMessage::Logout
    );
    let pong = b"\x81\xa4Pong\x91\x05";
    assert_eq!(
        rmp_serde::to_vec(&ServerMessage::Pong { seq: 5 }).unwrap(),
        pong
    );
    assert_eq!(
        rmp_serde::from_slice::<ServerMessage>(pong).unwrap(),
        ServerMessage::Pong { seq: 5 }
    );
}

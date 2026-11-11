use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    Input3d {
        seq: u32,
        input: crate::PlayerInput,
    },
    Chat3d {
        text: String,
    },
    Login {
        version: u32,
        player_name: String,
        player_uuid: [u8; 16],
    },
    Logout,
    Ping {
        seq: u32,
    },
    Container(crate::ContainerCommand),
    Inventory(crate::InventoryCommand),
    EditBlock(crate::BlockEdit),
    Equipment(crate::EquipmentCommand),
    Distillation(crate::DistillationCommand),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Display-only terrain, separate from the authoritative nearby voxel grid.
    DistantView {
        epoch: u64,
        center: [i32; 2],
    },
    DistantTile {
        epoch: u64,
        key: crate::DistantTileKey,
        revision: u64,
        blocks: Vec<u8>,
    },
    Distillation(crate::DistillationReply),
    PipeRouteReply(crate::PipeRouteReply),
    AssemblyReport(crate::AssemblyReport),
    UnloadColumn {
        column: [i32; 2],
    },
    Welcome3d {
        player_id: u32,
        seed: u64,
        rotation: [f32; 2],
    },
    Chunk3d {
        origin: [i32; 3],
        blocks: Vec<u8>,
    },
    World3d(crate::Snapshot3d),
    Error {
        code: u32,
        text: String,
    },
    Pong {
        seq: u32,
    },
    EquipmentState {
        revision: u64,
        devices: Vec<crate::EquipmentSnapshot>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn roundtrip<T>(v: T)
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let bytes = rmp_serde::to_vec(&v).expect("serialize");
        let back: T = rmp_serde::from_slice(&bytes).expect("deserialize");
        assert_eq!(v, back);
    }

    #[test]
    fn client_session_messages() {
        roundtrip(ClientMessage::Login {
            version: PROTOCOL_VERSION,
            player_name: "alice".to_string(),
            player_uuid: [1; 16],
        });
        roundtrip(ClientMessage::Chat3d {
            text: "hello".to_string(),
        });
        roundtrip(ClientMessage::Logout);
        roundtrip(ClientMessage::Ping { seq: 99 });
    }

    #[test]
    fn server_session_messages() {
        roundtrip(ServerMessage::Welcome3d {
            player_id: 1,
            seed: 0xDEAD_BEEF,
            rotation: [0.5, -0.1],
        });
        roundtrip(ServerMessage::Error {
            code: 1,
            text: "bad".to_string(),
        });
        roundtrip(ServerMessage::Pong { seq: 5 });
    }
}

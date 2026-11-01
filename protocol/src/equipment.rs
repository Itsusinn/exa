//! Stable equipment identities and facts. Process simulation is not part of the wire layer.
use serde::{Deserialize, Serialize};

pub const TANK_ITEM: &str = "储罐";
pub const PIPE_ITEM: &str = "管道";
pub const PUMP_ITEM: &str = "测试泵";
pub const VALVE_ITEM: &str = "阀门";
// Keep a complete visible equipment message below the transport's 1 MiB frame limit,
// including the worst-case material names and MessagePack overhead.
pub const MAX_EQUIPMENT: usize = 1024;
pub const MAX_MATERIAL_COMPONENTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EquipmentKind {
    Tank,
    Pipe,
    Pump,
    Valve,
}
impl EquipmentKind {
    pub fn item_name(self) -> &'static str {
        match self {
            Self::Tank => TANK_ITEM,
            Self::Pipe => PIPE_ITEM,
            Self::Pump => PUMP_ITEM,
            Self::Valve => VALVE_ITEM,
        }
    }
    pub fn from_item(name: &str) -> Option<Self> {
        match name {
            TANK_ITEM => Some(Self::Tank),
            PIPE_ITEM => Some(Self::Pipe),
            PUMP_ITEM => Some(Self::Pump),
            VALVE_ITEM => Some(Self::Valve),
            _ => None,
        }
    }
    pub fn size(self) -> [i32; 3] {
        match self {
            Self::Tank => [2, 3, 2],
            Self::Pipe | Self::Pump | Self::Valve => [1; 3],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAmount {
    /// Stable material identifier, independent of display language.
    pub material: String,
    pub moles: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentSnapshot {
    pub id: u64,
    pub revision: u64,
    pub kind: EquipmentKind,
    pub origin: [i32; 3],
    /// Pipe direction: 0 = X, 1 = Y, 2 = Z. Tanks use 0.
    pub axis: u8,
    pub contents: Vec<MaterialAmount>,
    pub internal_energy_j: f64,
    pub enabled: bool,
    pub opening: f64,
    pub pump_reversed: bool,
    /// Derived presentation only; never persisted as an authoritative inventory.
    pub reading: Option<EquipmentReading>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentReading {
    pub temperature_k: Option<f64>,
    pub pressure_pa: Option<f64>,
    pub volume_m3: f64,
    pub capacity_m3: f64,
    /// Tanks report net inflow; inline devices report positive flow from their
    /// negative-axis port to their positive-axis port.
    pub flow_m3_s: f64,
    pub status: String,
}
impl EquipmentReading {
    pub fn valid(&self) -> bool {
        self.temperature_k
            .is_none_or(|v| v.is_finite() && (0.0..=1e6).contains(&v))
            && self
                .pressure_pa
                .is_none_or(|v| v.is_finite() && (0.0..=1e12).contains(&v))
            && self.volume_m3.is_finite()
            && (0.0..=1e9).contains(&self.volume_m3)
            && self.capacity_m3.is_finite()
            && (0.0..=1e9).contains(&self.capacity_m3)
            && self.flow_m3_s.is_finite()
            && self.flow_m3_s.abs() <= 1e9
            && self.status.len() <= 96
    }
}
impl EquipmentSnapshot {
    pub fn valid(&self) -> bool {
        let size = self.kind.size();
        self.id != 0
            && self.id <= i64::MAX as u64
            && self.revision <= i64::MAX as u64
            && self.axis <= 2
            && (self.kind != EquipmentKind::Tank || self.axis == 0)
            && crate::valid_voxel(self.origin)
            && self.origin[1] > 0
            && (0..3).all(|i| {
                self.origin[i].checked_add(size[i] - 1).is_some_and(|last| {
                    let mut corner = self.origin;
                    corner[i] = last;
                    crate::valid_voxel(corner)
                })
            })
            && self.internal_energy_j.is_finite()
            && self.internal_energy_j.abs() <= 1e18
            && self.opening.is_finite()
            && (0.0..=1.0).contains(&self.opening)
            && self.reading.as_ref().is_none_or(EquipmentReading::valid)
            && self.contents.len() <= MAX_MATERIAL_COMPONENTS
            && self.contents.iter().enumerate().all(|(index, component)| {
                !component.material.is_empty()
                    && component.material.len() <= 64
                    && component
                        .material
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b":_./-".contains(&c))
                    && component.moles.is_finite()
                    && component.moles > 0.0
                    && component.moles <= 1e12
                    && !self.contents[..index]
                        .iter()
                        .any(|earlier| earlier.material == component.material)
            })
    }
    pub fn is_empty(&self) -> bool {
        self.contents.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EquipmentCommand {
    Place {
        target: [i32; 3],
        yaw: f32,
        pitch: f32,
        kind: EquipmentKind,
        axis: u8,
        inventory_revision: u64,
        slot: u8,
    },
    Remove {
        id: u64,
        revision: u64,
        yaw: f32,
        pitch: f32,
        inventory_revision: u64,
    },
    Configure {
        id: u64,
        revision: u64,
        yaw: f32,
        pitch: f32,
        enabled: bool,
        opening: f64,
        pump_reversed: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tank() -> EquipmentSnapshot {
        EquipmentSnapshot {
            id: 1,
            revision: 0,
            kind: EquipmentKind::Tank,
            origin: [-1, 1, -1],
            axis: 0,
            contents: vec![MaterialAmount {
                material: "icheme:water".into(),
                moles: 55.5,
            }],
            internal_energy_j: 1000.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            reading: None,
        }
    }
    #[test]
    fn equipment_wire_roundtrip_and_invalid_facts() {
        let state = crate::ServerMessage::EquipmentState {
            revision: 9,
            devices: vec![tank()],
        };
        assert_eq!(
            rmp_serde::from_slice::<crate::ServerMessage>(&rmp_serde::to_vec(&state).unwrap())
                .unwrap(),
            state
        );
        for command in [
            EquipmentCommand::Place {
                target: [-1, 0, -1],
                yaw: 0.0,
                pitch: -0.5,
                kind: EquipmentKind::Tank,
                axis: 0,
                inventory_revision: 7,
                slot: 1,
            },
            EquipmentCommand::Remove {
                id: 1,
                revision: 0,
                yaw: 0.0,
                pitch: 0.0,
                inventory_revision: 8,
            },
            EquipmentCommand::Configure {
                id: 1,
                revision: 2,
                yaw: 0.0,
                pitch: 0.0,
                enabled: true,
                opening: 0.5,
                pump_reversed: true,
            },
        ] {
            let message = crate::ClientMessage::Equipment(command);
            assert_eq!(
                rmp_serde::from_slice::<crate::ClientMessage>(
                    &rmp_serde::to_vec(&message).unwrap()
                )
                .unwrap(),
                message
            );
        }
        assert!(tank().valid());
        let mut invalid = tank();
        invalid.contents[0].moles = f64::NAN;
        assert!(!invalid.valid());
        invalid = tank();
        invalid.contents.push(invalid.contents[0].clone());
        assert!(!invalid.valid());
        invalid = tank();
        invalid.origin[0] = crate::WORLD_LIMIT - 1;
        assert!(!invalid.valid());
        invalid = tank();
        invalid.axis = 1;
        assert!(!invalid.valid());
    }

    #[test]
    fn maximum_equipment_message_fits_transport_frame() {
        let mut devices = Vec::with_capacity(MAX_EQUIPMENT);
        for i in 0..MAX_EQUIPMENT {
            let mut device = tank();
            device.id = i as u64 + 1;
            device.revision = i64::MAX as u64;
            device.contents = (0..MAX_MATERIAL_COMPONENTS)
                .map(|component| MaterialAmount {
                    material: format!("{component:064}"),
                    moles: 1e12,
                })
                .collect();
            device.internal_energy_j = 1e18;
            device.reading = Some(EquipmentReading {
                temperature_k: Some(1e6),
                pressure_pa: Some(1e12),
                volume_m3: 1e9,
                capacity_m3: 1e9,
                flow_m3_s: 1e9,
                status: "x".repeat(96),
            });
            assert!(device.valid());
            devices.push(device);
        }
        let bytes = rmp_serde::to_vec(&crate::ServerMessage::EquipmentState {
            revision: i64::MAX as u64,
            devices,
        })
        .unwrap();
        assert!(bytes.len() < 1_048_576, "{} bytes", bytes.len());
    }
}

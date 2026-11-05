//! Stable equipment identities and facts. Process simulation is not part of the wire layer.
use serde::{Deserialize, Serialize};

pub const COLUMN_THERMO_REVISION: &str = "butane-pentane-pr78-cp298-r1";

/// Construction facts only. Validity does not certify a converged process design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDesign {
    pub diameter: u8,
    pub spec: crate::DistillationSpec,
    pub thermo_revision: String,
}
impl Default for ColumnDesign {
    fn default() -> Self {
        Self {
            diameter: 2,
            spec: Default::default(),
            thermo_revision: COLUMN_THERMO_REVISION.into(),
        }
    }
}
impl ColumnDesign {
    pub fn valid(&self) -> bool {
        (2..=6).contains(&self.diameter)
            && self.spec.valid()
            && self.thermo_revision == COLUMN_THERMO_REVISION
    }
    pub fn size(&self) -> [i32; 3] {
        [
            i32::from(self.diameter),
            i32::from(self.spec.trays) + 2,
            i32::from(self.diameter),
        ]
    }
}

pub const COLUMN_ITEM: &str = "蒸馏塔";
pub const REFLUX_COLUMN_ITEM: &str = "回流蒸馏塔";
pub const CONDENSER_ITEM: &str = "冷凝器";
pub const REBOILER_ITEM: &str = "再沸器";
pub const TEE_ITEM: &str = "回流三通";
pub const TANK_ITEM: &str = "储罐";
pub const PIPE_ITEM: &str = "管道";
pub const PUMP_ITEM: &str = "测试泵";
pub const VALVE_ITEM: &str = "阀门";
pub const VESSEL_ITEM: &str = "密闭罐";
pub const HEAT_EXCHANGER_ITEM: &str = "双流换热器";
pub const PIPE_RACK_ITEM: &str = "管架";
pub const MAX_PIPE_ROUTE: usize = 32;
pub const PIPE_START_RANGE: f32 = 32.0;
// Keep a complete visible equipment message below the transport's 1 MiB frame limit,
// including the worst-case material names and MessagePack overhead.
pub const MAX_EQUIPMENT: usize = 1024;
pub const MAX_MATERIAL_COMPONENTS: usize = 8;

/// Immutable construction dimensions in metres (one voxel is one metre).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TankSize {
    pub diameter: u8,
    pub height: u8,
}
impl Default for TankSize {
    fn default() -> Self {
        Self {
            diameter: 2,
            height: 3,
        }
    }
}
impl TankSize {
    pub fn valid(self) -> bool {
        (2..=6).contains(&self.diameter) && (3..=12).contains(&self.height)
    }
    pub fn size(self) -> [i32; 3] {
        [
            i32::from(self.diameter),
            i32::from(self.height),
            i32::from(self.diameter),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EquipmentKind {
    Tank,
    Pipe,
    Pump,
    Valve,
    Vessel,
    HeatExchanger,
    PipeRack,
    Column,
    RefluxColumn,
    Condenser,
    Reboiler,
    Tee,
}
impl EquipmentKind {
    pub fn is_column(self) -> bool {
        matches!(self, Self::Column | Self::RefluxColumn)
    }
    /// Construction-only equipment pauses its connected process group.
    pub fn is_planning(self) -> bool {
        self.is_column() || matches!(self, Self::Condenser | Self::Reboiler | Self::Tee)
    }
    pub fn axis_count(self) -> u8 {
        match self {
            Self::Tee => 12,
            Self::Pipe | Self::Pump | Self::Valve | Self::PipeRack => 3,
            _ => 1,
        }
    }
    pub fn item_name(self) -> &'static str {
        match self {
            Self::RefluxColumn => REFLUX_COLUMN_ITEM,
            Self::Condenser => CONDENSER_ITEM,
            Self::Reboiler => REBOILER_ITEM,
            Self::Tee => TEE_ITEM,
            Self::Column => COLUMN_ITEM,
            Self::Tank => TANK_ITEM,
            Self::Pipe => PIPE_ITEM,
            Self::Pump => PUMP_ITEM,
            Self::Valve => VALVE_ITEM,
            Self::Vessel => VESSEL_ITEM,
            Self::HeatExchanger => HEAT_EXCHANGER_ITEM,
            Self::PipeRack => PIPE_RACK_ITEM,
        }
    }
    pub fn from_item(name: &str) -> Option<Self> {
        match name {
            REFLUX_COLUMN_ITEM => Some(Self::RefluxColumn),
            CONDENSER_ITEM => Some(Self::Condenser),
            REBOILER_ITEM => Some(Self::Reboiler),
            TEE_ITEM => Some(Self::Tee),
            COLUMN_ITEM => Some(Self::Column),
            TANK_ITEM => Some(Self::Tank),
            PIPE_ITEM => Some(Self::Pipe),
            PUMP_ITEM => Some(Self::Pump),
            VALVE_ITEM => Some(Self::Valve),
            VESSEL_ITEM => Some(Self::Vessel),
            HEAT_EXCHANGER_ITEM => Some(Self::HeatExchanger),
            PIPE_RACK_ITEM => Some(Self::PipeRack),
            _ => None,
        }
    }
    pub fn size(self) -> [i32; 3] {
        match self {
            Self::Column | Self::RefluxColumn => [2, 8, 2],
            Self::Condenser | Self::Reboiler => [3, 2, 3],
            Self::Tank | Self::Vessel => [2, 3, 2],
            Self::HeatExchanger => [2, 1, 3],
            Self::Pipe | Self::Pump | Self::Valve | Self::PipeRack | Self::Tee => [1; 3],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAmount {
    /// Stable material identifier, independent of display language.
    pub material: String,
    pub moles: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeatControlMode {
    Off,
    Manual,
    OutletTemperature,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThermalSettings {
    pub mode: HeatControlMode,
    pub duty_fraction: f64,
    /// Temperature target on exchanger circuit A, in kelvin.
    pub target_temperature_k: f64,
}
impl Default for ThermalSettings {
    fn default() -> Self {
        Self {
            mode: HeatControlMode::Off,
            duty_fraction: 1.0,
            target_temperature_k: 330.0,
        }
    }
}
impl ThermalSettings {
    pub fn valid(&self) -> bool {
        self.duty_fraction.is_finite()
            && (0.0..=1.0).contains(&self.duty_fraction)
            && self.target_temperature_k.is_finite()
            && (250.0..=600.0).contains(&self.target_temperature_k)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingQuality {
    Available,
    NoFlow,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlState {
    Off,
    Manual,
    Regulating,
    Limited,
}

/// Derived observations. Component order is n-butane, n-pentane. Never persisted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThermalReading {
    pub vapor_fraction: Option<f64>,
    pub liquid_moles: [f64; 2],
    pub vapor_moles: [f64; 2],
    /// Positive heat enters circuit A; circuit B receives its exact negative.
    pub heat_w: f64,
    pub inlet_temperature_k: [Option<f64>; 2],
    pub outlet_temperature_k: [Option<f64>; 2],
    pub flow_m3_s: [f64; 2],
    pub quality: ReadingQuality,
    pub control_state: ControlState,
    pub sampled_at_tick: u64,
}
impl ThermalReading {
    pub fn valid(&self) -> bool {
        self.vapor_fraction
            .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            && self
                .liquid_moles
                .iter()
                .chain(&self.vapor_moles)
                .all(|v| v.is_finite() && (0.0..=1e12).contains(v))
            && self.heat_w.is_finite()
            && self.heat_w.abs() <= 1e15
            && self
                .inlet_temperature_k
                .iter()
                .chain(&self.outlet_temperature_k)
                .all(|v| v.is_none_or(|t| t.is_finite() && (0.0..=1e6).contains(&t)))
            && self
                .flow_m3_s
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1e9)
            && self.sampled_at_tick <= i64::MAX as u64
    }
}
impl Default for ThermalReading {
    fn default() -> Self {
        Self {
            vapor_fraction: None,
            liquid_moles: [0.0; 2],
            vapor_moles: [0.0; 2],
            heat_w: 0.0,
            inlet_temperature_k: [None; 2],
            outlet_temperature_k: [None; 2],
            flow_m3_s: [0.0; 2],
            quality: ReadingQuality::Unavailable,
            control_state: ControlState::Off,
            sampled_at_tick: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentSnapshot {
    pub id: u64,
    pub revision: u64,
    pub kind: EquipmentKind,
    pub origin: [i32; 3],
    /// Pipe direction: 0 = X, 1 = Y, 2 = Z. Tanks use 0.
    pub axis: u8,
    /// Explicit sorted faces: -X,+X,-Y,+Y,-Z,+Z. None keeps the legacy axis pair.
    /// Only ordinary pipes may carry these two ends; no branches are implied.
    pub pipe_ends: Option<[u8; 2]>,
    /// None retains the original 2 x 3 x 2 geometry. Only tanks and vessels use this.
    pub tank_size: Option<TankSize>,
    pub column_design: Option<ColumnDesign>,
    pub contents: Vec<MaterialAmount>,
    pub internal_energy_j: f64,
    pub enabled: bool,
    pub opening: f64,
    pub pump_reversed: bool,
    pub thermal: Option<ThermalSettings>,
    /// Derived presentation only; never persisted as an authoritative inventory.
    pub reading: Option<EquipmentReading>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentReading {
    pub temperature_k: Option<f64>,
    pub pressure_pa: Option<f64>,
    pub volume_m3: f64,
    pub capacity_m3: f64,
    /// Tanks report net inflow; inline devices report positive flow from port 0
    /// to port 1 (legacy straight devices: negative axis to positive axis).
    pub flow_m3_s: f64,
    pub status: String,
    pub thermal: Option<ThermalReading>,
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
            && self.thermal.as_ref().is_none_or(ThermalReading::valid)
    }
}
impl EquipmentSnapshot {
    pub fn tank_size_or_default(&self) -> TankSize {
        self.tank_size.unwrap_or_default()
    }
    pub fn size(&self) -> [i32; 3] {
        if matches!(self.kind, EquipmentKind::Tank | EquipmentKind::Vessel) {
            self.tank_size_or_default().size()
        } else if self.kind.is_column() {
            self.column_design
                .as_ref()
                .map_or([2, 8, 2], ColumnDesign::size)
        } else {
            self.kind.size()
        }
    }
    pub fn valid(&self) -> bool {
        let size = self.size();
        self.id != 0
            && self.id <= i64::MAX as u64
            && self.revision <= i64::MAX as u64
            && self.axis < self.kind.axis_count()
            && self.tank_size.is_none_or(|size| {
                matches!(self.kind, EquipmentKind::Tank | EquipmentKind::Vessel) && size.valid()
            })
            && match (&self.column_design, self.kind) {
                (Some(design), kind) if kind.is_column() => {
                    design.valid()
                        && self.axis == 0
                        && self.contents.is_empty()
                        && self.internal_energy_j == 0.0
                }
                (None, kind) => !kind.is_column(),
                _ => false,
            }
            && (self.kind != EquipmentKind::PipeRack || self.axis != 1)
            && (!self.kind.is_planning()
                || (self.contents.is_empty() && self.internal_energy_j == 0.0))
            && match (self.kind, self.pipe_ends) {
                (EquipmentKind::Pipe, Some([a, b])) => a < b && b < 6,
                (_, None) => true,
                _ => false,
            }
            && (!matches!(
                self.kind,
                EquipmentKind::Tank | EquipmentKind::Vessel | EquipmentKind::HeatExchanger
            ) || self.axis == 0)
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
            && match (&self.thermal, self.kind) {
                (Some(settings), EquipmentKind::HeatExchanger) => settings.valid(),
                (None, kind) => kind != EquipmentKind::HeatExchanger,
                _ => false,
            }
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
    CheckAssembly {
        request_id: u64,
        id: u64,
        revision: u64,
        yaw: f32,
        pitch: f32,
    },
    BeginPipe {
        request_id: u64,
        yaw: f32,
        pitch: f32,
    },
    LayPipe {
        request_id: u64,
        start: PipeAnchor,
        end: PipeAnchor,
        cells: Vec<[i32; 3]>,
        yaw: f32,
        pitch: f32,
        inventory_revision: u64,
    },
    CancelPipe {
        request_id: u64,
    },
    ConfigureThermal {
        id: u64,
        revision: u64,
        yaw: f32,
        pitch: f32,
        settings: ThermalSettings,
    },
    Place {
        target: [i32; 3],
        yaw: f32,
        pitch: f32,
        kind: EquipmentKind,
        axis: u8,
        tank_size: Option<TankSize>,
        column_design: Option<ColumnDesign>,
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

/// Result of checking a device's physical construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssemblyStatus {
    Complete,
    Incomplete,
    Stale,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssemblyLine {
    /// Logical inspection line, not necessarily a port on the master device.
    pub port: u8,
    pub tank_id: Option<u64>,
    pub pump_id: Option<u64>,
    pub valve_id: Option<u64>,
    pub valid: bool,
    pub message: String,
}

/// An on-demand structural observation, not a persisted machine or a process solve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssemblyReport {
    pub request_id: u64,
    pub master_id: u64,
    pub master_kind: EquipmentKind,
    pub status: AssemblyStatus,
    pub lines: Vec<AssemblyLine>,
    pub message: String,
}
impl AssemblyReport {
    pub fn valid(&self) -> bool {
        let valid_id = |id| id > 0 && id <= i64::MAX as u64;
        let line_count = match self.master_kind {
            EquipmentKind::HeatExchanger => 4,
            EquipmentKind::Column => 3,
            EquipmentKind::RefluxColumn => 9,
            _ => return false,
        };
        valid_id(self.request_id)
            && valid_id(self.master_id)
            && self.message.len() <= 256
            && self.lines.len() <= line_count
            && self.lines.iter().enumerate().all(|(index, line)| {
                usize::from(line.port) < line_count
                    && !self.lines[..index]
                        .iter()
                        .any(|earlier| earlier.port == line.port)
                    && line.tank_id.is_none_or(valid_id)
                    && line.pump_id.is_none_or(valid_id)
                    && line.valve_id.is_none_or(valid_id)
                    && line.message.len() <= 256
            })
            && match self.status {
                AssemblyStatus::Complete => {
                    self.lines.len() == line_count && self.lines.iter().all(|line| line.valid)
                }
                AssemblyStatus::Incomplete => {
                    self.lines.len() == line_count && self.lines.iter().any(|line| !line.valid)
                }
                AssemblyStatus::Stale | AssemblyStatus::Rejected => true,
            }
    }
}

/// A validated interface of an existing device. New route cells exclude anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeAnchor {
    pub id: u64,
    pub revision: u64,
    pub port: u8,
}
impl PipeAnchor {
    pub fn valid(self) -> bool {
        self.id > 0
            && self.id <= i64::MAX as u64
            && self.revision <= i64::MAX as u64
            && self.port < 5
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipeRouteStage {
    Started,
    Laid,
    Cancelled,
    Rejected,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipeRouteReply {
    pub request_id: u64,
    pub stage: PipeRouteStage,
    pub anchor: Option<PipeAnchor>,
    pub message: String,
}
impl PipeRouteReply {
    pub fn valid(&self) -> bool {
        self.request_id > 0
            && self.request_id <= i64::MAX as u64
            && self.anchor.is_none_or(PipeAnchor::valid)
            && self.message.len() <= 256
            && match self.stage {
                PipeRouteStage::Started => self.anchor.is_some(),
                PipeRouteStage::Laid | PipeRouteStage::Cancelled => self.anchor.is_none(),
                PipeRouteStage::Rejected => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflux_components_are_bounded_empty_construction_facts() {
        for kind in [
            EquipmentKind::RefluxColumn,
            EquipmentKind::Condenser,
            EquipmentKind::Reboiler,
            EquipmentKind::Tee,
        ] {
            let mut d = tank();
            d.kind = kind;
            d.contents.clear();
            d.internal_energy_j = 0.0;
            d.column_design = kind.is_column().then(ColumnDesign::default);
            assert_eq!(EquipmentKind::from_item(kind.item_name()), Some(kind));
            for axis in 0..kind.axis_count() {
                d.axis = axis;
                assert!(d.valid(), "{kind:?} {axis}");
                assert_eq!(
                    rmp_serde::from_slice::<EquipmentSnapshot>(&rmp_serde::to_vec(&d).unwrap())
                        .unwrap(),
                    d
                );
            }
            d.axis = kind.axis_count();
            assert!(!d.valid());
            d.axis = 0;
            d.contents = tank().contents;
            assert!(!d.valid());
            d.contents.clear();
            d.internal_energy_j = 1.0;
            assert!(!d.valid());
            d.internal_energy_j = 0.0;
            d.thermal = Some(Default::default());
            assert!(!d.valid());
            d.thermal = None;
            d.origin[1] = crate::WORLD_HEIGHT - d.size()[1];
            assert!(d.valid());
            d.origin[1] += 1;
            assert!(!d.valid());
        }
        let anchor = PipeAnchor {
            id: 1,
            revision: 0,
            port: 4,
        };
        assert!(anchor.valid());
        assert!(!PipeAnchor { port: 5, ..anchor }.valid());
        let mut report = AssemblyReport {
            request_id: 1,
            master_id: 1,
            master_kind: EquipmentKind::RefluxColumn,
            status: AssemblyStatus::Complete,
            lines: (0..9)
                .map(|port| AssemblyLine {
                    port,
                    tank_id: None,
                    pump_id: None,
                    valve_id: None,
                    valid: true,
                    message: String::new(),
                })
                .collect(),
            message: String::new(),
        };
        assert!(report.valid());
        report.lines.pop();
        assert!(!report.valid());
    }

    #[test]
    fn column_construction_facts_are_bounded_and_roundtrip_without_process_results() {
        let mut d = tank();
        d.kind = EquipmentKind::Column;
        d.contents.clear();
        d.internal_energy_j = 0.0;
        assert!(!d.valid());
        let mut design = ColumnDesign::default();
        for diameter in 2..=6 {
            for trays in [2, 12] {
                for feed in [1, trays] {
                    design.diameter = diameter;
                    design.spec.trays = trays;
                    design.spec.feed_tray = feed;
                    d.column_design = Some(design.clone());
                    assert!(d.valid());
                    assert_eq!(
                        d.size(),
                        [
                            i32::from(diameter),
                            i32::from(trays) + 2,
                            i32::from(diameter)
                        ]
                    );
                    let bytes = rmp_serde::to_vec(&d).unwrap();
                    assert_eq!(
                        rmp_serde::from_slice::<EquipmentSnapshot>(&bytes).unwrap(),
                        d
                    );
                }
            }
        }
        d.internal_energy_j = 1.0;
        assert!(!d.valid());
        d.internal_energy_j = 0.0;
        d.contents = tank().contents;
        assert!(!d.valid());
        d.contents.clear();
        d.kind = EquipmentKind::Vessel;
        assert!(!d.valid());
        d.kind = EquipmentKind::Column;
        d.column_design.as_mut().unwrap().thermo_revision = "unknown".into();
        assert!(!d.valid());
        d.column_design.as_mut().unwrap().spec.trays = u8::MAX;
        assert_eq!(d.size()[1], 257);
        assert!(!d.valid());
        design.spec.trays = 6;
        design.spec.feed_tray = 7;
        assert!(!design.valid());
        design.spec.feed_tray = 3;
        design.spec.feed_mol_s = f64::NAN;
        assert!(!design.valid());
    }

    fn tank() -> EquipmentSnapshot {
        EquipmentSnapshot {
            id: 1,
            revision: 0,
            kind: EquipmentKind::Tank,
            origin: [-1, 1, -1],
            axis: 0,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            contents: vec![MaterialAmount {
                material: "icheme:water".into(),
                moles: 55.5,
            }],
            internal_energy_j: 1000.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: None,
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
                tank_size: None,
                column_design: None,
                inventory_revision: 7,
                slot: 1,
            },
            EquipmentCommand::CheckAssembly {
                request_id: 1,
                id: 2,
                revision: 3,
                yaw: 0.0,
                pitch: 0.0,
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
            EquipmentCommand::ConfigureThermal {
                id: 2,
                revision: 3,
                yaw: 0.0,
                pitch: 0.0,
                settings: ThermalSettings {
                    mode: HeatControlMode::OutletTemperature,
                    ..Default::default()
                },
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
    fn tank_dimensions_are_bounded_and_preserve_legacy_default() {
        let mut device = tank();
        assert_eq!(device.size(), [2, 3, 2]);
        device.tank_size = Some(TankSize::default());
        assert_eq!(device.size(), [2, 3, 2]);
        for kind in [EquipmentKind::Tank, EquipmentKind::Vessel] {
            device.kind = kind;
            for diameter in 2..=6 {
                for height in 3..=12 {
                    device.tank_size = Some(TankSize { diameter, height });
                    assert!(device.valid());
                    assert_eq!(
                        rmp_serde::from_slice::<EquipmentSnapshot>(
                            &rmp_serde::to_vec(&device).unwrap()
                        )
                        .unwrap(),
                        device
                    );
                }
            }
        }
        for (diameter, height) in [(0, 3), (1, 3), (7, 3), (2, 2), (2, 13), (255, 255)] {
            device.tank_size = Some(TankSize { diameter, height });
            assert!(!device.valid());
        }
        device.tank_size = Some(TankSize {
            diameter: 6,
            height: 12,
        });
        device.origin[0] = crate::WORLD_LIMIT - 5;
        assert!(!device.valid());
        device.origin = [0, 1, 0];
        device.kind = EquipmentKind::Pipe;
        assert!(!device.valid());
    }

    #[test]
    fn assembly_reports_validate_bounded_complete_and_incomplete_lines() {
        let mut report = AssemblyReport {
            request_id: 1,
            master_id: 2,
            master_kind: EquipmentKind::HeatExchanger,
            status: AssemblyStatus::Complete,
            lines: (0..4)
                .map(|port| AssemblyLine {
                    port,
                    tank_id: Some(u64::from(port) + 3),
                    pump_id: None,
                    valve_id: Some(8),
                    valid: true,
                    message: "connected".into(),
                })
                .collect(),
            message: "complete".into(),
        };
        assert!(report.valid());
        let message = crate::ServerMessage::AssemblyReport(report.clone());
        assert_eq!(
            rmp_serde::from_slice::<crate::ServerMessage>(&rmp_serde::to_vec(&message).unwrap())
                .unwrap(),
            message
        );
        report.lines[0].valid = false;
        assert!(!report.valid());
        report.status = AssemblyStatus::Incomplete;
        assert!(report.valid());
        report.lines[1].port = 0;
        assert!(!report.valid());
        report.lines[1].port = 1;
        report.lines[0].tank_id = Some(0);
        assert!(!report.valid());
        report.lines[0].tank_id = None;
        report.lines[0].message = "x".repeat(257);
        assert!(!report.valid());
        report.lines.clear();
        assert!(!report.valid());
        report.status = AssemblyStatus::Stale;
        assert!(report.valid());
        report.status = AssemblyStatus::Rejected;
        assert!(report.valid());
        report.master_id = 0;
        assert!(!report.valid());
    }

    #[test]
    fn pipe_faces_and_route_replies_are_bounded_and_roundtrip() {
        let mut pipe = tank();
        pipe.kind = EquipmentKind::Pipe;
        pipe.contents.clear();
        for a in 0..6 {
            for b in a + 1..6 {
                pipe.pipe_ends = Some([a, b]);
                assert!(pipe.valid());
            }
        }
        for ends in [[0, 0], [2, 1], [0, 6]] {
            pipe.pipe_ends = Some(ends);
            assert!(!pipe.valid());
        }
        pipe.pipe_ends = Some([0, 1]);
        pipe.kind = EquipmentKind::Pump;
        assert!(!pipe.valid());
        pipe.pipe_ends = None;
        pipe.kind = EquipmentKind::PipeRack;
        pipe.axis = 1;
        assert!(!pipe.valid());
        pipe.axis = 2;
        assert!(pipe.valid());
        let anchor = PipeAnchor {
            id: 1,
            revision: 2,
            port: 0,
        };
        let reply = PipeRouteReply {
            request_id: 1,
            stage: PipeRouteStage::Started,
            anchor: Some(anchor),
            message: "起点已确认".into(),
        };
        assert!(reply.valid());
        let message = crate::ServerMessage::PipeRouteReply(reply.clone());
        assert_eq!(
            rmp_serde::from_slice::<crate::ServerMessage>(&rmp_serde::to_vec(&message).unwrap())
                .unwrap(),
            message
        );
        assert!(!PipeRouteReply {
            anchor: None,
            ..reply.clone()
        }
        .valid());
        assert!(!PipeRouteReply {
            request_id: 0,
            ..reply.clone()
        }
        .valid());
        assert!(!PipeRouteReply {
            message: "x".repeat(257),
            ..reply
        }
        .valid());
        for command in [
            EquipmentCommand::BeginPipe {
                request_id: 1,
                yaw: 0.0,
                pitch: 0.0,
            },
            EquipmentCommand::LayPipe {
                request_id: 2,
                start: anchor,
                end: PipeAnchor { id: 2, ..anchor },
                cells: (0..MAX_PIPE_ROUTE).map(|x| [x as i32, 2, 0]).collect(),
                yaw: 0.0,
                pitch: 0.0,
                inventory_revision: 7,
            },
            EquipmentCommand::CancelPipe { request_id: 3 },
        ] {
            assert_eq!(
                rmp_serde::from_slice::<EquipmentCommand>(&rmp_serde::to_vec(&command).unwrap())
                    .unwrap(),
                command
            );
        }
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
            device.kind = EquipmentKind::HeatExchanger;
            device.thermal = Some(ThermalSettings {
                mode: HeatControlMode::OutletTemperature,
                ..Default::default()
            });
            device.reading = Some(EquipmentReading {
                temperature_k: Some(1e6),
                pressure_pa: Some(1e12),
                volume_m3: 1e9,
                capacity_m3: 1e9,
                flow_m3_s: 1e9,
                status: "x".repeat(96),
                thermal: Some(ThermalReading {
                    vapor_fraction: Some(0.5),
                    liquid_moles: [1e12; 2],
                    vapor_moles: [1e12; 2],
                    heat_w: 1e15,
                    inlet_temperature_k: [Some(1e6); 2],
                    outlet_temperature_k: [Some(1e6); 2],
                    flow_m3_s: [1e9; 2],
                    quality: ReadingQuality::Unavailable,
                    control_state: ControlState::Regulating,
                    sampled_at_tick: i64::MAX as u64,
                }),
            });
            assert!(device.valid());
            devices.push(device);
        }
        let bytes = rmp_serde::to_vec(&crate::ServerMessage::EquipmentState {
            revision: i64::MAX as u64,
            devices: devices.clone(),
        })
        .unwrap();
        assert!(bytes.len() < 1_048_576, "{} bytes", bytes.len());
        for device in &mut devices {
            device.kind = EquipmentKind::Column;
            device.thermal = None;
            device.contents.clear();
            device.internal_energy_j = 0.0;
            device.column_design = Some(ColumnDesign::default());
            assert!(device.valid());
        }
        let bytes = rmp_serde::to_vec(&crate::ServerMessage::EquipmentState {
            revision: i64::MAX as u64,
            devices,
        })
        .unwrap();
        assert!(bytes.len() < 1_048_576, "{} column bytes", bytes.len());
    }
}

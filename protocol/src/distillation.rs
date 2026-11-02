//! Bounded inputs and observations for an isolated binary column calculation.
//! These are transient design requests, never world inventories or saved equipment.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistillationSpec {
    /// Equilibrium trays excluding the total condenser and equilibrium reboiler.
    pub trays: u8,
    pub feed_tray: u8,
    pub feed_mol_s: f64,
    pub butane_fraction: f64,
    pub feed_temperature_k: f64,
    pub pressure_pa: f64,
    pub reflux_ratio: f64,
    pub distillate_fraction: f64,
}
impl Default for DistillationSpec {
    fn default() -> Self {
        Self {
            trays: 6,
            feed_tray: 3,
            feed_mol_s: 1.0,
            butane_fraction: 0.5,
            feed_temperature_k: 330.0,
            pressure_pa: 400_000.0,
            reflux_ratio: 2.0,
            distillate_fraction: 0.5,
        }
    }
}
impl DistillationSpec {
    pub fn valid(&self) -> bool {
        (2..=12).contains(&self.trays)
            && (1..=self.trays).contains(&self.feed_tray)
            && bounded(self.feed_mol_s, 0.001, 1000.0)
            && bounded(self.butane_fraction, 0.01, 0.99)
            && bounded(self.feed_temperature_k, 293.15, 400.0)
            && bounded(self.pressure_pa, 100_000.0, 2_000_000.0)
            && bounded(self.reflux_ratio, 0.05, 20.0)
            && bounded(self.distillate_fraction, 0.01, 0.99)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistillationStage {
    /// 1..=trays are trays; trays+1 is the equilibrium reboiler.
    pub number: u8,
    pub temperature_k: f64,
    pub liquid_butane_fraction: f64,
    pub vapor_butane_fraction: f64,
    pub liquid_mol_s: f64,
    pub vapor_mol_s: f64,
}
impl DistillationStage {
    fn valid(&self) -> bool {
        bounded(self.temperature_k, 293.15, 400.0)
            && bounded(self.liquid_butane_fraction, 0.0, 1.0)
            && bounded(self.vapor_butane_fraction, 0.0, 1.0)
            && bounded(self.liquid_mol_s, 0.0, 1e9)
            && bounded(self.vapor_mol_s, 0.0, 1e9)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistillationResult {
    pub spec: DistillationSpec,
    pub stages: Vec<DistillationStage>,
    pub distillate_mol_s: f64,
    pub bottoms_mol_s: f64,
    pub distillate_butane_fraction: f64,
    pub bottoms_butane_fraction: f64,
    pub condenser_temperature_k: f64,
    /// Positive means heat removed from the condenser.
    pub condenser_duty_w: f64,
    /// Positive means heat added to the reboiler.
    pub reboiler_duty_w: f64,
    pub max_component_residual_mol_s: f64,
    pub max_energy_residual_w: f64,
    pub max_equilibrium_residual: f64,
    pub iterations: u32,
}
impl DistillationResult {
    pub fn valid(&self) -> bool {
        self.spec.valid()
            && self.stages.len() == usize::from(self.spec.trays) + 1
            && self
                .stages
                .iter()
                .enumerate()
                .all(|(i, s)| s.number as usize == i + 1 && s.valid())
            && bounded(self.distillate_mol_s, 0.0, 1000.0)
            && bounded(self.bottoms_mol_s, 0.0, 1000.0)
            && bounded(self.distillate_butane_fraction, 0.0, 1.0)
            && bounded(self.bottoms_butane_fraction, 0.0, 1.0)
            && bounded(self.condenser_temperature_k, 293.15, 400.0)
            && bounded(self.condenser_duty_w, -1e15, 1e15)
            && bounded(self.reboiler_duty_w, -1e15, 1e15)
            && bounded(self.max_component_residual_mol_s, 0.0, 1e9)
            && bounded(self.max_energy_residual_w, 0.0, 1e15)
            && bounded(self.max_equilibrium_residual, 0.0, 1e9)
            && self.iterations <= 10_000
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DistillationCommand {
    Calculate {
        request_id: u64,
        spec: DistillationSpec,
    },
    /// Cancels the named request, not a newer request in the same session.
    Cancel { request_id: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistillationStatus {
    Running,
    Completed,
    Cancelled,
    Rejected,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistillationReply {
    pub request_id: u64,
    pub status: DistillationStatus,
    pub message: String,
    pub result: Option<DistillationResult>,
}
impl DistillationReply {
    pub fn valid(&self) -> bool {
        self.request_id > 0
            && self.request_id <= i64::MAX as u64
            && self.message.len() <= 512
            && match (&self.status, &self.result) {
                (DistillationStatus::Completed, Some(result)) => result.valid(),
                (DistillationStatus::Completed, None) | (_, Some(_)) => false,
                (_, None) => true,
            }
    }
}
fn bounded(value: f64, low: f64, high: f64) -> bool {
    value.is_finite() && (low..=high).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specifications_are_bounded_and_nonfinite_values_are_rejected() {
        assert!(DistillationSpec::default().valid());
        for spec in [
            DistillationSpec {
                trays: 13,
                ..Default::default()
            },
            DistillationSpec {
                feed_tray: 0,
                ..Default::default()
            },
            DistillationSpec {
                feed_tray: 7,
                ..Default::default()
            },
            DistillationSpec {
                feed_mol_s: f64::INFINITY,
                ..Default::default()
            },
            DistillationSpec {
                butane_fraction: f64::NAN,
                ..Default::default()
            },
            DistillationSpec {
                pressure_pa: 1.0,
                ..Default::default()
            },
            DistillationSpec {
                reflux_ratio: 0.0,
                ..Default::default()
            },
        ] {
            assert!(!spec.valid());
        }
    }
    #[test]
    fn request_reply_roundtrip_and_result_shape_are_bounded() {
        let command = crate::ClientMessage::Distillation(DistillationCommand::Calculate {
            request_id: 9,
            spec: DistillationSpec::default(),
        });
        let encoded = rmp_serde::to_vec(&command).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<crate::ClientMessage>(&encoded).unwrap(),
            command
        );
        let spec = DistillationSpec {
            trays: 12,
            ..Default::default()
        };
        let result = DistillationResult {
            stages: (1..=13)
                .map(|number| DistillationStage {
                    number,
                    temperature_k: 330.0,
                    liquid_butane_fraction: 0.5,
                    vapor_butane_fraction: 0.6,
                    liquid_mol_s: 1e9,
                    vapor_mol_s: 1e9,
                })
                .collect(),
            spec,
            distillate_mol_s: 0.5,
            bottoms_mol_s: 0.5,
            distillate_butane_fraction: 0.8,
            bottoms_butane_fraction: 0.2,
            condenser_temperature_k: 310.0,
            condenser_duty_w: 1e15,
            reboiler_duty_w: 1e15,
            max_component_residual_mol_s: 0.0,
            max_energy_residual_w: 0.0,
            max_equilibrium_residual: 0.0,
            iterations: 1000,
        };
        let mut reply = DistillationReply {
            request_id: 9,
            status: DistillationStatus::Completed,
            message: "x".repeat(512),
            result: Some(result),
        };
        assert!(reply.valid());
        let message = crate::ServerMessage::Distillation(reply.clone());
        let bytes = rmp_serde::to_vec(&message).unwrap();
        assert!(bytes.len() < 16_384);
        assert_eq!(
            rmp_serde::from_slice::<crate::ServerMessage>(&bytes).unwrap(),
            message
        );
        reply.result.as_mut().unwrap().stages.pop();
        assert!(!reply.valid());
        reply.result = None;
        assert!(!reply.valid());
        reply.status = DistillationStatus::Cancelled;
        assert!(reply.valid());
        reply.request_id = 0;
        assert!(!reply.valid());
    }
}

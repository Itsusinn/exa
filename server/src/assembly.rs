//! Bounded, request-only inspection of heat units and column external connections.
//! This is a construction report, never a condition for process execution.
use game_simulation::equipment::{connected, ports, EquipmentPort};
use protocol::{
    AssemblyLine, AssemblyReport, AssemblyStatus, EquipmentKind, EquipmentSnapshot, TankSize,
    MAX_EQUIPMENT,
};
use std::collections::{BTreeMap, BTreeSet};
mod reflux;
#[cfg(test)]
pub(crate) use reflux::fixture::fixture as reflux_fixture;

/// Only built structure affects report validity; running material/control readings do not.
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    id: u64,
    kind: EquipmentKind,
    origin: [i32; 3],
    axis: u8,
    ends: Option<[u8; 2]>,
    size: Option<TankSize>,
    reversed: bool,
    design: Option<protocol::ColumnDesign>,
}
pub fn structure(devices: &BTreeMap<u64, EquipmentSnapshot>) -> Vec<Structure> {
    devices
        .values()
        .filter(|d| d.kind != EquipmentKind::PipeRack)
        .map(|d| Structure {
            id: d.id,
            kind: d.kind,
            origin: d.origin,
            axis: d.axis,
            ends: d.pipe_ends,
            size: d.tank_size,
            design: d.column_design.clone(),
            reversed: d.kind == EquipmentKind::Pump && d.pump_reversed,
        })
        .collect()
}

fn key(port: EquipmentPort) -> [i32; 3] {
    port.position.map(|v| (v * 2.0).round() as i32)
}

pub fn inspect(
    devices: &BTreeMap<u64, EquipmentSnapshot>,
    master_id: u64,
    request_id: u64,
) -> AssemblyReport {
    if devices
        .get(&master_id)
        .is_some_and(|d| d.kind == EquipmentKind::RefluxColumn)
    {
        return reflux::inspect(devices, master_id, request_id);
    }
    let mut report = AssemblyReport {
        request_id,
        master_id,
        master_kind: devices
            .get(&master_id)
            .filter(|d| matches!(d.kind, EquipmentKind::HeatExchanger | EquipmentKind::Column))
            .map_or(EquipmentKind::HeatExchanger, |d| d.kind),
        status: AssemblyStatus::Rejected,
        lines: vec![],
        message: "请选择双流换热器或蒸馏塔。".into(),
    };
    let Some(master) = devices
        .get(&master_id)
        .filter(|d| matches!(d.kind, EquipmentKind::HeatExchanger | EquipmentKind::Column))
    else {
        return report;
    };
    let column = master.kind == EquipmentKind::Column;
    if devices.len() > MAX_EQUIPMENT {
        report.message = "设备数量超过检查上限。".into();
        return report;
    }
    let mut index: BTreeMap<[i32; 3], Vec<(u64, EquipmentPort)>> = BTreeMap::new();
    for device in devices.values() {
        for port in ports(device) {
            index.entry(key(port)).or_default().push((device.id, port));
        }
    }
    let mut used = BTreeSet::new();
    for start in ports(master) {
        let mut line = AssemblyLine {
            port: start.id,
            tank_id: None,
            pump_id: None,
            valve_id: None,
            valid: false,
            message: String::new(),
        };
        let mut current = (master_id, start);
        let mut visited = BTreeSet::from([master_id]);
        let inlet = if column {
            start.id == 0
        } else {
            start.id % 2 == 0
        };
        let mut faults = Vec::new();
        for _ in 0..MAX_EQUIPMENT {
            let next: Vec<_> = index
                .get(&key(current.1))
                .into_iter()
                .flatten()
                .filter(|(id, port)| *id != current.0 && connected(current.1, *port))
                .copied()
                .collect();
            if next.len() != 1 {
                faults.push(if next.is_empty() {
                    "断管或接口未连接"
                } else {
                    "同口存在歧义连接"
                });
                break;
            }
            let (id, entry) = next[0];
            if !visited.insert(id) {
                faults.push("管路形成回环或回接主设备");
                break;
            }
            if !used.insert(id) {
                faults.push("流道共用部件或储罐");
            }
            let device = &devices[&id];
            match device.kind {
                EquipmentKind::Tank | EquipmentKind::Vessel => {
                    if column && device.kind != EquipmentKind::Vessel {
                        faults.push("蒸馏塔外部连接需要密闭罐");
                    }
                    line.tank_id = Some(id);
                    break;
                }
                EquipmentKind::Pipe => (),
                EquipmentKind::Pump => {
                    if line.pump_id.replace(id).is_some() {
                        faults.push("一条线路存在多台泵");
                    }
                    let expected = if inlet {
                        u8::from(!device.pump_reversed)
                    } else {
                        u8::from(device.pump_reversed)
                    };
                    if entry.id != expected {
                        faults.push("泵方向错误");
                    }
                }
                EquipmentKind::Valve => {
                    if line.valve_id.replace(id).is_some() {
                        faults.push("一条线路存在多个阀门");
                    }
                }
                _ => {
                    faults.push("线路包含不支持的设备");
                    break;
                }
            }
            let exits: Vec<_> = ports(device)
                .into_iter()
                .filter(|p| p.circuit == entry.circuit && p.id != entry.id)
                .collect();
            if exits.len() != 1 {
                faults.push("流道出口不唯一");
                break;
            }
            current = (id, exits[0]);
        }
        if line.tank_id.is_none() {
            faults.push("缺少独立储罐");
        }
        if inlet && line.pump_id.is_none() {
            faults.push("进料线缺泵");
        }
        if (inlet || column) && line.valve_id.is_none() {
            faults.push(if inlet {
                "进料线缺阀门"
            } else {
                "产品线缺阀门"
            });
        }
        faults.sort_unstable();
        faults.dedup();
        line.valid = faults.is_empty();
        line.message = if line.valid {
            "部件与连线齐全".into()
        } else {
            faults.join("；")
        };
        while line.message.len() > 256 {
            line.message.pop();
        }
        report.lines.push(line);
    }
    // Mark both sides of a shared reservoir, not only the later traversal.
    for i in 0..report.lines.len() {
        if report.lines[i].tank_id.is_some()
            && report
                .lines
                .iter()
                .enumerate()
                .any(|(j, line)| j != i && line.tank_id == report.lines[i].tank_id)
        {
            report.lines[i].valid = false;
            report.lines[i].message = "每条线路必须连接独立储罐。".into();
        }
    }
    for (a, b) in if column { vec![] } else { vec![(0, 1), (2, 3)] } {
        if let (Some(left), Some(right)) = (report.lines[a].tank_id, report.lines[b].tank_id) {
            if devices[&left].kind != devices[&right].kind {
                for i in [a, b] {
                    report.lines[i].valid = false;
                    report.lines[i].message = "同一流道不能混接常压水罐与烃密闭罐。".into();
                }
            }
        }
    }
    report.status = if report.lines.len() == (if column { 3 } else { 4 })
        && report.lines.iter().all(|l| l.valid)
    {
        AssemblyStatus::Complete
    } else {
        AssemblyStatus::Incomplete
    };
    report.message = if column {
        "蒸馏塔三罐外部接管检查；不代表完整冷凝、回流、再沸装置或运行性能。".into()
    } else {
        "四罐双流换热单元：A0/B0 为进料，各需一泵一阀；A1/B1 为出料。仅检查结构。".into()
    };
    report
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn device(id: u64, kind: EquipmentKind, origin: [i32; 3]) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id,
            revision: 0,
            kind,
            origin,
            axis: 0,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: (kind == EquipmentKind::HeatExchanger).then(Default::default),
            reading: None,
        }
    }
    pub(crate) fn unit() -> BTreeMap<u64, EquipmentSnapshot> {
        let mut ds = BTreeMap::from([(1, device(1, EquipmentKind::HeatExchanger, [0, 1, 0]))]);
        for (port, z) in [(0, 0), (2, 2)] {
            let base = 10 + port * 10;
            for (offset, kind, x) in [
                (0, EquipmentKind::Tank, -5),
                (1, EquipmentKind::Pump, -3),
                (2, EquipmentKind::Valve, -2),
                (3, EquipmentKind::Pipe, -1),
            ] {
                ds.insert(base + offset, device(base + offset, kind, [x, 1, z]));
            }
            ds.insert(base + 4, device(base + 4, EquipmentKind::Pipe, [2, 1, z]));
            ds.insert(base + 5, device(base + 5, EquipmentKind::Tank, [3, 1, z]));
        }
        ds
    }
    #[test]
    fn complete_unit_and_missing_direction_and_unsupported_parts() {
        let mut ds = unit();
        assert_eq!(inspect(&ds, 1, 1).status, AssemblyStatus::Complete);
        ds.get_mut(&15).unwrap().kind = EquipmentKind::Vessel;
        assert!(inspect(&ds, 1, 1).lines[1].message.contains("混接"));
        ds.get_mut(&15).unwrap().kind = EquipmentKind::Tank;
        ds.get_mut(&11).unwrap().pump_reversed = true;
        let report = inspect(&ds, 1, 2);
        assert!(report.valid());
        assert!(report.lines[0].message.contains("方向"));
        ds.remove(&12);
        assert!(inspect(&ds, 1, 3).lines[0].message.contains("断管"));
        ds.insert(12, device(12, EquipmentKind::Pipe, [-2, 1, 0]));
        assert!(inspect(&ds, 1, 4).lines[0].message.contains("缺阀门"));
        ds.remove(&11);
        ds.insert(12, device(12, EquipmentKind::HeatExchanger, [-3, 1, 0]));
        assert!(inspect(&ds, 1, 5).lines[0].message.contains("不支持"));
    }
    #[test]
    fn column_external_lines_require_three_vessels_valves_and_feed_direction() {
        let mut column = device(1, EquipmentKind::Column, [0, 10, 0]);
        let mut design = protocol::ColumnDesign::default();
        design.spec.trays = 2;
        design.spec.feed_tray = 1;
        column.column_design = Some(design);
        let mut ds = BTreeMap::from([(1, column)]);
        for (id, kind, origin) in [
            (10, EquipmentKind::Vessel, [-5, 12, 0]),
            (11, EquipmentKind::Pump, [-3, 12, 0]),
            (12, EquipmentKind::Valve, [-2, 12, 0]),
            (13, EquipmentKind::Pipe, [-1, 12, 0]),
            (20, EquipmentKind::Valve, [2, 13, 0]),
            (21, EquipmentKind::Vessel, [3, 13, 0]),
            (30, EquipmentKind::Valve, [2, 10, 0]),
            (31, EquipmentKind::Vessel, [3, 10, 0]),
        ] {
            ds.insert(id, device(id, kind, origin));
        }
        let complete = inspect(&ds, 1, 1);
        assert!(complete.valid());
        assert_eq!(complete.status, AssemblyStatus::Complete);
        assert_eq!(complete.lines.len(), 3);

        ds.get_mut(&20).unwrap().kind = EquipmentKind::Pipe;
        let report = inspect(&ds, 1, 2);
        assert_eq!(report.status, AssemblyStatus::Incomplete);
        assert!(report.lines[1].message.contains("缺阀门"));
        ds.get_mut(&20).unwrap().kind = EquipmentKind::Valve;
        ds.get_mut(&11).unwrap().pump_reversed = true;
        let report = inspect(&ds, 1, 3);
        assert_eq!(report.status, AssemblyStatus::Incomplete);
        assert!(report.lines[0].message.contains("方向"));
        ds.get_mut(&11).unwrap().pump_reversed = false;
        ds.get_mut(&31).unwrap().kind = EquipmentKind::Tank;
        let report = inspect(&ds, 1, 4);
        assert_eq!(report.status, AssemblyStatus::Incomplete);
        assert!(report.lines[2].message.contains("密闭罐"));

        // Join the two product lines vertically back to the same column.
        ds.remove(&21);
        ds.remove(&31);
        for (id, y, ends) in [
            (40, 13, [0, 2]),
            (41, 12, [2, 3]),
            (42, 11, [2, 3]),
            (43, 10, [0, 3]),
        ] {
            let mut pipe = device(id, EquipmentKind::Pipe, [3, y, 0]);
            pipe.pipe_ends = Some(ends);
            ds.insert(id, pipe);
        }
        let report = inspect(&ds, 1, 5);
        assert_eq!(report.status, AssemblyStatus::Incomplete);
        assert!(report.lines[1].message.contains("回环"));
        assert!(report.lines[2].message.contains("回环"));
    }
    #[test]
    fn structure_ignores_process_facts_but_tracks_geometry_and_direction() {
        let mut ds = unit();
        let old = structure(&ds);
        ds.get_mut(&10).unwrap().internal_energy_j = 123.0;
        ds.get_mut(&11).unwrap().enabled = true;
        ds.get_mut(&12).unwrap().opening = 0.4;
        ds.get_mut(&1).unwrap().revision += 1;
        assert_eq!(old, structure(&ds));
        ds.get_mut(&10).unwrap().tank_size = Some(TankSize {
            diameter: 3,
            height: 5,
        });
        assert_ne!(old, structure(&ds));
    }
    #[test]
    fn repeated_tank_and_cycles_never_pass() {
        let mut ds = unit();
        // Connect the two exchanger A ports back through pipes without a reservoir.
        ds.remove(&10);
        ds.remove(&15);
        ds.get_mut(&13).unwrap().pipe_ends = Some([1, 3]);
        ds.get_mut(&14).unwrap().pipe_ends = Some([0, 3]);
        for (id, x, ends) in [
            (50, -1, [1, 2]),
            (51, 0, [0, 1]),
            (52, 1, [0, 1]),
            (53, 2, [0, 2]),
        ] {
            let mut d = device(id, EquipmentKind::Pipe, [x, 2, 0]);
            d.pipe_ends = Some(ends);
            ds.insert(id, d);
        }
        let report = inspect(&ds, 1, 1);
        assert_eq!(report.status, AssemblyStatus::Incomplete);
        assert!(report.lines[0].message.contains("回环"));
    }
}

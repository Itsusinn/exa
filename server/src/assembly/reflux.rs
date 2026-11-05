//! Fixed, bounded process-line blueprint for the non-operating reflux-column foundation.
//! Utility nozzles are deliberately outside this report; no heat source is implied.
use super::*;

const LABELS: [&str; 9] = [
    "进料",
    "塔顶蒸气",
    "冷凝液",
    "回流输送",
    "回流返回",
    "馏出产品",
    "塔底液",
    "再沸蒸气",
    "塔底产品",
];

#[cfg(test)]
#[path = "../../tests/support/reflux_fixture.rs"]
pub(crate) mod fixture;

#[derive(Default)]
struct Walk {
    end: Option<(u64, u8)>,
    inline: Vec<u64>,
    pumps: Vec<u64>,
    valves: Vec<u64>,
    faults: Vec<String>,
}

struct Graph<'a> {
    devices: &'a BTreeMap<u64, EquipmentSnapshot>,
    index: BTreeMap<[i32; 3], Vec<(u64, EquipmentPort)>>,
}
impl<'a> Graph<'a> {
    fn new(devices: &'a BTreeMap<u64, EquipmentSnapshot>) -> Self {
        let mut index: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for device in devices.values() {
            for port in ports(device) {
                index.entry(key(port)).or_default().push((device.id, port));
            }
        }
        Self { devices, index }
    }
    fn port(&self, id: u64, port: u8) -> Option<EquipmentPort> {
        ports(self.devices.get(&id)?)
            .into_iter()
            .find(|p| p.id == port)
    }
    fn walk(&self, start: Option<(u64, u8)>, upstream: bool) -> Walk {
        let mut result = Walk::default();
        let Some((start_id, start_port)) = start else {
            result.faults.push("前置部件缺失".into());
            return result;
        };
        let Some(port) = self.port(start_id, start_port) else {
            result.faults.push("起始接口无效".into());
            return result;
        };
        let mut current = (start_id, port);
        let mut visited = BTreeSet::from([start_id]);
        for _ in 0..MAX_EQUIPMENT {
            let next: Vec<_> = self
                .index
                .get(&key(current.1))
                .into_iter()
                .flatten()
                .filter(|(id, port)| *id != current.0 && connected(current.1, *port))
                .copied()
                .collect();
            if next.len() != 1 {
                result.faults.push(
                    if next.is_empty() {
                        "断管或接口未连接"
                    } else {
                        "同口存在歧义连接"
                    }
                    .into(),
                );
                return result;
            }
            let (id, entry) = next[0];
            let device = &self.devices[&id];
            if !matches!(
                device.kind,
                EquipmentKind::Pipe | EquipmentKind::Pump | EquipmentKind::Valve
            ) {
                result.end = Some((id, entry.id));
                return result;
            }
            if !visited.insert(id) {
                result.faults.push("管路形成回环".into());
                return result;
            }
            result.inline.push(id);
            match device.kind {
                EquipmentKind::Pump => {
                    result.pumps.push(id);
                    let expected = if upstream {
                        u8::from(!device.pump_reversed)
                    } else {
                        u8::from(device.pump_reversed)
                    };
                    if entry.id != expected {
                        result.faults.push("泵方向错误".into());
                    }
                }
                EquipmentKind::Valve => result.valves.push(id),
                _ => (),
            }
            let exits: Vec<_> = ports(device)
                .into_iter()
                .filter(|p| p.circuit == entry.circuit && p.id != entry.id)
                .collect();
            if exits.len() != 1 {
                result.faults.push("流道出口不唯一".into());
                return result;
            }
            current = (id, exits[0]);
        }
        result.faults.push("管路超过检查上限".into());
        result
    }
    fn endpoint(
        &self,
        walk: &mut Walk,
        kind: EquipmentKind,
        port: Option<u8>,
    ) -> Option<(u64, u8)> {
        match walk.end {
            Some((id, entry))
                if self.devices[&id].kind == kind && port.is_none_or(|p| p == entry) =>
            {
                Some((id, entry))
            }
            Some(_) => {
                walk.faults
                    .push(format!("需要{}及正确工艺接口", kind.item_name()));
                None
            }
            None => None,
        }
    }
}

pub(super) fn inspect(
    devices: &BTreeMap<u64, EquipmentSnapshot>,
    master_id: u64,
    request_id: u64,
) -> AssemblyReport {
    let mut report = AssemblyReport {
        request_id,
        master_id,
        master_kind: EquipmentKind::RefluxColumn,
        status: AssemblyStatus::Rejected,
        lines: vec![],
        message: "冷凝、回流与再沸工艺接管；公用工程口未检查，装置尚未运行。".into(),
    };
    if devices.len() > MAX_EQUIPMENT || devices.values().any(|d| !d.valid()) {
        report.message = "设备数量或设备事实无效。".into();
        return report;
    }
    let graph = Graph::new(devices);
    let mut walks: [Walk; 9] = std::array::from_fn(|_| Walk::default());
    walks[0] = graph.walk(Some((master_id, 0)), true);
    let feed = graph.endpoint(&mut walks[0], EquipmentKind::Vessel, None);
    walks[1] = graph.walk(Some((master_id, 1)), false);
    let condenser = graph.endpoint(&mut walks[1], EquipmentKind::Condenser, Some(0));
    walks[2] = graph.walk(condenser.map(|(id, _)| (id, 1)), false);
    let receiver = graph.endpoint(&mut walks[2], EquipmentKind::Vessel, None);
    walks[3] = graph.walk(receiver.map(|(id, entry)| (id, 1 - entry)), false);
    let tee = graph.endpoint(&mut walks[3], EquipmentKind::Tee, None);
    let mut distillate = None;
    if let Some((tee_id, entry)) = tee {
        let branches: Vec<_> = ports(&devices[&tee_id])
            .into_iter()
            .filter(|p| p.id != entry)
            .collect();
        let mut has_reflux = false;
        let mut has_product = false;
        for branch in branches {
            let mut walk = graph.walk(Some((tee_id, branch.id)), false);
            if walk.end == Some((master_id, 3)) && !has_reflux {
                has_reflux = true;
                walks[4] = walk;
            } else if walk
                .end
                .is_some_and(|(id, _)| devices[&id].kind == EquipmentKind::Vessel)
                && !has_product
            {
                has_product = true;
                distillate = graph.endpoint(&mut walk, EquipmentKind::Vessel, None);
                walks[5] = walk;
            } else {
                let slot = if !has_reflux { 4 } else { 5 };
                walk.faults
                    .push("三通支路必须分别接塔回流口与独立馏出罐".into());
                walks[slot] = walk;
            }
        }
        if !has_reflux {
            walks[4].faults.push("缺少到本塔回流口的支路".into());
        }
        if !has_product {
            walks[5].faults.push("缺少独立馏出罐支路".into());
        }
    } else {
        walks[4].faults.push("缺少回流三通".into());
        walks[5].faults.push("缺少回流三通".into());
    }
    walks[6] = graph.walk(Some((master_id, 2)), false);
    let reboiler = graph.endpoint(&mut walks[6], EquipmentKind::Reboiler, Some(0));
    walks[7] = graph.walk(reboiler.map(|(id, _)| (id, 2)), false);
    if walks[7].end != Some((master_id, 4)) {
        walks[7].faults.push("再沸蒸气必须返回本塔返汽口".into());
    }
    walks[8] = graph.walk(reboiler.map(|(id, _)| (id, 1)), false);
    let bottoms = graph.endpoint(&mut walks[8], EquipmentKind::Vessel, None);

    let mut claimed = BTreeMap::<u64, usize>::new();
    for row in 0..9 {
        let pumps = usize::from(matches!(row, 0 | 3));
        let valves = usize::from(matches!(row, 0 | 4 | 5 | 8));
        if walks[row].pumps.len() != pumps {
            walks[row].faults.push(format!("该线需要{pumps}台泵"));
        }
        if walks[row].valves.len() != valves {
            walks[row].faults.push(format!("该线需要{valves}个阀门"));
        }
        for id in walks[row].inline.clone() {
            if let Some(previous) = claimed.insert(id, row) {
                walks[row].faults.push("工艺线共用管道或部件".into());
                walks[previous].faults.push("工艺线共用管道或部件".into());
            }
        }
    }
    let reservoirs = [(0, feed), (2, receiver), (5, distillate), (8, bottoms)];
    for &(row, terminal) in &reservoirs {
        if let Some((id, entry)) = terminal {
            if reservoirs.iter().any(|&(other, end)| {
                other != row && end.is_some_and(|(other_id, _)| id == other_id)
            }) {
                walks[row]
                    .faults
                    .push("进料、回流和两产品必须使用四个独立密闭罐".into());
            }
            // Product and feed reservoirs must not silently join another process group.
            // The receiver intentionally uses both of its two ports.
            if row != 2 {
                let spare = graph.port(id, 1 - entry).unwrap();
                if graph.index.get(&key(spare)).is_some_and(|same| {
                    same.iter()
                        .any(|&(other, p)| other != id && connected(spare, p))
                }) {
                    walks[row].faults.push("储罐另一口连接了额外线路".into());
                }
            }
        }
    }
    report.lines = walks
        .into_iter()
        .enumerate()
        .map(|(row, mut walk)| {
            walk.faults.sort();
            walk.faults.dedup();
            let valid = walk.faults.is_empty();
            let mut message = if valid {
                format!(
                    "{}：部件与连线齐全（端点 #{}）",
                    LABELS[row],
                    walk.end.unwrap().0
                )
            } else {
                format!("{}：{}", LABELS[row], walk.faults.join("；"))
            };
            while message.len() > 256 {
                message.pop();
            }
            AssemblyLine {
                port: row as u8,
                tank_id: walk
                    .end
                    .filter(|(id, _)| devices[id].kind == EquipmentKind::Vessel)
                    .map(|(id, _)| id),
                pump_id: walk.pumps.first().copied(),
                valve_id: walk.valves.first().copied(),
                valid,
                message,
            }
        })
        .collect();
    report.status = if report.lines.iter().all(|line| line.valid) {
        AssemblyStatus::Complete
    } else {
        AssemblyStatus::Incomplete
    };
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_simulation::equipment::occupied_cells;

    #[test]
    fn reflux_blueprint_has_nine_role_checked_lines_and_nonoverlapping_geometry() {
        for offset in [[0, 10, 0], [-20, 81, -20]] {
            let devices = fixture::fixture(offset);
            let mut occupied = BTreeMap::new();
            for device in devices.values() {
                assert!(device.valid(), "{device:?}");
                for cell in occupied_cells(device) {
                    assert!(
                        occupied.insert(cell, device.id).is_none(),
                        "overlap at {cell:?}"
                    );
                }
            }
            let report = inspect(&devices, 1, 42);
            assert!(report.valid(), "{report:?}");
            assert_eq!(report.status, AssemblyStatus::Complete, "{report:?}");
            assert_eq!(report.lines.len(), 9);
            assert_eq!(report.lines[0].tank_id, Some(2));
            assert_eq!(report.lines[2].tank_id, Some(4));
            assert_eq!(report.lines[5].tank_id, Some(6));
            assert_eq!(report.lines[8].tank_id, Some(8));
            assert!(report.message.contains("公用工程口未检查"));
        }
    }

    #[test]
    fn reflux_blueprint_rejects_missing_wrong_direction_role_and_extra_connections() {
        let initial = fixture::fixture([0, 10, 0]);
        for (missing, line) in [(3, 1), (4, 2), (5, 3), (13, 4), (14, 5), (7, 6), (15, 8)] {
            let mut changed = initial.clone();
            changed.remove(&missing);
            let report = inspect(&changed, 1, 1);
            assert!(report.valid());
            assert_eq!(report.status, AssemblyStatus::Incomplete);
            assert!(!report.lines[line].valid, "removed {missing}: {report:?}");
        }
        let mut changed = initial.clone();
        changed.get_mut(&12).unwrap().pump_reversed = true;
        assert!(inspect(&changed, 1, 1).lines[3].message.contains("方向"));
        let mut changed = initial.clone();
        changed.get_mut(&6).unwrap().kind = EquipmentKind::Tank;
        assert!(!inspect(&changed, 1, 1).lines[5].valid);
        let mut changed = initial.clone();
        let id = changed
            .values()
            .find(|d| d.origin == [7, 12, 0])
            .unwrap()
            .id;
        changed.get_mut(&id).unwrap().pipe_ends = Some([0, 3]);
        assert!(!inspect(&changed, 1, 1).lines[7].valid);
        let mut changed = initial.clone();
        let extra = super::super::tests::device(999, EquipmentKind::Pipe, [22, 17, 0]);
        changed.insert(extra.id, extra);
        let report = inspect(&changed, 1, 1);
        assert!(!report.lines[5].valid);
        assert!(report.lines[5].message.contains("额外线路"));
        // Utility connections are visible geometry but not claimed as process validation.
        let mut changed = initial;
        let utility = super::super::tests::device(999, EquipmentKind::Pipe, [5, 16, 2]);
        changed.insert(utility.id, utility);
        assert_eq!(inspect(&changed, 1, 1).status, AssemblyStatus::Complete);
    }
}

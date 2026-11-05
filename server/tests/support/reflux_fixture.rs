//! Shared test-only reflux process layout. Coordinates are relative to `offset`.
use exa_protocol::{ColumnDesign, EquipmentKind, EquipmentSnapshot};
use exa_simulation::equipment::{face_normal, ports};
use std::collections::BTreeMap;

fn device(id: u64, kind: EquipmentKind, origin: [i32; 3]) -> EquipmentSnapshot {
    EquipmentSnapshot {
        id,
        revision: 0,
        kind,
        origin,
        axis: 0,
        tank_size: None,
        column_design: kind.is_column().then(ColumnDesign::default),
        pipe_ends: None,
        contents: vec![],
        internal_energy_j: 0.0,
        enabled: true,
        opening: 1.0,
        pump_reversed: false,
        thermal: None,
        reading: None,
    }
}

/// Main equipment IDs 1..8, pump/valve IDs 10..15, pipe IDs >=100.
/// Occupied bounds before offset: x -6..=21, y 0..=9, z -4..=4.
pub fn fixture(offset: [i32; 3]) -> BTreeMap<u64, EquipmentSnapshot> {
    let mut ds = BTreeMap::new();
    for (id, kind, origin) in [
        (1, EquipmentKind::RefluxColumn, [0, 0, 0]),
        (2, EquipmentKind::Vessel, [-6, 4, 0]),
        (3, EquipmentKind::Condenser, [6, 6, 0]),
        (4, EquipmentKind::Vessel, [12, 7, 0]),
        (5, EquipmentKind::Tee, [17, 7, 0]),
        (6, EquipmentKind::Vessel, [20, 7, 0]),
        (7, EquipmentKind::Reboiler, [5, 0, 0]),
        (8, EquipmentKind::Vessel, [10, 0, 0]),
    ] {
        ds.insert(id, device(id, kind, origin));
    }
    ds.get_mut(&5).unwrap().axis = 3; // X trunk, +Z branch.
    let mut next = 100;
    let mut path = |from: (u64, usize),
                    to: (u64, usize),
                    cells: Vec<[i32; 3]>,
                    fittings: &[([i32; 3], u64, EquipmentKind)]| {
        let start = ports(&ds[&from.0])[from.1];
        let finish = ports(&ds[&to.0])[to.1];
        let face = |normal: [i32; 3]| (0..6).find(|&f| face_normal(f) == Some(normal)).unwrap();
        for (index, &cell) in cells.iter().enumerate() {
            let before = if index == 0 {
                start.normal.map(|v| -v)
            } else {
                std::array::from_fn(|a| cells[index - 1][a] - cell[a])
            };
            let after = if index + 1 == cells.len() {
                finish.normal.map(|v| -v)
            } else {
                std::array::from_fn(|a| cells[index + 1][a] - cell[a])
            };
            let (id, kind) = fittings
                .iter()
                .find(|(position, _, _)| *position == cell)
                .map(|(_, id, kind)| (*id, *kind))
                .unwrap_or_else(|| {
                    let id = next;
                    next += 1;
                    (id, EquipmentKind::Pipe)
                });
            let mut d = device(id, kind, cell);
            if kind == EquipmentKind::Pipe {
                let mut ends = [face(before), face(after)];
                ends.sort_unstable();
                d.pipe_ends = Some(ends);
            } else {
                assert_eq!(
                    before,
                    after.map(|v| -v),
                    "fitting must be on a straight segment"
                );
                d.axis = before.iter().position(|v| *v != 0).unwrap() as u8;
            }
            assert!(ds.insert(id, d).is_none());
        }
    };
    path(
        (1, 0),
        (2, 1),
        (-4..=-1).rev().map(|x| [x, 4, 0]).collect(),
        &[
            ([-4, 4, 0], 10, EquipmentKind::Pump),
            ([-3, 4, 0], 11, EquipmentKind::Valve),
        ],
    );
    path((1, 1), (3, 0), (2..=5).map(|x| [x, 7, 0]).collect(), &[]);
    path((3, 1), (4, 0), (9..=11).map(|x| [x, 7, 0]).collect(), &[]);
    path(
        (4, 1),
        (5, 0),
        (14..=16).map(|x| [x, 7, 0]).collect(),
        &[([14, 7, 0], 12, EquipmentKind::Pump)],
    );
    let mut reflux: Vec<_> = (1..=4).map(|z| [17, 7, z]).collect();
    reflux.extend((0..=16).rev().map(|x| [x, 7, 4]));
    reflux.extend([[0, 6, 4], [0, 6, 3], [0, 6, 2]]);
    path(
        (5, 2),
        (1, 3),
        reflux,
        &[([8, 7, 4], 13, EquipmentKind::Valve)],
    );
    path(
        (5, 1),
        (6, 0),
        vec![[18, 7, 0], [19, 7, 0]],
        &[([18, 7, 0], 14, EquipmentKind::Valve)],
    );
    path((1, 2), (7, 0), (2..=4).map(|x| [x, 0, 0]).collect(), &[]);
    let mut vapour = vec![[7, 2, 0], [7, 3, 0]];
    vapour.extend((-4..=-1).rev().map(|z| [7, 3, z]));
    vapour.extend((0..=6).rev().map(|x| [x, 3, -4]));
    vapour.extend([[0, 2, -4], [0, 1, -4], [0, 1, -3], [0, 1, -2], [0, 1, -1]]);
    path((7, 2), (1, 4), vapour, &[]);
    path(
        (7, 1),
        (8, 0),
        vec![[8, 0, 0], [9, 0, 0]],
        &[([8, 0, 0], 15, EquipmentKind::Valve)],
    );
    for d in ds.values_mut() {
        d.origin = std::array::from_fn(|a| d.origin[a] + offset[a]);
    }
    ds
}

//! Bounded pipe planning and the identical structural checks used by authority.
//! The client searches known terrain. The server accepts only a complete checked
//! proposal; actor overlap, inventory and the player session are server concerns.
use crate::{equipment, grid::Grid, EYE_HEIGHT};
use protocol::{EquipmentKind, PipeAnchor, AIR, BLOCK_REACH, MAX_PIPE_ROUTE};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
};

pub const MAX_ROUTE_EXPANSIONS: usize = 8192;
type Cell = [i32; 3];

fn anchor_port(grid: &Grid, anchor: PipeAnchor) -> Result<equipment::EquipmentPort, String> {
    let device = grid
        .equipment
        .get(&anchor.id)
        .ok_or("pipe anchor is no longer loaded")?;
    if device.revision != anchor.revision {
        return Err("pipe anchor revision changed".into());
    }
    let port = equipment::ports(device)
        .into_iter()
        .find(|p| p.id == anchor.port)
        .ok_or("pipe anchor has no such port")?;
    if grid
        .equipment
        .values()
        .filter(|d| d.id != anchor.id)
        .any(|d| {
            equipment::ports(d)
                .into_iter()
                .any(|other| equipment::connected(port, other))
        })
    {
        return Err("pipe anchor is already connected".into());
    }
    Ok(port)
}

/// Resolve a visible, outward-facing, unconnected port on the device actually
/// hit by the shared world ray. Seeing a vessel does not expose its hidden back port.
pub fn resolve_anchor(
    grid: &Grid,
    feet: [f32; 3],
    yaw: f32,
    pitch: f32,
) -> Result<PipeAnchor, String> {
    let hit = equipment::target(grid, feet, yaw, pitch).ok_or("no loaded pipe anchor in sight")?;
    let id = hit
        .equipment
        .ok_or("aim at an equipment port or an open pipe end")?;
    let device = &grid.equipment[&id];
    let eye = [feet[0], feet[1] + EYE_HEIGHT, feet[2]];
    let direction = [
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    ];
    let point: [f32; 3] = std::array::from_fn(|i| eye[i] + direction[i] * hit.distance);
    let mut candidates: Vec<_> = equipment::ports(device)
        .into_iter()
        .filter_map(|port| {
            let delta: [f32; 3] = std::array::from_fn(|i| eye[i] - port.position[i]);
            let distance: f32 = delta.iter().map(|v| v * v).sum();
            let outward: f32 = (0..3).map(|i| delta[i] * port.normal[i] as f32).sum();
            let anchor = PipeAnchor {
                id,
                revision: device.revision,
                port: port.id,
            };
            if distance > BLOCK_REACH * BLOCK_REACH
                || outward < -0.001
                || anchor_port(grid, anchor).is_err()
            {
                return None;
            }
            let score: f32 = (0..3).map(|i| (point[i] - port.position[i]).powi(2)).sum();
            Some((score, anchor))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.port.cmp(&b.1.port)));
    candidates
        .first()
        .map(|(_, anchor)| *anchor)
        .ok_or_else(|| "no visible free port within reach".into())
}

pub fn anchor_cell(grid: &Grid, anchor: PipeAnchor) -> Result<Cell, String> {
    let port = anchor_port(grid, anchor)?;
    let cell =
        std::array::from_fn(|i| (port.position[i] + port.normal[i] as f32 * 0.5).floor() as i32);
    if !protocol::valid_voxel(cell) || cell[1] == 0 {
        return Err("pipe anchor faces outside build bounds".into());
    }
    Ok(cell)
}

fn face(normal: [i32; 3]) -> Result<u8, String> {
    (0..6)
        .find(|&f| equipment::face_normal(f) == Some(normal))
        .ok_or_else(|| "pipe route has an invalid face".into())
}
fn direction(from: Cell, to: Cell) -> Result<u8, String> {
    let mut delta = [0; 3];
    for i in 0..3 {
        delta[i] = to[i]
            .checked_sub(from[i])
            .ok_or("pipe route step overflows coordinates")?;
    }
    face(delta)
}
fn manhattan(a: Cell, b: Cell) -> usize {
    (0..3).map(|i| (a[i] - b[i]).unsigned_abs() as usize).sum()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Room {
    Free,
    Rack,
    Blocked,
}
fn occupied(grid: &Grid) -> BTreeMap<Cell, Room> {
    let mut occupied = BTreeMap::new();
    for device in grid.equipment.values() {
        for cell in equipment::occupied_cells(device) {
            occupied
                .entry(cell)
                .and_modify(|room| *room = Room::Blocked)
                .or_insert(if device.kind == EquipmentKind::PipeRack {
                    Room::Rack
                } else {
                    Room::Blocked
                });
        }
    }
    occupied
}
fn room(grid: &Grid, occupied: &BTreeMap<Cell, Room>, cell: Cell) -> Room {
    if cell[1] == 0 || grid.get(cell) != AIR {
        return Room::Blocked;
    }
    occupied.get(&cell).copied().unwrap_or(Room::Free)
}
fn admits(room: Room, ends: [u8; 2]) -> bool {
    room == Room::Free || (room == Room::Rack && ends.iter().all(|f| *f != 2 && *f != 3))
}

fn endpoints(
    grid: &Grid,
    start: PipeAnchor,
    end: PipeAnchor,
) -> Result<(Cell, u8, Cell, u8), String> {
    if start.id == end.id {
        return Err("choose ports on two different devices".into());
    }
    let a = anchor_port(grid, start)?;
    let b = anchor_port(grid, end)?;
    let a_cell = anchor_cell(grid, start)?;
    let b_cell = anchor_cell(grid, end)?;
    Ok((
        a_cell,
        face(a.normal.map(|n| -n))?,
        b_cell,
        face(b.normal.map(|n| -n))?,
    ))
}

/// Check a complete path and return canonical face pairs, one per new pipe.
pub fn validate(
    grid: &Grid,
    start: PipeAnchor,
    end: PipeAnchor,
    cells: &[Cell],
) -> Result<Vec<[u8; 2]>, String> {
    if cells.is_empty() || cells.len() > MAX_PIPE_ROUTE {
        return Err("pipe route must contain 1 to 32 new pipe cells".into());
    }
    let (a, enter, b, leave) = endpoints(grid, start, end)?;
    if cells.first() != Some(&a) || cells.last() != Some(&b) {
        return Err("pipe route no longer matches its endpoints".into());
    }
    let mut seen = BTreeSet::new();
    let occupied = occupied(grid);
    let mut result = Vec::with_capacity(cells.len());
    for (i, &cell) in cells.iter().enumerate() {
        if !protocol::valid_voxel(cell) || !seen.insert(cell) {
            return Err("pipe route repeats a cell or exceeds world bounds".into());
        }
        let mut ends = [
            if i == 0 {
                enter
            } else {
                direction(cell, cells[i - 1])?
            },
            if i + 1 == cells.len() {
                leave
            } else {
                direction(cell, cells[i + 1])?
            },
        ];
        if ends[0] == ends[1] {
            return Err("pipe route folds back through the same face".into());
        }
        ends.sort_unstable();
        if !admits(room(grid, &occupied, cell), ends) {
            return Err("pipe route is blocked, unknown, or crosses a rack vertically".into());
        }
        result.push(ends);
    }
    Ok(result)
}

/// Deterministic six-neighbour A*. It never loads terrain and refuses to search
/// beyond its cell and expansion budgets. A one-point rack carries only horizontal arms.
pub fn plan(grid: &Grid, start: PipeAnchor, end: PipeAnchor) -> Result<Vec<Cell>, String> {
    let (a, enter, b, leave) = endpoints(grid, start, end)?;
    if manhattan(a, b) + 1 > MAX_PIPE_ROUTE {
        return Err("pipe route exceeds 32 new cells".into());
    }
    let mut rooms = BTreeMap::new();
    let occupied = occupied(grid);
    let mut room_at = |cell| {
        *rooms
            .entry(cell)
            .or_insert_with(|| room(grid, &occupied, cell))
    };
    if room_at(a) == Room::Blocked || room_at(b) == Room::Blocked {
        return Err("pipe route endpoint is blocked or unknown".into());
    }
    type Entry = (usize, usize, u64, Cell, u8, Vec<Cell>);
    let mut open = BinaryHeap::<Reverse<Entry>>::new();
    let mut best = BTreeMap::new();
    let mut serial = 0;
    open.push(Reverse((
        manhattan(a, b) * 10,
        0,
        serial,
        a,
        enter,
        vec![a],
    )));
    best.insert((a, enter), 0);
    for _ in 0..MAX_ROUTE_EXPANSIONS {
        let Some(Reverse((_, g, _, cell, incoming, path))) = open.pop() else {
            return Err("no pipe route through known free cells".into());
        };
        if best.get(&(cell, incoming)) != Some(&g) {
            continue;
        }
        if cell == b && incoming != leave && admits(room_at(cell), [incoming, leave]) {
            validate(grid, start, end, &path)?;
            return Ok(path);
        }
        if path.len() == MAX_PIPE_ROUTE {
            continue;
        }
        for outgoing in 0..6 {
            if outgoing == incoming || !admits(room_at(cell), [incoming, outgoing]) {
                continue;
            }
            let normal = equipment::face_normal(outgoing).expect("six route faces");
            let next = std::array::from_fn(|i| cell[i] + normal[i]);
            if path.contains(&next)
                || path.len() + 1 + manhattan(next, b) > MAX_PIPE_ROUTE
                || room_at(next) == Room::Blocked
            {
                continue;
            }
            let next_incoming = outgoing ^ 1;
            if room_at(next) == Room::Rack && (next_incoming == 2 || next_incoming == 3) {
                continue;
            }
            let cost = g + 10 + usize::from((incoming ^ 1) != outgoing);
            let key = (next, next_incoming);
            if best.get(&key).is_some_and(|earlier| *earlier <= cost) {
                continue;
            }
            best.insert(key, cost);
            let mut next_path = path.clone();
            next_path.push(next);
            serial += 1;
            open.push(Reverse((
                cost + manhattan(next, b) * 10,
                cost,
                serial,
                next,
                next_incoming,
                next_path,
            )));
        }
    }
    Err("pipe route search reached its 8192 expansion limit".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{EquipmentSnapshot, STONE};

    fn pipe(id: u64, origin: Cell, axis: u8) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id,
            revision: 7,
            kind: EquipmentKind::Pipe,
            origin,
            axis,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            thermal: None,
            reading: None,
        }
    }
    fn anchor(id: u64, port: u8) -> PipeAnchor {
        PipeAnchor {
            id,
            revision: 7,
            port,
        }
    }
    fn grid() -> Grid {
        let mut grid = Grid::unknown();
        for x in [-16, 0, 16, 32] {
            for y in [0, 16, 32] {
                for z in [-16, 0, 16] {
                    assert!(grid.apply_chunk([x, y, z], &vec![AIR; 4096]));
                }
            }
        }
        grid
    }
    fn endpoints_at(grid: &mut Grid, a: Cell, b: Cell) -> (PipeAnchor, PipeAnchor) {
        grid.equipment.insert(1, pipe(1, a, 0));
        grid.equipment.insert(2, pipe(2, b, 0));
        (anchor(1, 1), anchor(2, 0))
    }
    #[test]
    fn route_crosses_height_and_negative_sections_with_exact_connected_ends() {
        let mut g = grid();
        g.equipment.insert(1, pipe(1, [-4, 4, -2], 0));
        g.equipment.insert(2, pipe(2, [3, 7, 4], 2));
        let a = anchor(1, 1);
        let b = anchor(2, 0);
        let cells = plan(&g, a, b).unwrap();
        assert_eq!(cells.first(), Some(&[-3, 4, -2]));
        assert_eq!(cells.last(), Some(&[3, 7, 3]));
        assert_eq!(cells, plan(&g, a, b).unwrap());
        let ends = validate(&g, a, b, &cells).unwrap();
        assert!(ends.iter().any(|pair| pair[0] / 2 != pair[1] / 2));
        let mut laid = vec![];
        for (i, (&origin, &faces)) in cells.iter().zip(&ends).enumerate() {
            let mut d = pipe(i as u64 + 3, origin, 0);
            d.pipe_ends = Some(faces);
            assert!(d.valid());
            laid.push(d);
        }
        let start = anchor_port(&g, a).unwrap();
        let end = anchor_port(&g, b).unwrap();
        assert!(equipment::ports(&laid[0])
            .into_iter()
            .any(|p| equipment::connected(start, p)));
        assert!(equipment::ports(laid.last().unwrap())
            .into_iter()
            .any(|p| equipment::connected(end, p)));
        for pair in laid.windows(2) {
            assert_eq!(
                equipment::ports(&pair[0])
                    .into_iter()
                    .filter(|a| {
                        equipment::ports(&pair[1])
                            .into_iter()
                            .any(|b| equipment::connected(*a, b))
                    })
                    .count(),
                1
            );
        }
        for d in laid {
            g.equipment.insert(d.id, d);
        }
        assert!(anchor_cell(&g, a).is_err());
    }
    #[test]
    fn route_detours_obstacles_rejects_unknown_and_does_not_mutate_grid() {
        let mut g = grid();
        let (a, b) = endpoints_at(&mut g, [-2, 4, 0], [3, 4, 0]);
        g.set([1, 4, 0], STONE);
        let before = g.clone();
        let cells = plan(&g, a, b).unwrap();
        assert!(!cells.contains(&[1, 4, 0]));
        assert_eq!(g, before);
        assert!(validate(&g, a, b, &[[-1, 4, 0], [0, 4, 0], [1, 4, 0], [2, 4, 0]]).is_err());
        g.blocks.clear();
        assert!(plan(&g, a, b).is_err());
    }
    #[test]
    fn validation_rejects_stale_broken_repeated_overlong_and_folded_routes() {
        let mut g = grid();
        let (a, b) = endpoints_at(&mut g, [-1, 4, 0], [4, 4, 0]);
        let cells = plan(&g, a, b).unwrap();
        let mut stale = a;
        stale.revision += 1;
        assert!(validate(&g, stale, b, &cells).is_err());
        assert!(validate(&g, a, b, &[]).is_err());
        let mut broken = cells.clone();
        broken[1] = [i32::MIN, 4, 0];
        assert!(validate(&g, a, b, &broken).is_err());
        let mut repeated = cells.clone();
        repeated.insert(1, cells[0]);
        assert!(validate(&g, a, b, &repeated).is_err());
        assert!(validate(&g, a, b, &vec![cells[0]; MAX_PIPE_ROUTE + 1]).is_err());
        g.equipment.insert(2, pipe(2, [0, 4, 1], 0));
        // Both anchors now face away from the one-cell proposal, not through it.
        assert!(validate(&g, a, b, &[[0, 4, 0]]).is_err());
    }
    #[test]
    fn exact_length_budget_and_bounded_unreachable_search() {
        let mut g = grid();
        let (a, b) = endpoints_at(&mut g, [-1, 4, 0], [32, 4, 0]);
        assert_eq!(plan(&g, a, b).unwrap().len(), MAX_PIPE_ROUTE);
        g.equipment.get_mut(&2).unwrap().origin[0] = 33;
        assert!(plan(&g, a, b).is_err());
        let (a, b) = endpoints_at(&mut g, [-8, 16, 0], [8, 16, 0]);
        let end = anchor_cell(&g, b).unwrap();
        for f in 0..6 {
            let n = equipment::face_normal(f).unwrap();
            g.set(std::array::from_fn(|i| end[i] + n[i]), STONE);
        }
        let before = g.clone();
        assert!(plan(&g, a, b).is_err());
        assert_eq!(g, before);
    }
    #[test]
    fn anchor_requires_visible_free_port_and_checked_world_ray() {
        let mut g = grid();
        g.equipment.insert(1, pipe(1, [5, 4, 5], 0));
        let yaw = std::f32::consts::PI * 1.5;
        assert_eq!(
            resolve_anchor(&g, [8.0, 3.0, 5.5], yaw, 0.0).unwrap(),
            anchor(1, 1)
        );
        assert!(resolve_anchor(&g, [11.0, 3.0, 5.5], yaw, 0.0).is_err());
        g.set([7, 4, 5], STONE);
        assert!(resolve_anchor(&g, [8.0, 3.0, 5.5], yaw, 0.0).is_err());
        g.set([7, 4, 5], AIR);
        g.equipment.insert(2, pipe(2, [6, 4, 5], 0));
        assert!(anchor_cell(&g, anchor(1, 1)).is_err());
    }
    #[test]
    fn single_cell_corner_and_horizontal_rack_share_geometry_without_false_ports() {
        let mut g = grid();
        g.equipment.insert(1, pipe(1, [-1, 4, 0], 0));
        g.equipment.insert(2, pipe(2, [0, 4, 1], 2));
        let mut rack = pipe(3, [0, 4, 0], 0);
        rack.kind = EquipmentKind::PipeRack;
        g.equipment.insert(3, rack.clone());
        let a = anchor(1, 1);
        let b = anchor(2, 0);
        assert_eq!(plan(&g, a, b).unwrap(), vec![[0, 4, 0]]);
        assert_eq!(validate(&g, a, b, &[[0, 4, 0]]).unwrap(), vec![[0, 5]]);
        let mut bend = pipe(4, [0, 4, 0], 0);
        bend.pipe_ends = Some([0, 5]);
        assert!(equipment::rack_compatible(&rack, &bend));
        assert!(equipment::ports(&rack).is_empty());
        for a in equipment::collision_boxes(&rack) {
            for b in equipment::collision_boxes(&bend) {
                assert!(!a.overlaps(&b));
            }
        }
        g.equipment.get_mut(&2).unwrap().origin = [0, 5, 0];
        g.equipment.get_mut(&2).unwrap().axis = 1;
        assert!(validate(&g, a, b, &[[0, 4, 0]]).is_err());
    }
}

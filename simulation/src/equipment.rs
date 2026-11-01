//! Shared device geometry, occupancy and ray selection for authority and prediction.
use crate::{grid::Grid, EYE_HEIGHT};
use protocol::{EquipmentKind, EquipmentSnapshot, AIR, BLOCK_REACH, WATER};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl CollisionBox {
    pub fn overlaps(&self, other: &Self) -> bool {
        (0..3).all(|i| self.min[i] < other.max[i] && self.max[i] > other.min[i])
    }
    pub fn contains(&self, point: [f32; 3]) -> bool {
        (0..3).all(|i| point[i] >= self.min[i] && point[i] < self.max[i])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EquipmentPort {
    pub position: [f32; 3],
    pub normal: [i32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldHit {
    pub position: [i32; 3],
    pub normal: [i32; 3],
    pub equipment: Option<u64>,
    pub distance: f32,
}

/// Anchor the entire footprint outside the selected voxel face, including on negative axes.
/// The authority still validates bounds, inventory and occupancy before committing placement.
pub fn placement_origin(hit: WorldHit, kind: EquipmentKind) -> [i32; 3] {
    let size = kind.size();
    std::array::from_fn(|i| match hit.normal[i] {
        -1 => hit.position[i].saturating_sub(size[i]),
        1 => hit.position[i].saturating_add(1),
        _ => hit.position[i],
    })
}

pub fn bounds(device: &EquipmentSnapshot) -> CollisionBox {
    let min = device.origin.map(|v| v as f32);
    let size = device.kind.size();
    CollisionBox {
        min,
        max: std::array::from_fn(|i| min[i] + size[i] as f32),
    }
}

pub fn occupied_cells(device: &EquipmentSnapshot) -> Vec<[i32; 3]> {
    let size = device.kind.size();
    (0..size[1])
        .flat_map(|y| (0..size[2]).flat_map(move |z| (0..size[0]).map(move |x| [x, y, z])))
        .filter_map(|offset| {
            Some([
                device.origin[0].checked_add(offset[0])?,
                device.origin[1].checked_add(offset[1])?,
                device.origin[2].checked_add(offset[2])?,
            ])
        })
        .collect()
}

/// Conservative circular tank envelope made of 32 narrow strips, plus its nozzles.
/// The same geometry is used for picking, server movement and client replay.
pub fn collision_boxes(device: &EquipmentSnapshot) -> Vec<CollisionBox> {
    let origin = device.origin.map(|v| v as f32);
    let translated = |min: [f32; 3], max: [f32; 3]| CollisionBox {
        min: std::array::from_fn(|i| origin[i] + min[i]),
        max: std::array::from_fn(|i| origin[i] + max[i]),
    };
    match device.kind {
        EquipmentKind::Pipe | EquipmentKind::Pump | EquipmentKind::Valve => {
            let mut min = [0.375; 3];
            let mut max = [0.625; 3];
            let axis = usize::from(device.axis.min(2));
            min[axis] = 0.0;
            max[axis] = 1.0;
            let mut shapes = vec![translated(min, max)];
            if device.kind != EquipmentKind::Pipe {
                shapes.push(translated([0.2; 3], [0.8; 3]));
            }
            shapes
        }
        EquipmentKind::Tank => {
            let mut boxes = Vec::with_capacity(34);
            for band in 0..32 {
                let z0 = 0.1 + band as f32 * (1.8 / 32.0);
                let z1 = z0 + 1.8 / 32.0;
                let nearest = if z0 <= 1.0 && z1 >= 1.0 {
                    0.0
                } else {
                    (z0 - 1.0).abs().min((z1 - 1.0).abs())
                };
                let radius = (0.81 - nearest * nearest).max(0.0).sqrt();
                boxes.push(translated(
                    [1.0 - radius, 0.0, z0],
                    [1.0 + radius, 2.85, z1],
                ));
            }
            boxes.push(translated([0.0, 0.375, 0.375], [0.4, 0.625, 0.625]));
            boxes.push(translated([1.6, 0.375, 0.375], [2.0, 0.625, 0.625]));
            boxes
        }
    }
}

pub fn ports(device: &EquipmentSnapshot) -> Vec<EquipmentPort> {
    let origin = device.origin.map(|v| v as f32);
    let axis = if device.kind == EquipmentKind::Tank {
        0
    } else {
        device.axis.min(2) as usize
    };
    let length = device.kind.size()[axis] as f32;
    [-1, 1]
        .into_iter()
        .map(|sign| {
            let mut position = origin.map(|v| v + 0.5);
            position[axis] = origin[axis] + if sign < 0 { 0.0 } else { length };
            let mut normal = [0; 3];
            normal[axis] = sign;
            EquipmentPort { position, normal }
        })
        .collect()
}

/// Geometric port connection, shared with the authoritative process graph.
pub fn connected(a: EquipmentPort, b: EquipmentPort) -> bool {
    a.position == b.position && (0..3).all(|i| a.normal[i] == -b.normal[i])
}

pub fn ray_box(
    eye: [f32; 3],
    direction: [f32; 3],
    bounds: CollisionBox,
) -> Option<(f32, [i32; 3])> {
    let mut near = 0.0f32;
    let mut far = f32::INFINITY;
    let mut normal = [0; 3];
    for axis in 0..3 {
        if direction[axis].abs() < 1e-7 {
            if eye[axis] < bounds.min[axis] || eye[axis] > bounds.max[axis] {
                return None;
            }
        } else {
            let a = (bounds.min[axis] - eye[axis]) / direction[axis];
            let b = (bounds.max[axis] - eye[axis]) / direction[axis];
            let enter = a.min(b);
            if enter > near {
                near = enter;
                normal = [0; 3];
                normal[axis] = if direction[axis] > 0.0 { -1 } else { 1 };
            }
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    (far >= 0.0).then_some((near, normal))
}

pub fn target(map: &Grid, feet: [f32; 3], yaw: f32, pitch: f32) -> Option<WorldHit> {
    if !feet.iter().all(|v| v.is_finite())
        || !yaw.is_finite()
        || !pitch.is_finite()
        || yaw.abs() > std::f32::consts::TAU
        || pitch.abs() > 1.5
    {
        return None;
    }
    let eye = [feet[0], feet[1] + EYE_HEIGHT, feet[2]];
    let direction = [
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    ];
    if !protocol::valid_voxel(eye.map(|v| v.floor() as i32)) {
        return None;
    }
    let mut closest = None;
    let mut distance = BLOCK_REACH + 1e-5;
    for device in map.equipment.values() {
        if !ray_box(eye, direction, bounds(device)).is_some_and(|(t, _)| t < distance) {
            continue;
        }
        for bounds in collision_boxes(device) {
            if let Some((t, normal)) = ray_box(eye, direction, bounds) {
                if t < distance && t <= BLOCK_REACH {
                    let size = device.kind.size();
                    let position = std::array::from_fn(|i| {
                        ((eye[i] + direction[i] * (t + 1e-4)).floor() as i32)
                            .clamp(device.origin[i], device.origin[i] + size[i] - 1)
                    });
                    closest = Some(WorldHit {
                        position,
                        normal,
                        equipment: Some(device.id),
                        distance: t,
                    });
                    distance = t;
                }
            }
        }
    }
    let mut cell = eye.map(|v| v.floor() as i32);
    let step = direction.map(|v| if v >= 0.0 { 1 } else { -1 });
    let delta = direction.map(|v| {
        if v.abs() < 1e-7 {
            f32::INFINITY
        } else {
            1.0 / v.abs()
        }
    });
    let mut next: [f32; 3] = std::array::from_fn(|i| {
        if delta[i].is_infinite() {
            f32::INFINITY
        } else {
            ((cell[i] + i32::from(step[i] > 0)) as f32 - eye[i]) / direction[i]
        }
    });
    let mut normal = [0; 3];
    let mut t = 0.0;
    for _ in 0..32 {
        if t > distance {
            return closest;
        }
        match map.get(cell) {
            255 => return None,
            AIR | WATER => {}
            _ => {
                return Some(WorldHit {
                    position: cell,
                    normal,
                    equipment: None,
                    distance: t,
                })
            }
        }
        let axis = (0..3).min_by(|&a, &b| next[a].total_cmp(&next[b]))?;
        t = next[axis];
        if t > BLOCK_REACH {
            return closest;
        }
        cell[axis] += step[axis];
        normal = [0; 3];
        normal[axis] = -step[axis];
        next[axis] += delta[axis];
    }
    closest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{overlaps_grid, step_player_in_grid};
    use protocol::{Body, PlayerInput, STONE};
    fn device(kind: EquipmentKind, origin: [i32; 3], axis: u8) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id: 1,
            revision: 0,
            kind,
            origin,
            axis,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            reading: None,
        }
    }
    #[test]
    fn ports_and_occupancy_work_across_negative_chunk_boundaries() {
        let tank = device(EquipmentKind::Tank, [-1, 1, -1], 0);
        assert_eq!(occupied_cells(&tank).len(), 12);
        assert!(occupied_cells(&tank).contains(&[0, 3, 0]));
        let pipe = device(EquipmentKind::Pipe, [1, 1, -1], 0);
        assert!(connected(ports(&tank)[1], ports(&pipe)[0]));
        assert!(!connected(
            ports(&tank)[1],
            ports(&device(EquipmentKind::Pipe, [1, 1, -1], 1))[0]
        ));
    }
    #[test]
    fn pump_valve_collision_and_ports_rotate_together() {
        for kind in [EquipmentKind::Pump, EquipmentKind::Valve] {
            for axis in 0..3 {
                let d = device(kind, [-1, 5, 2], axis);
                assert!(d.valid());
                assert_eq!(occupied_cells(&d).len(), 1);
                assert_eq!(collision_boxes(&d).len(), 2);
                let p = ports(&d);
                let mut next = d.clone();
                next.origin[axis as usize] += 1;
                assert!(connected(p[1], ports(&next)[0]));
                assert_eq!(p[1].normal[axis as usize], 1);
                assert!(collision_boxes(&d)
                    .iter()
                    .any(|b| b.contains([-0.5, 5.5, 2.5])));
            }
        }
    }

    #[test]
    fn placement_keeps_tanks_and_pipes_adjacent_on_all_six_faces() {
        for base_kind in [EquipmentKind::Tank, EquipmentKind::Pipe] {
            let base = device(base_kind, [-1, 10, -1], 0);
            let occupied = occupied_cells(&base);
            for kind in [EquipmentKind::Tank, EquipmentKind::Pipe] {
                for axis in 0..3 {
                    for sign in [-1, 1] {
                        let mut position = base.origin;
                        if sign > 0 {
                            position[axis] += base.kind.size()[axis] - 1;
                        }
                        let mut normal = [0; 3];
                        normal[axis] = sign;
                        let hit = WorldHit {
                            position,
                            normal,
                            equipment: Some(base.id),
                            distance: 1.0,
                        };
                        let placed = device(kind, placement_origin(hit, kind), 0);
                        assert!(placed.valid());
                        assert!(
                            occupied_cells(&placed)
                                .iter()
                                .all(|p| !occupied.contains(p)),
                            "{base_kind:?} -> {kind:?} face {normal:?}: {:?}",
                            placed.origin
                        );
                        if sign < 0 {
                            assert_eq!(placed.origin[axis] + kind.size()[axis], base.origin[axis]);
                        } else {
                            assert_eq!(
                                placed.origin[axis],
                                base.origin[axis] + base.kind.size()[axis]
                            );
                        }
                        for other in (0..3).filter(|i| *i != axis) {
                            assert_eq!(placed.origin[other], position[other]);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn picking_uses_nearest_shape_and_unknown_space_blocks_it() {
        let mut map = Grid::empty();
        map.equipment
            .insert(1, device(EquipmentKind::Tank, [5, 1, 5], 0));
        let feet = [6.0, 1.0, 9.0];
        assert_eq!(target(&map, feet, 0.0, 0.0).unwrap().equipment, Some(1));
        map.set([6, 2, 7], STONE);
        assert_eq!(target(&map, feet, 0.0, 0.0).unwrap().equipment, None);
        map.set([6, 2, 7], AIR);
        map.blocks.remove(&[0, 0, 0]);
        assert!(target(&map, feet, 0.0, 0.0).is_none());
    }

    #[test]
    fn pipe_ray_passes_its_empty_corner_and_cannot_cross_an_unknown_column() {
        let mut map = Grid::empty();
        map.equipment
            .insert(1, device(EquipmentKind::Pipe, [5, 1, 5], 1));
        map.set([5, 1, 4], STONE);
        assert_eq!(
            target(&map, [5.05, 0.0, 7.0], 0.0, 0.0).unwrap().equipment,
            None
        );
        assert_eq!(
            target(&map, [5.5, 0.0, 7.0], 0.0, 0.0).unwrap().equipment,
            Some(1)
        );
        let mut unknown = Grid::unknown();
        unknown.apply_chunk([0, 0, 0], &vec![AIR; 4096]);
        unknown
            .equipment
            .insert(1, device(EquipmentKind::Pipe, [17, 1, 5], 1));
        assert!(target(&unknown, [14.5, 0.0, 5.5], std::f32::consts::FRAC_PI_2, 0.0).is_none());
        unknown.apply_chunk([16, 0, 0], &vec![AIR; 4096]);
        assert_eq!(
            target(&unknown, [14.5, 0.0, 5.5], std::f32::consts::FRAC_PI_2, 0.0)
                .unwrap()
                .equipment,
            Some(1)
        );
    }
    #[test]
    fn pipe_has_passable_space_and_blocks_its_actual_shape() {
        let mut map = Grid::empty();
        map.equipment
            .insert(1, device(EquipmentKind::Pipe, [5, 1, 5], 1));
        assert!(!overlaps_grid([5.01, 1.0, 5.01], &map));
        assert!(overlaps_grid([5.5, 1.0, 5.5], &map));
        let mut body = Body {
            position: [4.5, 1.0, 5.5],
            ..Default::default()
        };
        for _ in 0..20 {
            step_player_in_grid(
                &mut body,
                PlayerInput {
                    strafe: 1.0,
                    ..Default::default()
                },
                true,
                &map,
                &[],
            );
        }
        assert!(body.position[0] < 5.087, "{body:?}");
        assert!(!overlaps_grid(body.position, &map));
        map.equipment
            .insert(1, device(EquipmentKind::Tank, [5, 1, 5], 0));
        assert!(!overlaps_grid([5.0, 1.0, 5.0], &map));
        assert!(overlaps_grid([6.0, 1.0, 6.0], &map));
    }
}

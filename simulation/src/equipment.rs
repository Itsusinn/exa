//! Shared device geometry, occupancy and ray selection for authority and prediction.
use crate::{grid::Grid, EYE_HEIGHT};
use protocol::{ColumnDesign, EquipmentKind, EquipmentSnapshot, TankSize, AIR, BLOCK_REACH, WATER};

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
    /// Stable within a device. A circuit groups material ports; different circuits
    /// never mix. Planning devices may have more than two ports in a circuit.
    pub id: u8,
    pub circuit: u8,
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
    placement_origin_sized(hit, kind, None)
}

pub fn placement_origin_sized(
    hit: WorldHit,
    kind: EquipmentKind,
    tank_size: Option<TankSize>,
) -> [i32; 3] {
    placement_origin_configured(hit, kind, tank_size, None)
}

pub fn placement_origin_configured(
    hit: WorldHit,
    kind: EquipmentKind,
    tank_size: Option<TankSize>,
    column_design: Option<&ColumnDesign>,
) -> [i32; 3] {
    let size = if matches!(kind, EquipmentKind::Tank | EquipmentKind::Vessel) {
        tank_size.unwrap_or_default().size()
    } else if kind.is_column() {
        column_design.map_or(kind.size(), ColumnDesign::size)
    } else {
        kind.size()
    };
    std::array::from_fn(|i| match hit.normal[i] {
        -1 => hit.position[i].saturating_sub(size[i]),
        1 => hit.position[i].saturating_add(1),
        _ => hit.position[i],
    })
}

pub fn bounds(device: &EquipmentSnapshot) -> CollisionBox {
    let min = device.origin.map(|v| v as f32);
    let size = device.size();
    CollisionBox {
        min,
        max: std::array::from_fn(|i| min[i] + size[i] as f32),
    }
}

pub fn occupied_cells(device: &EquipmentSnapshot) -> Vec<[i32; 3]> {
    let size = device.size();
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

/// The retained cylindrical model excludes the top 0.2 m from fluid capacity.
pub fn tank_fill_height_m(device: &EquipmentSnapshot) -> f64 {
    f64::from(device.tank_size_or_default().height) - 0.2
}

pub fn tank_area_m2(device: &EquipmentSnapshot) -> f64 {
    let radius = f64::from(device.tank_size_or_default().diameter) / 2.0 - 0.1;
    std::f64::consts::PI * radius * radius
}

pub fn tank_capacity_m3(device: &EquipmentSnapshot) -> f64 {
    tank_area_m2(device) * tank_fill_height_m(device)
}

/// Fixed lumped mass centre used by the existing sealed-vessel energy model.
pub fn tank_centroid_height_m(device: &EquipmentSnapshot) -> f64 {
    tank_fill_height_m(device) / 2.0
}

/// The nearest cell-centred row to the axis, retaining local z=0.5 for old tanks.
pub fn tank_nozzle_z_m(device: &EquipmentSnapshot) -> f32 {
    ((device.size()[0] - 1).max(0) / 2) as f32 + 0.5
}

/// Neck length reaches inside the cylindrical wall; the original neck is 0.4 m.
pub fn tank_nozzle_length_m(device: &EquipmentSnapshot) -> f32 {
    let centre = device.size()[0] as f32 / 2.0;
    let radius = centre - 0.1;
    let offset = tank_nozzle_z_m(device) - centre;
    (centre - (radius * radius - offset * offset).max(0.0).sqrt() + 0.1).max(0.4)
}

/// Face order is part of the protocol: -X, +X, -Y, +Y, -Z, +Z.
pub fn face_normal(face: u8) -> Option<[i32; 3]> {
    if face >= 6 {
        return None;
    }
    let mut normal = [0; 3];
    normal[usize::from(face / 2)] = if face.is_multiple_of(2) { -1 } else { 1 };
    Some(normal)
}

/// A pipe's positive reading is from its first face to its second face.
/// Old pipes retain their axis-aligned pair when no explicit faces were saved.
pub fn pipe_faces(device: &EquipmentSnapshot) -> [u8; 2] {
    device
        .pipe_ends
        .filter(|ends| ends[0] < ends[1] && ends[1] < 6)
        .unwrap_or([device.axis.min(2) * 2, device.axis.min(2) * 2 + 1])
}

/// One independent rack frame may share its cell with one horizontal pipe.
/// The frame ends at the lower tangent of the pipe (local y = 0.375).
pub fn rack_compatible(a: &EquipmentSnapshot, b: &EquipmentSnapshot) -> bool {
    let pipe = match (a.kind, b.kind) {
        (EquipmentKind::PipeRack, EquipmentKind::Pipe) => b,
        (EquipmentKind::Pipe, EquipmentKind::PipeRack) => a,
        _ => return false,
    };
    a.origin == b.origin && pipe_faces(pipe).iter().all(|face| *face != 2 && *face != 3)
}

/// Twelve T fittings: one opposing main pair and one of four perpendicular faces.
/// Port order is the main negative end, main positive end, then the branch.
pub fn tee_faces(axis: u8) -> [u8; 3] {
    let main = (axis / 4).min(2);
    let branch = (0..6)
        .filter(|face| face / 2 != main)
        .nth(usize::from(axis % 4))
        .expect("four perpendicular faces");
    [main * 2, main * 2 + 1, branch]
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
        EquipmentKind::Pipe | EquipmentKind::Tee => (if device.kind == EquipmentKind::Tee {
            tee_faces(device.axis).to_vec()
        } else {
            pipe_faces(device).to_vec()
        })
        .into_iter()
        .map(|face| {
            let mut min = [0.375; 3];
            let mut max = [0.625; 3];
            let axis = usize::from(face / 2);
            if face % 2 == 0 {
                min[axis] = 0.0;
            } else {
                max[axis] = 1.0;
            }
            translated(min, max)
        })
        .collect(),
        EquipmentKind::Pump | EquipmentKind::Valve => {
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
        EquipmentKind::PipeRack => {
            let shapes = [
                ([0.05, 0.0, 0.4], [0.15, 0.25, 0.6]),
                ([0.85, 0.0, 0.4], [0.95, 0.25, 0.6]),
                ([0.05, 0.25, 0.375], [0.95, 0.375, 0.625]),
            ];
            shapes
                .into_iter()
                .map(|(mut min, mut max)| {
                    if device.axis == 2 {
                        min.swap(0, 2);
                        max.swap(0, 2);
                    }
                    translated(min, max)
                })
                .collect()
        }
        EquipmentKind::Tank
        | EquipmentKind::Vessel
        | EquipmentKind::Column
        | EquipmentKind::RefluxColumn => {
            let mut boxes = Vec::with_capacity(34);
            let diameter = device.size()[0] as f32;
            let height = device.size()[1] as f32;
            let centre = diameter / 2.0;
            let shell_radius = if device.kind == EquipmentKind::Vessel || device.kind.is_column() {
                centre - 0.075
            } else {
                centre - 0.1
            };
            for band in 0..32 {
                let z0 = centre - shell_radius + band as f32 * (2.0 * shell_radius / 32.0);
                let z1 = z0 + 2.0 * shell_radius / 32.0;
                let nearest = if z0 <= centre && z1 >= centre {
                    0.0
                } else {
                    (z0 - centre).abs().min((z1 - centre).abs())
                };
                let radius = (shell_radius * shell_radius - nearest * nearest)
                    .max(0.0)
                    .sqrt();
                boxes.push(translated(
                    [centre - radius, 0.0, z0],
                    [centre + radius, height - 0.15, z1],
                ));
            }
            let nozzle_z = tank_nozzle_z_m(device);
            let neck = tank_nozzle_length_m(device);
            if device.kind.is_column() {
                for port in ports(device) {
                    boxes.push(port_neck_box(port, neck));
                }
                return boxes;
            }
            boxes.push(translated(
                [0.0, 0.375, nozzle_z - 0.125],
                [neck, 0.625, nozzle_z + 0.125],
            ));
            boxes.push(translated(
                [diameter - neck, 0.375, nozzle_z - 0.125],
                [diameter, 0.625, nozzle_z + 0.125],
            ));
            if device.kind == EquipmentKind::Vessel {
                boxes.push(translated(
                    [centre - 0.22, height - 0.15, centre - 0.22],
                    [centre + 0.22, height, centre + 0.22],
                ));
            }
            boxes
        }
        EquipmentKind::HeatExchanger => {
            let mut shapes = vec![translated([0.2, 0.15, 0.2], [1.8, 0.85, 2.8])];
            for z in [0.5, 2.5] {
                shapes.push(translated([0.0, 0.375, z - 0.125], [2.0, 0.625, z + 0.125]));
            }
            shapes
        }
        EquipmentKind::Condenser | EquipmentKind::Reboiler => {
            let mut shapes = vec![translated([0.2, 0.15, 0.2], [2.8, 1.85, 2.8])];
            shapes.extend(ports(device).into_iter().map(|p| port_neck_box(p, 0.3)));
            shapes
        }
    }
}

fn port_neck_box(port: EquipmentPort, length: f32) -> CollisionBox {
    let mut min = port.position.map(|p| p - 0.125);
    let mut max = port.position.map(|p| p + 0.125);
    let axis = port.normal.iter().position(|n| *n != 0).unwrap();
    if port.normal[axis] < 0 {
        min[axis] = port.position[axis];
        max[axis] = min[axis] + length;
    } else {
        max[axis] = port.position[axis];
        min[axis] = max[axis] - length;
    }
    CollisionBox { min, max }
}

pub fn ports(device: &EquipmentSnapshot) -> Vec<EquipmentPort> {
    let origin = device.origin.map(|v| v as f32);
    if device.kind.is_column() {
        let Some(design) = device.column_design.as_ref().filter(|d| d.valid()) else {
            return vec![];
        };
        let n = f32::from(design.spec.trays);
        let mut result: Vec<_> = [n + 1.5 - f32::from(design.spec.feed_tray), n + 1.5, 0.5]
            .into_iter()
            .enumerate()
            .map(|(id, y)| EquipmentPort {
                id: id as u8,
                circuit: id as u8,
                position: [
                    origin[0]
                        + if id == 0 {
                            0.0
                        } else {
                            f32::from(design.diameter)
                        },
                    origin[1] + y,
                    origin[2] + tank_nozzle_z_m(device),
                ],
                normal: [if id == 0 { -1 } else { 1 }, 0, 0],
            })
            .collect();
        if device.kind == EquipmentKind::RefluxColumn {
            for (id, y, sign) in [(3, n + 0.5, 1), (4, 1.5, -1)] {
                result.push(EquipmentPort {
                    id,
                    circuit: id,
                    position: [
                        origin[0] + tank_nozzle_z_m(device),
                        origin[1] + y,
                        origin[2]
                            + if sign > 0 {
                                f32::from(design.diameter)
                            } else {
                                0.0
                            },
                    ],
                    normal: [0, 0, sign],
                });
            }
        }
        return result;
    }
    if matches!(
        device.kind,
        EquipmentKind::Condenser | EquipmentKind::Reboiler
    ) {
        let mut definitions = if device.kind == EquipmentKind::Condenser {
            vec![
                (0, [0.0, 1.5, 0.5], [-1, 0, 0]),
                (0, [3.0, 1.5, 0.5], [1, 0, 0]),
            ]
        } else {
            vec![
                (0, [0.0, 0.5, 0.5], [-1, 0, 0]),
                (0, [3.0, 0.5, 0.5], [1, 0, 0]),
                (0, [2.5, 2.0, 0.5], [0, 1, 0]),
            ]
        };
        definitions.extend([
            (1, [0.0, 0.5, 2.5], [-1, 0, 0]),
            (1, [3.0, 0.5, 2.5], [1, 0, 0]),
        ]);
        return definitions
            .into_iter()
            .enumerate()
            .map(|(id, (circuit, p, normal))| EquipmentPort {
                id: id as u8,
                circuit,
                position: std::array::from_fn(|i| origin[i] + p[i]),
                normal,
            })
            .collect();
    }
    if device.kind == EquipmentKind::PipeRack {
        return vec![];
    }
    if matches!(device.kind, EquipmentKind::Pipe | EquipmentKind::Tee) {
        let faces = if device.kind == EquipmentKind::Tee {
            tee_faces(device.axis).to_vec()
        } else {
            pipe_faces(device).to_vec()
        };
        return faces
            .into_iter()
            .enumerate()
            .map(|(id, face)| {
                let normal = face_normal(face).expect("pipe faces are bounded");
                let position = std::array::from_fn(|i| origin[i] + 0.5 + normal[i] as f32 * 0.5);
                EquipmentPort {
                    id: id as u8,
                    circuit: 0,
                    position,
                    normal,
                }
            })
            .collect();
    }
    let axis = if matches!(
        device.kind,
        EquipmentKind::Tank | EquipmentKind::Vessel | EquipmentKind::HeatExchanger
    ) {
        0
    } else {
        device.axis.min(2) as usize
    };
    let length = device.size()[axis] as f32;
    let circuits = if device.kind == EquipmentKind::HeatExchanger {
        2
    } else {
        1
    };
    (0..circuits)
        .flat_map(|circuit| {
            [-1, 1].into_iter().map(move |sign| {
                let mut position = origin.map(|v| v + 0.5);
                if circuits == 2 {
                    position[2] += f32::from(circuit) * 2.0;
                } else if matches!(device.kind, EquipmentKind::Tank | EquipmentKind::Vessel) {
                    position[2] = origin[2] + tank_nozzle_z_m(device);
                }
                position[axis] = origin[axis] + if sign < 0 { 0.0 } else { length };
                let mut normal = [0; 3];
                normal[axis] = sign;
                EquipmentPort {
                    id: circuit * 2 + u8::from(sign > 0),
                    circuit,
                    position,
                    normal,
                }
            })
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
                    let size = device.size();
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
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            thermal: (kind == EquipmentKind::HeatExchanger).then(Default::default),
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
    fn reflux_devices_expose_distinct_grid_ports_inside_shared_geometry() {
        for kind in [
            EquipmentKind::RefluxColumn,
            EquipmentKind::Condenser,
            EquipmentKind::Reboiler,
            EquipmentKind::Tee,
        ] {
            for axis in 0..kind.axis_count() {
                for diameter in [2, 6] {
                    let mut d = device(kind, [-17, 16, -17], axis);
                    d.column_design = kind.is_column().then(|| ColumnDesign {
                        diameter,
                        ..Default::default()
                    });
                    assert!(d.valid());
                    let ps = ports(&d);
                    if kind == EquipmentKind::RefluxColumn {
                        let mut legacy = d.clone();
                        legacy.kind = EquipmentKind::Column;
                        assert_eq!(&ps[..3], ports(&legacy).as_slice());
                    }
                    assert_eq!(
                        ps.len(),
                        match kind {
                            EquipmentKind::RefluxColumn | EquipmentKind::Reboiler => 5,
                            EquipmentKind::Condenser => 4,
                            _ => 3,
                        }
                    );
                    let boxes = collision_boxes(&d);
                    let envelope = bounds(&d);
                    for b in &boxes {
                        assert!((0..3)
                            .all(|i| b.min[i] >= envelope.min[i] && b.max[i] <= envelope.max[i]));
                    }
                    let mut grid = Grid::unknown();
                    grid.equipment.insert(d.id, d.clone());
                    for (index, p) in ps.iter().enumerate() {
                        assert_eq!(p.id as usize, index);
                        assert!(ps[..index].iter().all(|other| other.position != p.position));
                        let along = p.normal.iter().position(|n| *n != 0).unwrap();
                        for i in 0..3 {
                            assert_eq!(
                                p.position[i].fract().abs(),
                                if i == along { 0.0 } else { 0.5 }
                            );
                        }
                        let inside =
                            std::array::from_fn(|i| p.position[i] - p.normal[i] as f32 * 0.001);
                        assert!(
                            boxes.iter().any(|b| b.contains(inside)),
                            "{kind:?} {axis} {p:?}"
                        );
                        let anchor = protocol::PipeAnchor {
                            id: d.id,
                            revision: d.revision,
                            port: p.id,
                        };
                        let cell = crate::pipe_route::anchor_cell(&grid, anchor).unwrap();
                        let neighbour = device(EquipmentKind::Pipe, cell, along as u8);
                        assert!(ports(&neighbour)
                            .into_iter()
                            .any(|other| connected(*p, other)));
                    }
                    assert!(crate::pipe_route::anchor_cell(
                        &grid,
                        protocol::PipeAnchor {
                            id: d.id,
                            revision: d.revision,
                            port: ps.len() as u8
                        }
                    )
                    .is_err());
                }
            }
        }
        let arrangements: std::collections::BTreeSet<_> = (0..12)
            .map(|axis| {
                let mut faces = tee_faces(axis);
                faces.sort_unstable();
                faces
            })
            .collect();
        assert_eq!(arrangements.len(), 12);
    }

    #[test]
    fn columns_have_three_separate_grid_ports_and_bounded_collision_for_every_size() {
        for diameter in 2..=6 {
            for trays in [2, 12] {
                for feed_tray in [1, trays] {
                    let mut d = device(EquipmentKind::Column, [-17, 16, -17], 0);
                    let mut design = ColumnDesign {
                        diameter,
                        ..Default::default()
                    };
                    design.spec.trays = trays;
                    design.spec.feed_tray = feed_tray;
                    d.column_design = Some(design.clone());
                    assert!(d.valid());
                    let ps = ports(&d);
                    assert_eq!(ps.len(), 3);
                    assert_eq!(
                        ps.iter().map(|p| p.circuit).collect::<Vec<_>>(),
                        vec![0, 1, 2]
                    );
                    assert_eq!(ps[0].normal, [-1, 0, 0]);
                    assert_eq!(ps[1].normal, [1, 0, 0]);
                    assert_eq!(ps[2].normal, [1, 0, 0]);
                    assert!(ps[1].position[1] > ps[0].position[1]);
                    assert!(ps[0].position[1] > ps[2].position[1]);
                    let b = bounds(&d);
                    let boxes = collision_boxes(&d);
                    for shape in &boxes {
                        assert!(
                            (0..3).all(|i| shape.min[i] >= b.min[i] && shape.max[i] <= b.max[i])
                        );
                    }
                    for p in &ps {
                        assert_eq!(p.position[1].fract(), 0.5);
                        assert_eq!(p.position[2].fract().abs(), 0.5);
                        // Ports lie on the outside face. Occupancy is half-open,
                        // so the +X face itself is not an interior point. Check
                        // actual aiming from outside as well as solid interior.
                        let inward =
                            std::array::from_fn(|i| p.position[i] - p.normal[i] as f32 * 0.001);
                        assert!(boxes.iter().any(|shape| shape.contains(inward)));
                        let eye = std::array::from_fn(|i| p.position[i] + p.normal[i] as f32);
                        let direction = p.normal.map(|v| -(v as f32));
                        assert!(boxes.iter().any(|shape| ray_box(eye, direction, *shape)
                            .is_some_and(|(distance, normal)| (distance - 1.0).abs() < 1e-5
                                && normal == p.normal)));
                    }
                    assert_eq!(
                        occupied_cells(&d).len(),
                        usize::from(diameter).pow(2) * (usize::from(trays) + 2)
                    );
                    for face in 0..6 {
                        let normal = face_normal(face).unwrap();
                        let hit = WorldHit {
                            position: [-1, 30, -1],
                            normal,
                            equipment: None,
                            distance: 1.0,
                        };
                        let placed = placement_origin_configured(
                            hit,
                            EquipmentKind::Column,
                            None,
                            Some(&design),
                        );
                        let a = usize::from(face / 2);
                        assert_eq!(
                            placed[a],
                            if face % 2 == 0 {
                                hit.position[a] - design.size()[a]
                            } else {
                                hit.position[a] + 1
                            }
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn parametric_tanks_share_bounded_geometry_and_grid_aligned_nozzles() {
        for kind in [EquipmentKind::Tank, EquipmentKind::Vessel] {
            let legacy = device(kind, [-17, 1, -17], 0);
            let mut explicit = legacy.clone();
            explicit.tank_size = Some(TankSize::default());
            assert_eq!(collision_boxes(&legacy), collision_boxes(&explicit));
            assert_eq!(ports(&legacy), ports(&explicit));
            assert_eq!(
                tank_capacity_m3(&legacy),
                std::f64::consts::PI * 0.9 * 0.9 * 2.8
            );
            assert_eq!(tank_centroid_height_m(&legacy), 1.4);
            for diameter in 2..=6 {
                for height in 3..=12 {
                    explicit.tank_size = Some(TankSize { diameter, height });
                    let extent = bounds(&explicit);
                    assert_eq!(
                        occupied_cells(&explicit).len(),
                        usize::from(diameter).pow(2) * usize::from(height)
                    );
                    for shape in collision_boxes(&explicit) {
                        assert!((0..3).all(
                            |i| shape.min[i] >= extent.min[i] && shape.max[i] <= extent.max[i]
                        ));
                    }
                    for port in ports(&explicit) {
                        let cell = [
                            if port.id == 0 {
                                -18
                            } else {
                                -17 + i32::from(diameter)
                            },
                            1,
                            -17 + i32::from((diameter - 1) / 2),
                        ];
                        let pipe = device(EquipmentKind::Pipe, cell, 0);
                        assert!(connected(port, ports(&pipe)[usize::from(1 - port.id)]));
                        let mut inside = port.position;
                        inside[0] -= port.normal[0] as f32 * 0.01;
                        assert!(collision_boxes(&explicit)
                            .iter()
                            .any(|shape| shape.contains(inside)));
                    }
                }
            }
        }
        let hit = WorldHit {
            position: [-1, 7, -1],
            normal: [-1, 0, 0],
            equipment: None,
            distance: 1.0,
        };
        assert_eq!(
            placement_origin_sized(
                hit,
                EquipmentKind::Tank,
                Some(TankSize {
                    diameter: 6,
                    height: 12
                })
            ),
            [-7, 7, -1]
        );
    }
    #[test]
    fn exchanger_ports_keep_two_independent_channels_and_full_footprint() {
        let exchanger = device(EquipmentKind::HeatExchanger, [-1, 5, -1], 0);
        assert!(exchanger.valid());
        assert_eq!(occupied_cells(&exchanger).len(), 6);
        let p = ports(&exchanger);
        assert_eq!(p.len(), 4);
        for circuit in 0..2 {
            for end in 0..2 {
                let port = p[circuit * 2 + end];
                assert_eq!(usize::from(port.id), circuit * 2 + end);
                assert_eq!(usize::from(port.circuit), circuit);
                let pipe = device(
                    EquipmentKind::Pipe,
                    [if end == 0 { -2 } else { 1 }, 5, -1 + circuit as i32 * 2],
                    0,
                );
                assert!(connected(port, ports(&pipe)[1 - end]));
                assert!(collision_boxes(&exchanger).iter().any(|bounds| {
                    let mut inside = port.position;
                    inside[0] -= port.normal[0] as f32 * 0.01;
                    bounds.contains(inside)
                }));
            }
        }
        let vessel = device(EquipmentKind::Vessel, [-4, 5, -1], 0);
        assert!(vessel.valid());
        assert_eq!(ports(&vessel).len(), 2);
        assert_eq!(occupied_cells(&vessel).len(), 12);
        assert!(connected(
            ports(&vessel)[1],
            ports(&device(EquipmentKind::Pipe, [-2, 5, -1], 0))[0]
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
    fn all_twelve_elbows_and_three_straights_expose_only_their_saved_faces() {
        let mut elbows = 0;
        let mut straights = 0;
        for first in 0..6 {
            for second in first + 1..6 {
                let mut pipe = device(EquipmentKind::Pipe, [-1, 6, -1], 0);
                pipe.pipe_ends = Some([first, second]);
                assert!(pipe.valid());
                let p = ports(&pipe);
                assert_eq!(p.len(), 2);
                for (id, face) in [first, second].into_iter().enumerate() {
                    let normal = face_normal(face).unwrap();
                    assert_eq!(p[id].normal, normal);
                    assert_eq!(usize::from(p[id].id), id);
                    let mut neighbour = device(
                        EquipmentKind::Pipe,
                        std::array::from_fn(|i| pipe.origin[i] + normal[i]),
                        face / 2,
                    );
                    neighbour.id = 2;
                    assert_eq!(
                        ports(&neighbour)
                            .into_iter()
                            .filter(|other| connected(p[id], *other))
                            .count(),
                        1
                    );
                }
                let geometry = collision_boxes(&pipe);
                assert!(geometry
                    .iter()
                    .any(|bounds| bounds.contains([-0.5, 6.5, -0.5])));
                assert!(!geometry
                    .iter()
                    .any(|bounds| bounds.contains([-0.95, 6.05, -0.95])));
                for unused in (0..6).filter(|face| *face != first && *face != second) {
                    let normal = face_normal(unused).unwrap();
                    let neighbour = device(
                        EquipmentKind::Pipe,
                        std::array::from_fn(|i| pipe.origin[i] + normal[i]),
                        unused / 2,
                    );
                    assert!(!p.iter().any(|port| ports(&neighbour)
                        .into_iter()
                        .any(|other| connected(*port, other))));
                }
                if first ^ 1 == second {
                    straights += 1;
                } else {
                    elbows += 1;
                }
            }
        }
        assert_eq!((elbows, straights), (12, 3));
    }

    #[test]
    fn placement_keeps_tanks_and_pipes_adjacent_on_all_six_faces() {
        for base_kind in [
            EquipmentKind::Tank,
            EquipmentKind::Pipe,
            EquipmentKind::Vessel,
            EquipmentKind::HeatExchanger,
        ] {
            let base = device(base_kind, [-1, 10, -1], 0);
            let occupied = occupied_cells(&base);
            for kind in [
                EquipmentKind::Tank,
                EquipmentKind::Pipe,
                EquipmentKind::Vessel,
                EquipmentKind::HeatExchanger,
            ] {
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

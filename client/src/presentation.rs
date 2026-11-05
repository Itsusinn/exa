use crate::mesh::{Mesh, Vertex, NORMALS};
use exa_client_core::voxel::Client3d;
use exa_protocol::*;
pub use exa_simulation::equipment::ports as equipment_ports;
pub use exa_simulation::equipment::tank_capacity_m3;
pub use exa_simulation::equipment::{
    collision_boxes as equipment_collision_boxes, tank_nozzle_length_m,
};

/// Reusable local-space equipment mesh; instances carry their authoritative origin.
pub fn equipment_shape(kind: EquipmentKind, axis: u8) -> Mesh {
    equipment_shape_sized(kind, axis, None)
}

pub fn tank_capacity_for_size(size: TankSize) -> f64 {
    tank_capacity_m3(&EquipmentSnapshot {
        id: 1,
        revision: 0,
        kind: EquipmentKind::Tank,
        origin: [0; 3],
        axis: 0,
        tank_size: Some(size),
        column_design: None,
        pipe_ends: None,
        contents: vec![],
        internal_energy_j: 0.0,
        enabled: false,
        opening: 1.0,
        pump_reversed: false,
        thermal: None,
        reading: None,
    })
}

pub fn equipment_shape_sized(kind: EquipmentKind, axis: u8, size: Option<TankSize>) -> Mesh {
    equipment_shape_configured(kind, axis, size, None)
}

pub fn equipment_shape_configured(
    kind: EquipmentKind,
    axis: u8,
    size: Option<TankSize>,
    column_design: Option<&ColumnDesign>,
) -> Mesh {
    let mut mesh = Mesh::default();
    let steel = [0.57, 0.62, 0.64];
    let dark = [0.27, 0.31, 0.33];
    let size = size.unwrap_or_default();
    let default_column = ColumnDesign::default();
    let column = column_design.unwrap_or(&default_column);
    let diameter = if kind.is_column() {
        f32::from(column.diameter)
    } else {
        f32::from(size.diameter)
    };
    let height = if kind.is_column() {
        column.size()[1] as f32
    } else {
        f32::from(size.height)
    };
    let template = EquipmentSnapshot {
        id: 1,
        revision: 0,
        kind,
        origin: [0; 3],
        axis,
        tank_size: matches!(kind, EquipmentKind::Tank | EquipmentKind::Vessel).then_some(size),
        column_design: kind.is_column().then(|| column.clone()),
        pipe_ends: None,
        contents: vec![],
        internal_energy_j: 0.0,
        enabled: false,
        opening: 1.0,
        pump_reversed: false,
        thermal: None,
        reading: None,
    };
    let nozzle_z = exa_simulation::equipment::tank_nozzle_z_m(&template);
    let neck = exa_simulation::equipment::tank_nozzle_length_m(&template);
    match kind {
        EquipmentKind::Column | EquipmentKind::RefluxColumn => {
            let centre = diameter / 2.0;
            // Keep the bottom cap off the supporting voxel's top face, including
            // in translucent build previews. Collision remains conservative.
            cylinder(
                &mut mesh,
                [centre, 0.01, centre],
                1,
                height - 0.16,
                centre - 0.1,
                [0.42, 0.51, 0.58],
            );
            for tray in 1..=column.spec.trays {
                let y = f32::from(column.spec.trays) + 1.5 - f32::from(tray);
                let color = if tray == column.spec.feed_tray {
                    [0.65, 0.49, 0.24]
                } else {
                    dark
                };
                cylinder(
                    &mut mesh,
                    [centre, y - 0.05, centre],
                    1,
                    0.1,
                    centre - 0.075,
                    color,
                );
            }
            for port in equipment_ports(&template) {
                let color = match port.id {
                    0 => [0.65, 0.49, 0.24],
                    1 => [0.40, 0.60, 0.58],
                    3 => [0.24, 0.54, 0.72],
                    4 => [0.72, 0.34, 0.22],
                    _ => dark,
                };
                equipment_nozzle(&mut mesh, port, neck, color);
            }
        }
        EquipmentKind::Condenser | EquipmentKind::Reboiler => {
            let utility_color = if kind == EquipmentKind::Condenser {
                [0.24, 0.54, 0.72]
            } else {
                [0.72, 0.34, 0.22]
            };
            if kind == EquipmentKind::Condenser {
                mesh.cuboid([0.25, 0.2, 0.25], [2.5, 1.5, 2.5], steel, 0);
                // Repeated ribs distinguish the condenser's exchanger body.
                for x in [0.6, 1.0, 1.4, 1.8, 2.2] {
                    mesh.cuboid([x, 0.25, 0.2], [0.05, 1.45, 2.6], utility_color, 0);
                }
            } else {
                // A horizontal kettle on two saddles with a raised vapour outlet.
                cylinder(&mut mesh, [0.25, 1.0, 1.5], 0, 2.5, 0.75, steel);
                mesh.cuboid([0.25, 0.3, 0.25], [2.5, 0.4, 2.5], dark, 0);
                for x in [0.6, 2.1] {
                    mesh.cuboid([x, 0.15, 0.6], [0.25, 0.4, 1.8], utility_color, 0);
                }
                mesh.cuboid([2.3, 0.7, 0.3], [0.4, 1.1, 0.4], steel, 0);
            }
            for port in equipment_ports(&template) {
                let color = if port.circuit == 1 {
                    utility_color
                } else {
                    dark
                };
                equipment_nozzle(&mut mesh, port, 0.3, color);
            }
        }
        EquipmentKind::Tee => {
            for face in exa_simulation::equipment::tee_faces(axis) {
                let along = usize::from(face / 2);
                let mut origin = [0.5; 3];
                if face % 2 == 0 {
                    origin[along] = 0.0;
                }
                cylinder_section(
                    &mut mesh,
                    origin,
                    along,
                    0.5,
                    0.125,
                    steel,
                    [face % 2 == 0, face % 2 != 0],
                );
            }
        }
        EquipmentKind::Tank => {
            cylinder(
                &mut mesh,
                [diameter / 2.0, 0.0, diameter / 2.0],
                1,
                height - 0.15,
                diameter / 2.0 - 0.1,
                steel,
            );
            // Nozzles align with the shared port centres; all pieces stay inside occupied cells.
            cylinder(&mut mesh, [0.0, 0.5, nozzle_z], 0, neck, 0.125, dark);
            cylinder(
                &mut mesh,
                [diameter - neck, 0.5, nozzle_z],
                0,
                neck,
                0.125,
                dark,
            );
        }
        EquipmentKind::Vessel => {
            // Closed, banded pressure vessel. The bands and top neck distinguish
            // it from the legacy atmospheric tank without changing its ports.
            cylinder(
                &mut mesh,
                [diameter / 2.0, 0.0, diameter / 2.0],
                1,
                height - 0.15,
                diameter / 2.0 - 0.1,
                [0.42, 0.51, 0.58],
            );
            for y in [0.35, height - 0.65] {
                cylinder(
                    &mut mesh,
                    [diameter / 2.0, y, diameter / 2.0],
                    1,
                    0.12,
                    diameter / 2.0 - 0.075,
                    dark,
                );
            }
            cylinder(
                &mut mesh,
                [diameter / 2.0, height - 0.15, diameter / 2.0],
                1,
                0.15,
                0.22,
                steel,
            );
            cylinder(&mut mesh, [0.0, 0.5, nozzle_z], 0, neck, 0.125, dark);
            cylinder(
                &mut mesh,
                [diameter - neck, 0.5, nozzle_z],
                0,
                neck,
                0.125,
                dark,
            );
        }
        EquipmentKind::HeatExchanger => {
            // Two separate, colour-coded passages share a plate body. Their
            // four ends exactly match the shared geometry's A/B port centres.
            mesh.cuboid([0.25, 0.2, 0.25], [1.5, 0.6, 2.5], steel, 0);
            for (z, colour) in [(0.5, [0.7, 0.36, 0.2]), (2.5, [0.2, 0.48, 0.65])] {
                cylinder(&mut mesh, [0.0, 0.5, z], 0, 0.25, 0.125, colour);
                cylinder(&mut mesh, [1.75, 0.5, z], 0, 0.25, 0.125, colour);
                mesh.cuboid([0.3, 0.8, z - 0.07], [1.4, 0.025, 0.14], colour, 0);
            }
            for x in [0.5, 0.8, 1.1, 1.4] {
                mesh.cuboid([x, 0.25, 0.2], [0.06, 0.5, 2.6], dark, 0);
            }
        }
        EquipmentKind::PipeRack => {
            for (mut origin, mut size) in [
                ([0.05, 0.0, 0.4], [0.1, 0.25, 0.2]),
                ([0.85, 0.0, 0.4], [0.1, 0.25, 0.2]),
                ([0.05, 0.25, 0.375], [0.9, 0.125, 0.25]),
            ] {
                if axis == 2 {
                    origin.swap(0, 2);
                    size.swap(0, 2);
                }
                mesh.cuboid(origin, size, steel, 0);
            }
        }
        EquipmentKind::Pipe => {
            let axis = usize::from(axis.min(2));
            let mut origin = [0.5; 3];
            origin[axis] = 0.0;
            // Disjoint axial sections avoid coplanar colour-band overlays. Only
            // the two exterior ends are capped; internal seams have no faces.
            cylinder_section(&mut mesh, origin, axis, 0.04, 0.125, dark, [true, false]);
            origin[axis] = 0.04;
            cylinder_section(
                &mut mesh,
                origin,
                axis,
                0.96 - 0.04,
                0.125,
                steel,
                [false, false],
            );
            origin[axis] = 0.96;
            cylinder_section(&mut mesh, origin, axis, 0.04, 0.125, dark, [false, true]);
        }
        EquipmentKind::Pump | EquipmentKind::Valve => {
            let axis = usize::from(axis.min(2));
            let mut origin = [0.5; 3];
            origin[axis] = 0.0;
            cylinder(&mut mesh, origin, axis, 0.25, 0.125, dark);
            origin[axis] = 0.75;
            cylinder(&mut mesh, origin, axis, 0.25, 0.125, dark);
            origin[axis] = 0.25;
            let color = if kind == EquipmentKind::Pump {
                [0.32, 0.48, 0.42]
            } else {
                [0.55, 0.40, 0.24]
            };
            cylinder(&mut mesh, origin, axis, 0.5, 0.28, color);
            // A raised longitudinal ridge identifies the shaft direction without
            // coplanar decals; dynamic controls remain separate from this mesh.
            let side = (axis + 1) % 3;
            let mut position = [0.46; 3];
            let mut size = [0.08; 3];
            position[axis] = 0.36;
            position[side] = 0.74;
            size[side] = 0.06;
            size[axis] = 0.28;
            mesh.cuboid(position, size, steel, 0);
        }
    }
    mesh
}

pub fn equipment_shape_with_ends(kind: EquipmentKind, axis: u8, ends: Option<[u8; 2]>) -> Mesh {
    if kind != EquipmentKind::Pipe || ends.is_none() {
        return equipment_shape(kind, axis);
    }
    let ends = ends.unwrap();
    if ends[0] / 2 == ends[1] / 2 {
        return equipment_shape(kind, ends[0] / 2);
    }
    let mut mesh = Mesh::default();
    let steel = [0.57, 0.62, 0.64];
    let dark = [0.27, 0.31, 0.33];
    for face in ends {
        let axis = usize::from(face / 2);
        let mut origin = [0.5; 3];
        origin[axis] = if face % 2 == 0 { 0.04 } else { 0.5 };
        cylinder_section(&mut mesh, origin, axis, 0.46, 0.125, steel, [false, false]);
        origin[axis] = if face % 2 == 0 { 0.0 } else { 0.96 };
        cylinder_section(
            &mut mesh,
            origin,
            axis,
            0.04,
            0.125,
            dark,
            [face % 2 == 0, face % 2 == 1],
        );
    }
    // A compact coupling conceals the meeting half-cylinder seams without
    // extending outside the shared union of the two pipe arms.
    mesh.cuboid([0.38; 3], [0.24; 3], steel, 0);
    mesh
}

fn equipment_nozzle(
    mesh: &mut Mesh,
    port: exa_simulation::equipment::EquipmentPort,
    length: f32,
    color: [f32; 3],
) {
    let axis = port.normal.iter().position(|n| *n != 0).unwrap();
    let mut origin = port.position;
    if port.normal[axis] > 0 {
        origin[axis] -= length;
    }
    cylinder(mesh, origin, axis, length, 0.125, color);
}

fn cylinder(
    mesh: &mut Mesh,
    origin: [f32; 3],
    axis: usize,
    length: f32,
    radius: f32,
    color: [f32; 3],
) {
    cylinder_section(mesh, origin, axis, length, radius, color, [true, true]);
}

fn cylinder_section(
    mesh: &mut Mesh,
    origin: [f32; 3],
    axis: usize,
    length: f32,
    radius: f32,
    color: [f32; 3],
    caps: [bool; 2],
) {
    // Large tank silhouettes need more facets than narrow inline fittings.
    // Shading is smooth independently of tessellation; cap vertices stay separate.
    let segments = if radius > 0.5 { 96 } else { 32 };
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let point = |angle: f32, along: f32| {
        let mut p = origin;
        p[axis] += along;
        p[u] += angle.cos() * radius;
        p[v] += angle.sin() * radius;
        p
    };
    let radial = |angle: f32| {
        let mut normal = [0.0; 3];
        normal[u] = angle.cos();
        normal[v] = angle.sin();
        normal
    };
    for i in 0..segments {
        let a = i as f32 * std::f32::consts::TAU / segments as f32;
        let b = (i + 1) as f32 * std::f32::consts::TAU / segments as f32;
        let points = [
            point(a, 0.0),
            point(a, length),
            point(b, length),
            point(b, 0.0),
        ];
        let normals = [radial(a), radial(a), radial(b), radial(b)];
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        // Match Mesh::face winding, but carry the circular normal at each vertex.
        // Adjacent facets share shading normals while the separate caps stay flat.
        mesh.vertices.extend([0, 2, 1, 0, 3, 2].map(|index| Vertex {
            position: points[index],
            normal: normals[index],
            color,
            uv: uv[index],
            pattern: 0,
            occlusion: 1.0,
            face_occlusion: 0,
            bevel_mask: 0,
        }));
        for (end, (along, sign)) in [(0.0, -1.0), (length, 1.0)].into_iter().enumerate() {
            if !caps[end] {
                continue;
            }
            let mut centre = origin;
            centre[axis] += along;
            let mut normal = [0.0; 3];
            normal[axis] = sign;
            let (a, b) = if sign < 0.0 { (a, b) } else { (b, a) };
            mesh.face(
                [centre, point(a, along), point(b, along), centre],
                normal,
                color,
                0,
            );
        }
    }
}

pub fn terrain_chunk(model: &Client3d, origin: [i32; 3]) -> Mesh {
    let mut mesh = Mesh::default();
    let Some(blocks) = model.terrain.blocks.get(&origin) else {
        return mesh;
    };
    if blocks.iter().all(|&b| b == AIR) {
        return mesh;
    }
    // Cache the six section references once. The hot loop uses contiguous byte
    // indices instead of BTreeMap lookups and signed coordinate division per face.
    let neighbors = NORMALS.map(|n| {
        let adjacent = std::array::from_fn(|i| origin[i] + n[i] as i32 * CHUNK_EDGE);
        model.terrain.blocks.get(&adjacent)
    });
    // One-cell halo includes edge/corner sections for view-independent contact AO.
    // Unknown cells and water do not invent opaque occluders at streaming borders.
    let sections: [_; 27] = std::array::from_fn(|i| {
        let offset = [i % 3, i / 9, i / 3 % 3];
        model.terrain.blocks.get(&std::array::from_fn(|axis| {
            origin[axis] + (offset[axis] as i32 - 1) * CHUNK_EDGE
        }))
    });
    let mut halo = [false; 18 * 18 * 18];
    for y in 0..18 {
        for z in 0..18 {
            for x in 0..18 {
                let p = [x as i32 - 1, y as i32 - 1, z as i32 - 1];
                let section = p.map(|v| (v.div_euclid(16) + 1) as usize);
                let local = p.map(|v| v.rem_euclid(16) as usize);
                let b = sections[section[0] + 3 * (section[2] + 3 * section[1])]
                    .map_or(255, |blocks| {
                        blocks[local[0] + 16 * (local[2] + 16 * local[1])]
                    });
                halo[x + 18 * (z + 18 * y)] = !matches!(b, AIR | WATER | 255);
            }
        }
    }
    for ly in 0..16usize {
        for lz in 0..16usize {
            for lx in 0..16usize {
                let index = lx + 16 * (lz + 16 * ly);
                let block = blocks[index];
                if block == AIR || block == 255 {
                    continue;
                }
                let [x, y, z] = [
                    origin[0] + lx as i32,
                    origin[1] + ly as i32,
                    origin[2] + lz as i32,
                ];
                let cells = [
                    (lx == 0, index.wrapping_sub(1), index + 15),
                    (lx == 15, index + 1, index.wrapping_sub(15)),
                    (ly == 0, index.wrapping_sub(256), index + 15 * 256),
                    (ly == 15, index + 256, index.wrapping_sub(15 * 256)),
                    (lz == 0, index.wrapping_sub(16), index + 15 * 16),
                    (lz == 15, index + 16, index.wrapping_sub(15 * 16)),
                ];
                for (face, (edge, local, adjacent)) in cells.into_iter().enumerate() {
                    let neighbor = if edge {
                        neighbors[face].map_or(255, |b| b[adjacent])
                    } else {
                        blocks[local]
                    };
                    if neighbor != AIR && !(neighbor == WATER && block != WATER) && neighbor != 255
                    {
                        continue;
                    }
                    if y == 0 && face == 2 {
                        continue;
                    }
                    let (color, pattern) = match block {
                        GRASS if face == 3 => ([0.46, 0.62, 0.32], 3),
                        GRASS | DIRT => ([0.48, 0.36, 0.24], 3),
                        STONE => ([0.56, 0.58, 0.53], 2),
                        WOOD => ([0.43, 0.32, 0.21], 1),
                        LEAVES => ([0.33, 0.51, 0.27], 4),
                        WATER => ([0.29, 0.49, 0.58], 0),
                        PLANK => ([0.65, 0.51, 0.33], 1),
                        CHEST => ([0.62, 0.39, 0.18], if face == 5 { 6 } else { 5 }),
                        SAND => ([0.78, 0.70, 0.49], 7),
                        SANDSTONE => ([0.69, 0.56, 0.36], 8),
                        GRAVEL => ([0.48, 0.48, 0.44], 9),
                        CLAY => ([0.47, 0.43, 0.36], 7),
                        SNOW => ([0.90, 0.94, 0.96], 10),
                        ICE => ([0.53, 0.75, 0.84], 11),
                        MOSS_STONE => ([0.36, 0.45, 0.28], 8),
                        BASALT => ([0.24, 0.26, 0.28], 8),
                        BIRCH_WOOD => ([0.82, 0.81, 0.70], 12),
                        BIRCH_LEAVES => ([0.52, 0.63, 0.28], 13),
                        PINE_WOOD => ([0.34, 0.24, 0.17], 12),
                        PINE_LEAVES => ([0.19, 0.34, 0.26], 13),
                        COAL_ORE => ([0.17, 0.18, 0.19], 14),
                        IRON_ORE => ([0.65, 0.43, 0.30], 14),
                        COPPER_ORE => ([0.41, 0.64, 0.48], 14),
                        _ => ([0.5, 0.5, 0.5], 0),
                    };
                    mesh.block_face(
                        [x as f32, y as f32, z as f32],
                        [1.0; 3],
                        face,
                        color,
                        pattern,
                    );
                    let start = mesh.vertices.len() - 6;
                    for vertex in &mut mesh.vertices[start..] {
                        let n = NORMALS[face].map(|v| v as i32);
                        let axes = match face / 2 {
                            0 => [1, 2],
                            1 => [0, 2],
                            _ => [0, 1],
                        };
                        let base = [
                            lx as i32 + 1 + n[0],
                            ly as i32 + 1 + n[1],
                            lz as i32 + 1 + n[2],
                        ];
                        let mut a = base;
                        let mut b = base;
                        let cell = [x, y, z];
                        a[axes[0]] += if vertex.position[axes[0]] > cell[axes[0]] as f32 {
                            1
                        } else {
                            -1
                        };
                        b[axes[1]] += if vertex.position[axes[1]] > cell[axes[1]] as f32 {
                            1
                        } else {
                            -1
                        };
                        let diagonal = std::array::from_fn(|i| a[i] + b[i] - base[i]);
                        let occupied = |p: [i32; 3]| {
                            halo[p[0] as usize + 18 * (p[2] as usize + 18 * p[1] as usize)]
                        };
                        vertex.occlusion =
                            corner_occlusion(occupied(a), occupied(b), occupied(diagonal));
                    }
                    let corners = [0, 2, 1, 4].map(|i| mesh.vertices[start + i].occlusion);
                    let packed = corners.into_iter().enumerate().fold(0u8, |bits, (i, ao)| {
                        bits | (((1.0 - ao) / 0.18).round() as u8).min(3) << (2 * i)
                    });
                    let mut bevel_mask = 0;
                    if block != WATER {
                        for (edge, (a, b)) in
                            [(0, 2), (2, 1), (1, 4), (4, 0)].into_iter().enumerate()
                        {
                            let p = mesh.vertices[start + a].position;
                            let q = mesh.vertices[start + b].position;
                            let cell = [x, y, z];
                            let local = [lx, ly, lz];
                            let adjacent: [usize; 3] = std::array::from_fn(|axis| {
                                let step = if NORMALS[face][axis] == 0.0 {
                                    (p[axis] + q[axis] - 2.0 * cell[axis] as f32 - 1.0) as i32
                                } else {
                                    0
                                };
                                (local[axis] as i32 + 1 + step) as usize
                            });
                            if !halo[adjacent[0] + 18 * (adjacent[2] + 18 * adjacent[1])] {
                                bevel_mask |= 1 << edge;
                            }
                        }
                    }
                    for vertex in &mut mesh.vertices[start..] {
                        vertex.face_occlusion = packed;
                        vertex.bevel_mask = bevel_mask;
                    }
                }
            }
        }
    }
    mesh
}

fn corner_occlusion(a: bool, b: bool, diagonal: bool) -> f32 {
    let count = if a && b {
        3
    } else {
        u8::from(a) + u8::from(b) + u8::from(diagonal)
    };
    1.0 - f32::from(count) * 0.18
}

pub fn actors(model: &Client3d) -> Mesh {
    let mut mesh = Mesh::default();
    for a in &model.actors {
        if a.id == model.player_id {
            continue;
        }
        let start = mesh.vertices.len();
        let skin = if a.kind == 0 {
            [0.78, 0.63, 0.47]
        } else {
            [0.45, 0.57, 0.33]
        };
        let shirt = if a.kind == 0 {
            [0.35, 0.57, 0.59]
        } else {
            [0.46, 0.35, 0.30]
        };
        mesh.cuboid([-0.22, 1.30, -0.22], [0.44, 0.44, 0.44], skin, 0);
        mesh.cuboid([-0.23, 0.62, -0.16], [0.46, 0.68, 0.32], shirt, 0);
        for x in [-0.23, 0.04] {
            mesh.cuboid([x, 0.0, -0.15], [0.19, 0.62, 0.30], [0.23, 0.29, 0.33], 0);
        }
        for x in [-0.40, 0.24] {
            mesh.cuboid([x, 0.68, -0.14], [0.16, 0.58, 0.28], shirt, 0);
            mesh.cuboid([x, 0.56, -0.14], [0.16, 0.16, 0.28], skin, 0);
        }
        mesh.cuboid(
            [-0.23, 1.65, -0.23],
            [0.46, 0.12, 0.46],
            [0.22, 0.21, 0.18],
            0,
        );
        for x in [-0.14, 0.07] {
            mesh.cuboid([x, 1.49, -0.231], [0.07, 0.06, 0.02], [0.10, 0.12, 0.10], 0);
        }
        for v in &mut mesh.vertices[start..] {
            let (s, c) = (-a.yaw).sin_cos();
            let p = v.position;
            let n = v.normal;
            v.position = [
                a.position[0] + p[0] * c + p[2] * s,
                a.position[1] + p[1],
                a.position[2] - p[0] * s + p[2] * c,
            ];
            v.normal = [n[0] * c + n[2] * s, n[1], -n[0] * s + n[2] * c];
        }
    }
    for item in &model.items {
        let p = item.position;
        mesh.cuboid(
            [p[0] - 0.18, p[1] + 0.04, p[2] - 0.15],
            [0.36, 0.40, 0.30],
            [0.74, 0.59, 0.32],
            1,
        );
        mesh.cuboid(
            [p[0] - 0.19, p[1] + 0.31, p[2] - 0.16],
            [0.38, 0.06, 0.32],
            [0.40, 0.32, 0.22],
            0,
        );
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use exa_simulation::grid::Grid;

    #[test]
    fn sized_tanks_match_shared_collision_and_keep_separate_meshes() {
        for kind in [EquipmentKind::Tank, EquipmentKind::Vessel] {
            for diameter in [2, 3, 6] {
                for height in [3, 6, 12] {
                    let size = TankSize { diameter, height };
                    let device = EquipmentSnapshot {
                        id: 1,
                        revision: 0,
                        kind,
                        origin: [0; 3],
                        axis: 0,
                        tank_size: Some(size),
                        column_design: None,
                        pipe_ends: None,
                        contents: vec![],
                        internal_energy_j: 0.0,
                        enabled: false,
                        opening: 1.0,
                        pump_reversed: false,
                        thermal: None,
                        reading: None,
                    };
                    let mesh = equipment_shape_sized(kind, 0, Some(size));
                    let boxes = exa_simulation::equipment::collision_boxes(&device);
                    assert!(!mesh.vertices.is_empty());
                    for vertex in &mesh.vertices {
                        assert!(
                            boxes.iter().any(|shape| (0..3)
                                .all(|i| vertex.position[i] >= shape.min[i] - 1e-5
                                    && vertex.position[i] <= shape.max[i] + 1e-5)),
                            "{kind:?} {size:?} {:?}",
                            vertex.position
                        );
                    }
                    let max_y = mesh
                        .vertices
                        .iter()
                        .map(|v| v.position[1])
                        .fold(0.0, f32::max);
                    assert!(
                        (max_y
                            - (f32::from(height)
                                - if kind == EquipmentKind::Tank {
                                    0.15
                                } else {
                                    0.0
                                }))
                        .abs()
                            < 1e-5
                    );
                    for port in equipment_ports(&device) {
                        assert!(mesh
                            .vertices
                            .iter()
                            .any(|v| (v.position[0] - port.position[0]).abs() < 1e-5
                                && (v.position[2] - port.position[2]).abs() <= 0.126));
                    }
                }
            }
        }
    }

    #[test]
    fn equipment_meshes_stay_inside_shared_collision_envelopes() {
        for kind in [
            EquipmentKind::Pump,
            EquipmentKind::Valve,
            EquipmentKind::Vessel,
            EquipmentKind::HeatExchanger,
            EquipmentKind::PipeRack,
        ] {
            let axes = if matches!(kind, EquipmentKind::Vessel | EquipmentKind::HeatExchanger) {
                1
            } else {
                3
            };
            for axis in 0..axes {
                if kind == EquipmentKind::PipeRack && axis == 1 {
                    continue;
                }
                let device = EquipmentSnapshot {
                    tank_size: None,
                    column_design: None,
                    id: 1,
                    revision: 0,
                    kind,
                    origin: [0, 1, 0],
                    axis,
                    contents: vec![],
                    internal_energy_j: 0.0,
                    enabled: false,
                    opening: 1.0,
                    pump_reversed: false,
                    thermal: (kind == EquipmentKind::HeatExchanger).then(ThermalSettings::default),
                    pipe_ends: None,
                    reading: None,
                };
                let boxes = exa_simulation::equipment::collision_boxes(&device);
                let mesh = equipment_shape(kind, axis);
                assert!(!mesh.vertices.is_empty());
                for vertex in mesh.vertices {
                    let mut point = vertex.position;
                    point[1] += 1.0;
                    assert!(
                        boxes.iter().any(|shape| (0..3).all(|i| {
                            point[i] >= shape.min[i] - 1e-6 && point[i] <= shape.max[i] + 1e-6
                        })),
                        "{kind:?} axis={axis}: visible surface outside shared collision {point:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn column_mesh_tracks_trays_feed_and_diameter_inside_shared_collision() {
        for kind in [EquipmentKind::Column, EquipmentKind::RefluxColumn] {
            for diameter in 2..=6 {
                for trays in [2, 12] {
                    for feed_tray in [1, trays] {
                        let mut design = ColumnDesign {
                            diameter,
                            ..Default::default()
                        };
                        design.spec.trays = trays;
                        design.spec.feed_tray = feed_tray;
                        let device = EquipmentSnapshot {
                            id: 1,
                            revision: 0,
                            kind,
                            origin: [0; 3],
                            axis: 0,
                            tank_size: None,
                            column_design: Some(design.clone()),
                            pipe_ends: None,
                            contents: vec![],
                            internal_energy_j: 0.0,
                            enabled: false,
                            opening: 1.0,
                            pump_reversed: false,
                            thermal: None,
                            reading: None,
                        };
                        let mesh = equipment_shape_configured(kind, 0, None, Some(&design));
                        assert!(!mesh.vertices.is_empty());
                        let boxes = exa_simulation::equipment::collision_boxes(&device);
                        for vertex in &mesh.vertices {
                            assert!(
                                boxes.iter().any(|shape| (0..3)
                                    .all(|i| vertex.position[i] >= shape.min[i] - 1e-5
                                        && vertex.position[i] <= shape.max[i] + 1e-5)),
                                "{design:?} {:?}",
                                vertex.position
                            );
                        }
                        for port in equipment_ports(&device) {
                            let along = port.normal.iter().position(|n| *n != 0).unwrap();
                            assert!(mesh
                                .vertices
                                .iter()
                                .any(|v| (0..3).all(|i| (v.position[i] - port.position[i]).abs()
                                    <= if i == along { 1e-5 } else { 0.126 })));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn reflux_component_meshes_match_collision_and_every_role_port() {
        for kind in [
            EquipmentKind::Condenser,
            EquipmentKind::Reboiler,
            EquipmentKind::Tee,
        ] {
            for axis in 0..kind.axis_count() {
                let d = EquipmentSnapshot {
                    id: 1,
                    revision: 0,
                    kind,
                    axis,
                    origin: [0; 3],
                    tank_size: None,
                    column_design: None,
                    pipe_ends: None,
                    contents: vec![],
                    internal_energy_j: 0.0,
                    enabled: false,
                    opening: 1.0,
                    pump_reversed: false,
                    thermal: None,
                    reading: None,
                };
                let mesh = equipment_shape(kind, axis);
                let boxes = exa_simulation::equipment::collision_boxes(&d);
                assert!(!mesh.vertices.is_empty());
                for v in &mesh.vertices {
                    assert!(
                        boxes
                            .iter()
                            .any(|b| (0..3).all(|i| v.position[i] >= b.min[i] - 1e-5
                                && v.position[i] <= b.max[i] + 1e-5)),
                        "{kind:?} axis={axis} vertex={:?}",
                        v.position
                    );
                }
                for p in equipment_ports(&d) {
                    let along = p.normal.iter().position(|n| *n != 0).unwrap();
                    assert!(mesh
                        .vertices
                        .iter()
                        .any(|v| (0..3).all(|i| (v.position[i] - p.position[i]).abs()
                            <= if i == along { 1e-5 } else { 0.126 })));
                }
            }
        }
    }

    #[test]
    fn every_explicit_pipe_join_stays_in_the_shared_arm_collision() {
        for first in 0..6 {
            for second in first + 1..6 {
                let device = EquipmentSnapshot {
                    tank_size: None,
                    column_design: None,
                    id: 1,
                    revision: 0,
                    kind: EquipmentKind::Pipe,
                    origin: [0, 1, 0],
                    axis: 0,
                    contents: vec![],
                    internal_energy_j: 0.0,
                    enabled: false,
                    opening: 1.0,
                    pump_reversed: false,
                    thermal: None,
                    pipe_ends: Some([first, second]),
                    reading: None,
                };
                assert!(device.valid());
                let boxes = exa_simulation::equipment::collision_boxes(&device);
                let mesh = equipment_shape_with_ends(device.kind, device.axis, device.pipe_ends);
                assert!(!mesh.vertices.is_empty());
                for vertex in mesh.vertices {
                    let mut point = vertex.position;
                    point[1] += 1.0;
                    assert!(
                        boxes
                            .iter()
                            .any(|shape| (0..3).all(|i| point[i] >= shape.min[i] - 1e-6
                                && point[i] <= shape.max[i] + 1e-6)),
                        "{first}/{second}: {point:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn pipe_side_bands_partition_length_without_overlays_or_internal_caps() {
        for axis in 0..3 {
            let mesh = equipment_shape(EquipmentKind::Pipe, axis as u8);
            let mut side_normals: Vec<[f32; 3]> = vec![];
            let mut cap_count = [0; 2];
            for tri in mesh.vertices.as_chunks::<3>().0 {
                let normal = tri[0].normal;
                if normal[axis].abs() < 0.5 {
                    if !side_normals.contains(&normal) {
                        side_normals.push(normal);
                    }
                } else {
                    let end = usize::from(normal[axis] > 0.0);
                    assert!(
                        tri.iter()
                            .all(|vertex| (vertex.position[axis] - end as f32).abs() < 1e-6),
                        "axis={axis}: cap buried inside pipe"
                    );
                    cap_count[end] += 1;
                }
            }
            assert_eq!(side_normals.len(), 32);
            assert_eq!(cap_count, [64, 64]); // cap fan plus degenerate quad halves
            for normal in side_normals {
                let mut spans: Vec<(f32, f32)> = mesh
                    .vertices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter(|tri| tri[0].normal == normal)
                    .map(|tri| {
                        (
                            tri.iter()
                                .map(|v| v.position[axis])
                                .fold(f32::INFINITY, f32::min),
                            tri.iter()
                                .map(|v| v.position[axis])
                                .fold(f32::NEG_INFINITY, f32::max),
                        )
                    })
                    .collect();
                spans.sort_by(|a, b| a.0.total_cmp(&b.0));
                assert_eq!(spans.len(), 6, "each facet has three disjoint quads");
                for (index, (start, end)) in [(0.0, 0.04), (0.04, 0.96), (0.96, 1.0)]
                    .into_iter()
                    .enumerate()
                {
                    for span in &spans[index * 2..index * 2 + 2] {
                        assert!(
                            (span.0 - start).abs() < 1e-6 && (span.1 - end).abs() < 1e-6,
                            "axis={axis}: side span {span:?} overlaps or leaves a gap"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cylinder_surfaces_keep_outward_winding_smooth_sides_and_hard_caps() {
        let origin = [4.0, -3.0, 7.0];
        let dot = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f32>();
        for axis in 0..3 {
            let mut mesh = Mesh::default();
            cylinder(&mut mesh, origin, axis, 2.85, 0.9, [1.0; 3]);
            let mut counts = [0; 3]; // side, bottom, top
            for tri in mesh.vertices.as_chunks::<3>().0 {
                let a: [f32; 3] = std::array::from_fn(|i| tri[1].position[i] - tri[0].position[i]);
                let b: [f32; 3] = std::array::from_fn(|i| tri[2].position[i] - tri[0].position[i]);
                let cross = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let area2 = dot(cross, cross).sqrt();
                // A cap fan currently occupies one half of each Mesh::face quad.
                if area2 < 1e-8 {
                    continue;
                }
                let normal = tri[0].normal;
                assert!(
                    dot(cross, normal) / area2 > 0.999,
                    "axis={axis}, tri={:?}",
                    tri.map(|v| v.position)
                );
                let mut outward: [f32; 3] = std::array::from_fn(|i| {
                    tri.iter().map(|v| v.position[i]).sum::<f32>() / 3.0 - origin[i]
                });
                if normal[axis] < -0.5 {
                    counts[1] += 1;
                    assert!(tri
                        .iter()
                        .all(|v| (v.position[axis] - origin[axis]).abs() < 1e-6));
                    outward = [0.0; 3];
                    outward[axis] = -1.0;
                    assert!(tri.iter().all(|vertex| vertex.normal == outward));
                } else if normal[axis] > 0.5 {
                    counts[2] += 1;
                    assert!(tri
                        .iter()
                        .all(|v| (v.position[axis] - origin[axis] - 2.85).abs() < 1e-6));
                    outward = [0.0; 3];
                    outward[axis] = 1.0;
                    assert!(tri.iter().all(|vertex| vertex.normal == outward));
                } else {
                    counts[0] += 1;
                    outward[axis] = 0.0;
                    for vertex in tri {
                        let mut radial: [f32; 3] =
                            std::array::from_fn(|i| (vertex.position[i] - origin[i]) / 0.9);
                        radial[axis] = 0.0;
                        assert!((dot(vertex.normal, vertex.normal) - 1.0).abs() < 1e-6);
                        assert!(
                            (0..3).all(|i| (vertex.normal[i] - radial[i]).abs() < 2e-6),
                            "side shading must follow the round surface at each vertex"
                        );
                    }
                    let mut chord_mid: [f32; 3] = std::array::from_fn(|i| {
                        (tri[0].position[i] + tri[1].position[i]) * 0.5 - origin[i]
                    });
                    chord_mid[axis] = 0.0;
                    assert!(
                        0.9 - dot(chord_mid, chord_mid).sqrt() < 0.001,
                        "tank silhouette facets must deviate less than 1 mm from its radius"
                    );
                }
                assert!(
                    dot(cross, outward) > 0.0,
                    "axis={axis}: front face points into cylinder"
                );
                assert!(
                    dot(normal, outward) > 0.0,
                    "axis={axis}: shading normal points into cylinder"
                );
            }
            assert_eq!(counts, [192, 96, 96], "axis={axis}: missing exterior faces");
        }
    }

    #[test]
    fn cached_neighbors_match_grid_queries_across_signed_edges_water_and_unloads() {
        let mut m = Client3d::default();
        for x in [-16, 0, 16] {
            for z in [-16, 0, 16] {
                for y in [0, 16] {
                    let blocks: Vec<_> = (0..4096).map(|i| ((i * 17 + i / 19) % 9) as u8).collect();
                    assert!(m.terrain.apply_chunk([x, y, z], &blocks));
                }
            }
        }
        for unloaded in [false, true] {
            if unloaded {
                m.terrain.unload_column([-1, 0]);
            }
            for origin in [[0, 0, 0], [-16, 16, -16], [16, 0, 16], [0, 16, 0]] {
                let mesh = terrain_chunk(&m, origin);
                let mut reference = Mesh::default();
                for y in origin[1]..origin[1] + 16 {
                    for z in origin[2]..origin[2] + 16 {
                        for x in origin[0]..origin[0] + 16 {
                            let block = m.terrain.get([x, y, z]);
                            if block == AIR || block == 255 {
                                continue;
                            }
                            for (face, normal) in NORMALS.iter().enumerate() {
                                let neighbor = m.terrain.get([
                                    x + normal[0] as i32,
                                    y + normal[1] as i32,
                                    z + normal[2] as i32,
                                ]);
                                if (neighbor != AIR
                                    && !(neighbor == WATER && block != WATER)
                                    && neighbor != 255)
                                    || (y == 0 && face == 2)
                                {
                                    continue;
                                }
                                reference.block_face(
                                    [x as f32, y as f32, z as f32],
                                    [1.0; 3],
                                    face,
                                    [0.0; 3],
                                    0,
                                );
                            }
                        }
                    }
                }
                let topology = |mesh: Mesh| {
                    mesh.vertices
                        .into_iter()
                        .map(|v| (v.position, v.normal, v.uv))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    topology(mesh),
                    topology(reference),
                    "origin={origin:?}, unloaded={unloaded}"
                );
            }
        }
        assert!(terrain_chunk(&m, [-16, 0, 0]).vertices.is_empty());
    }

    #[test]
    fn contact_ao_crosses_negative_section_corners_and_clears_on_removal() {
        let mut m = Client3d::default();
        m.terrain = Grid::empty();
        for x in [-16, 0] {
            for z in [-16, 0] {
                assert!(m.terrain.apply_chunk([x, 0, z], &[AIR; 4096]));
            }
        }
        m.terrain.set([-1, 7, -1], STONE);
        let top_corner = |m: &Client3d| {
            terrain_chunk(m, [-16, 0, -16])
                .vertices
                .into_iter()
                .find(|v| v.normal == [0.0, 1.0, 0.0] && v.position == [0.0, 8.0, 0.0])
                .unwrap()
                .occlusion
        };
        assert_eq!(top_corner(&m), 1.0);
        m.terrain.set([0, 8, 0], STONE);
        assert!((top_corner(&m) - 0.82).abs() < 0.001);
        // The diagonal blocker touches only UV corner 3. Both triangles must
        // carry the complete face data, rather than interpolating packed bits.
        let mesh = terrain_chunk(&m, [-16, 0, -16]);
        let top: Vec<_> = mesh
            .vertices
            .iter()
            .filter(|v| v.normal == [0.0, 1.0, 0.0])
            .collect();
        assert_eq!(top.len(), 6);
        assert!(top.iter().all(|v| v.face_occlusion == 64));
        m.terrain.set([0, 8, -1], STONE);
        m.terrain.set([-1, 8, 0], STONE);
        assert!((top_corner(&m) - 0.46).abs() < 0.001);
        m.terrain.set([0, 8, 0], AIR);
        assert!((top_corner(&m) - 0.46).abs() < 0.001);
        m.terrain.set([0, 8, -1], WATER);
        m.terrain.set([-1, 8, 0], AIR);
        assert_eq!(top_corner(&m), 1.0);
    }

    #[test]
    fn bevels_only_round_exposed_edges_and_update_across_sections() {
        let mut m = Client3d::default();
        m.terrain = Grid::empty();
        m.terrain.set([15, 7, 8], STONE);
        let isolated = terrain_chunk(&m, [0, 0, 0]);
        assert!(isolated.vertices.iter().all(|v| v.bevel_mask == 15));
        m.terrain.set([16, 7, 8], STONE);
        let left = terrain_chunk(&m, [0, 0, 0]);
        let right = terrain_chunk(&m, [16, 0, 0]);
        assert!(left
            .vertices
            .iter()
            .filter(|v| v.normal == [0., 1., 0.])
            .all(|v| v.bevel_mask == 11));
        assert!(right
            .vertices
            .iter()
            .filter(|v| v.normal == [0., 1., 0.])
            .all(|v| v.bevel_mask == 14));
        m.terrain.set([16, 7, 8], AIR);
        assert!(terrain_chunk(&m, [0, 0, 0])
            .vertices
            .iter()
            .all(|v| v.bevel_mask == 15));
        m.terrain.set([15, 7, 8], WATER);
        assert!(terrain_chunk(&m, [0, 0, 0])
            .vertices
            .iter()
            .all(|v| v.bevel_mask == 0));
    }

    #[test]
    fn adjacent_chunks_cull_their_shared_faces() {
        let mut m = Client3d::default();
        m.terrain = Grid::empty();
        m.terrain.set([15, 7, 8], STONE);
        m.terrain.set([16, 7, 8], STONE);
        let left = terrain_chunk(&m, [0, 0, 0]);
        let right = terrain_chunk(&m, [16, 0, 0]);
        assert_eq!(left.vertices.len(), 5 * 6);
        assert_eq!(right.vertices.len(), 5 * 6);
        assert!(left.vertices.iter().all(|v| v.normal != [1.0, 0.0, 0.0]));
        assert!(right.vertices.iter().all(|v| v.normal != [-1.0, 0.0, 0.0]));
    }
}

//! Bounded industrial steps over immutable facts. Legacy open tanks keep their
//! single-liquid water model; `thermal` adds sealed hydrocarbon equilibrium and
//! coupled heat exchange. Inline devices have zero holdup (no pipe filling).
use game_simulation::equipment::{ports, tank_area_m2, tank_capacity_m3, tank_centroid_height_m};
use protocol::{EquipmentKind, EquipmentReading, EquipmentSnapshot, MaterialAmount, MAX_EQUIPMENT};
use std::collections::{BTreeMap, BTreeSet};
mod thermal;
pub use thermal::{charge, heat, solve};

pub const WATER_MATERIAL: &str = "icheme:water";
pub const WATER_DENSITY_KG_M3: f64 = 1000.0;
pub const WATER_MOLAR_MASS_KG: f64 = 0.01801528;
pub const WATER_HEAT_CAPACITY_J_KG_K: f64 = 4180.0;
/// Legacy 2 m by 3 m fixture dimensions; production calculations use each instance.
pub const TANK_AREA_M2: f64 = std::f64::consts::PI * 0.9 * 0.9;
pub const TANK_CAPACITY_M3: f64 = TANK_AREA_M2 * 2.8;
pub const PUMP_PRESSURE_PA: f64 = 50_000.0;
const G: f64 = 9.80665;
const ATM: f64 = 101_325.0;
const MAX_FLOW: f64 = 0.02;

// Both free surfaces move when volume is transferred. Retain the original
// expression for equal areas, including the legacy default geometry.
fn water_equilibrium_volume(drive: f64, from: &EquipmentSnapshot, to: &EquipmentSnapshot) -> f64 {
    let a = tank_area_m2(from);
    let b = tank_area_m2(to);
    if a == b {
        drive.abs() * a / (2.0 * WATER_DENSITY_KG_M3 * G)
    } else {
        drive.abs() / (WATER_DENSITY_KG_M3 * G * (1.0 / a + 1.0 / b))
    }
}

pub struct FlowStep {
    pub devices: Vec<EquipmentSnapshot>,
    pub pump_work_j: f64,
}
pub fn water_moles(device: &EquipmentSnapshot) -> f64 {
    device
        .contents
        .iter()
        .filter(|c| c.material == WATER_MATERIAL)
        .map(|c| c.moles)
        .sum()
}
fn volume(device: &EquipmentSnapshot) -> f64 {
    water_moles(device) * WATER_MOLAR_MASS_KG / WATER_DENSITY_KG_M3
}
fn potential(device: &EquipmentSnapshot) -> f64 {
    if device.kind == EquipmentKind::Vessel {
        // Lumped sealed-vessel model: fixed geometric centre of mass. The legacy
        // open-water model retains its level-dependent gravitational energy.
        let mass: f64 = device
            .contents
            .iter()
            .map(|c| {
                crate::thermo::MATERIAL_IDS
                    .iter()
                    .position(|id| *id == c.material)
                    .map_or(0.0, |i| c.moles * crate::thermo::MOLAR_MASS_KG[i])
            })
            .sum();
        return mass * G * (f64::from(device.origin[1]) + tank_centroid_height_m(device));
    }
    let v = volume(device);
    WATER_DENSITY_KG_M3 * v * G * (f64::from(device.origin[1]) + v / (2.0 * tank_area_m2(device)))
}
pub fn stored_energy_j(device: &EquipmentSnapshot) -> f64 {
    device.internal_energy_j + potential(device)
}
fn validate(device: &EquipmentSnapshot) -> Result<(), String> {
    if !device.valid() {
        return Err("invalid equipment facts".into());
    }
    if device.contents.iter().any(|c| c.material != WATER_MATERIAL) {
        return Err("unsupported material: test loop accepts liquid water only".into());
    }
    if device.kind != EquipmentKind::Tank
        && (!device.contents.is_empty() || device.internal_energy_j != 0.0)
    {
        return Err("inline inventory unsupported; existing contents preserved".into());
    }
    if volume(device) > tank_capacity_m3(device) + 1e-10 || device.internal_energy_j < 0.0 {
        return Err("water capacity or energy outside model".into());
    }
    let mass = water_moles(device) * WATER_MOLAR_MASS_KG;
    if (mass == 0.0 && device.internal_energy_j != 0.0)
        || (mass > 0.0 && device.internal_energy_j / (mass * WATER_HEAT_CAPACITY_J_KG_K) > 100.0)
    {
        return Err("temperature outside single-liquid test range 273.15–373.15 K".into());
    }
    Ok(())
}
pub fn fill_water(
    device: &mut EquipmentSnapshot,
    liters: f64,
    temp_k: f64,
) -> Result<(f64, f64), String> {
    validate(device)?;
    if device.kind != EquipmentKind::Tank
        || !liters.is_finite()
        || liters <= 0.0
        || !temp_k.is_finite()
        || !(273.15..=373.15).contains(&temp_k)
        // Match validation's volume tolerance: L -> m3 roundoff must not reject
        // an exactly full parameterized tank. Never discard the supplied mass.
        || volume(device) + liters / 1000.0 > tank_capacity_m3(device) + 1e-10
    {
        return Err("invalid water amount, temperature or tank capacity".into());
    }
    let before = stored_energy_j(device);
    let moles = liters / WATER_MOLAR_MASS_KG;
    let total = water_moles(device) + moles;
    device.contents = vec![MaterialAmount {
        material: WATER_MATERIAL.into(),
        moles: total,
    }];
    device.internal_energy_j += liters * WATER_HEAT_CAPACITY_J_KG_K * (temp_k - 273.15);
    device.reading = None;
    Ok((moles, stored_energy_j(device) - before))
}
pub fn drain_water(device: &mut EquipmentSnapshot) -> Result<(f64, f64), String> {
    validate(device)?;
    if device.kind != EquipmentKind::Tank {
        return Err("only a tank can be drained".into());
    }
    let result = (water_moles(device), stored_energy_j(device));
    device.contents.clear();
    device.internal_energy_j = 0.0;
    device.reading = None;
    Ok(result)
}

struct Path {
    a: usize,
    b: usize,
    inline: Vec<(usize, i8)>,
}
fn graph(devices: &[EquipmentSnapshot]) -> Result<Vec<Path>, String> {
    type PortEndpoint = (usize, usize, [i32; 3]);
    let mut ends: BTreeMap<[i32; 3], Vec<PortEndpoint>> = BTreeMap::new();
    for (i, d) in devices.iter().enumerate() {
        for (p, port) in ports(d).iter().enumerate() {
            ends.entry(port.position.map(|v| (v * 2.0).round() as i32))
                .or_default()
                .push((i, p, port.normal));
        }
    }
    let mut links = vec![[None; 2]; devices.len()];
    for group in ends.values() {
        if group.len() > 2 {
            return Err("ambiguous port connection".into());
        }
        if let [a, b] = group.as_slice() {
            if a.0 == b.0 || a.2 != b.2.map(|v| -v) {
                return Err("invalid facing ports".into());
            }
            links[a.0][a.1] = Some((b.0, b.1));
            links[b.0][b.1] = Some((a.0, a.1));
        }
    }
    let mut paths = Vec::new();
    for (a, tank) in devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == EquipmentKind::Tank)
    {
        for start in 0..2 {
            let mut cursor = links[a][start];
            let mut seen = BTreeSet::new();
            let mut inline = Vec::new();
            while let Some((i, port)) = cursor {
                if !seen.insert(i) {
                    break;
                }
                if devices[i].kind == EquipmentKind::Tank {
                    if tank.id < devices[i].id {
                        paths.push(Path { a, b: i, inline });
                    }
                    break;
                }
                inline.push((i, if port == 0 { 1 } else { -1 }));
                cursor = links[i][1 - port];
            }
        }
    }
    Ok(paths)
}

/// One bounded step; no input is mutated even if the result fails validation.
fn solve_water(input: &[EquipmentSnapshot], dt_s: f64) -> Result<FlowStep, String> {
    if input.len() > MAX_EQUIPMENT
        || !dt_s.is_finite()
        || !(0.0..=0.2).contains(&dt_s)
        || dt_s == 0.0
    {
        return Err("invalid process step or equipment count".into());
    }
    let mut devices = input.to_vec();
    devices.sort_by_key(|d| d.id);
    for (i, d) in devices.iter().enumerate() {
        validate(d)?;
        if i > 0 && devices[i - 1].id == d.id {
            return Err("duplicate equipment identity".into());
        }
    }
    let initial_energy: f64 = devices.iter().map(stored_energy_j).sum();
    let initial_moles: f64 = devices.iter().map(water_moles).sum();
    let paths = graph(&devices)?;
    let mut rates = vec![0.0; devices.len()];
    let mut connected = vec![false; devices.len()];
    let mut blocked = vec![false; devices.len()];
    let mut work = 0.0;
    for path in paths {
        connected[path.a] = true;
        connected[path.b] = true;
        let mut head = 0.0;
        let mut resistance = 1e6;
        let mut closed = false;
        let mut pump_direction = 0;
        for &(i, direction) in &path.inline {
            connected[i] = true;
            match devices[i].kind {
                EquipmentKind::Valve => {
                    closed |= devices[i].opening == 0.0;
                    resistance += 1e6 / devices[i].opening.max(0.001).powi(2);
                }
                EquipmentKind::Pump => {
                    closed |= !devices[i].enabled;
                    let sign = direction * if devices[i].pump_reversed { -1 } else { 1 };
                    if pump_direction != 0 && pump_direction != sign {
                        closed = true;
                    }
                    pump_direction = sign;
                    head += f64::from(sign) * PUMP_PRESSURE_PA;
                    resistance += 1e6;
                }
                _ => resistance += 1e6,
            }
        }
        if closed {
            for &(i, _) in &path.inline {
                blocked[i] = true;
            }
            continue;
        }
        let surface = |d: &EquipmentSnapshot| f64::from(d.origin[1]) + volume(d) / tank_area_m2(d);
        let drive =
            WATER_DENSITY_KG_M3 * G * (surface(&devices[path.a]) - surface(&devices[path.b]))
                + head;
        let q = (drive / resistance).clamp(-MAX_FLOW, MAX_FLOW);
        if q == 0.0 || (pump_direction != 0 && q.signum() != f64::from(pump_direction)) {
            continue;
        }
        let (from, to) = if q > 0.0 {
            (path.a, path.b)
        } else {
            (path.b, path.a)
        };
        // A liquid nozzle at 0.5 m cannot draw the heel beneath it.
        let available = (volume(&devices[from]) - tank_area_m2(&devices[from]) * 0.5).max(0.0);
        let capacity = (tank_capacity_m3(&devices[to]) - volume(&devices[to])).max(0.0);
        // Do not cross the hydrostatic equilibrium in a finite explicit step.
        let equilibrium = water_equilibrium_volume(drive, &devices[from], &devices[to]);
        let dv = (q.abs() * dt_s)
            .min(available)
            .min(capacity)
            .min(equilibrium);
        if dv <= 0.0 {
            continue;
        }
        let dn = dv * WATER_DENSITY_KG_M3 / WATER_MOLAR_MASS_KG;
        let old_pe = potential(&devices[from]) + potential(&devices[to]);
        let transferred_u = devices[from].internal_energy_j * (dn / water_moles(&devices[from]));
        let from_n = water_moles(&devices[from]) - dn;
        let to_n = water_moles(&devices[to]) + dn;
        devices[from].contents = vec![MaterialAmount {
            material: WATER_MATERIAL.into(),
            moles: from_n,
        }];
        devices[to].contents = vec![MaterialAmount {
            material: WATER_MATERIAL.into(),
            moles: to_n,
        }];
        let pump_work = dv * head.abs();
        let dissipated = old_pe - potential(&devices[from]) - potential(&devices[to]) + pump_work;
        devices[from].internal_energy_j -= transferred_u;
        devices[to].internal_energy_j += transferred_u + dissipated;
        work += pump_work;
        let signed_rate = dv / dt_s * q.signum();
        rates[path.a] -= signed_rate;
        rates[path.b] += signed_rate;
        for &(i, direction) in &path.inline {
            rates[i] += signed_rate * f64::from(direction);
        }
    }
    for (i, d) in devices.iter_mut().enumerate() {
        validate(d)?;
        let mass = water_moles(d) * WATER_MOLAR_MASS_KG;
        d.reading = Some(EquipmentReading {
            temperature_k: (mass > 0.0)
                .then(|| 273.15 + d.internal_energy_j / (mass * WATER_HEAT_CAPACITY_J_KG_K)),
            pressure_pa: (d.kind == EquipmentKind::Tank).then(|| {
                ATM + WATER_DENSITY_KG_M3 * G * (volume(d) / tank_area_m2(d) - 0.5).max(0.0)
            }),
            volume_m3: volume(d),
            capacity_m3: if d.kind == EquipmentKind::Tank {
                tank_capacity_m3(d)
            } else {
                0.0
            },
            flow_m3_s: rates[i],
            status: if !connected[i] {
                "disconnected"
            } else if blocked[i] {
                "closed"
            } else if rates[i].abs() > 0.0 {
                "flowing"
            } else {
                "idle"
            }
            .into(),
            thermal: None,
        });
    }
    let final_energy: f64 = devices.iter().map(stored_energy_j).sum();
    let final_moles: f64 = devices.iter().map(water_moles).sum();
    if (final_energy - initial_energy - work).abs() > 1e-6 * initial_energy.abs().max(1.0)
        || (final_moles - initial_moles).abs() > 1e-10 * initial_moles.max(1.0)
    {
        return Err("process conservation check failed".into());
    }
    Ok(FlowStep {
        devices,
        pump_work_j: work,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: u64, kind: EquipmentKind, x: i32) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id,
            revision: 7,
            kind,
            origin: [x, 10, 0],
            axis: 0,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            pipe_ends: None,
            tank_size: None,
            thermal: None,
            reading: None,
        }
    }
    fn circuit(pumped: bool) -> Vec<EquipmentSnapshot> {
        let mut result = vec![
            device(1, EquipmentKind::Tank, 0),
            device(
                2,
                if pumped {
                    EquipmentKind::Pump
                } else {
                    EquipmentKind::Pipe
                },
                2,
            ),
            device(3, EquipmentKind::Valve, 3),
            device(4, EquipmentKind::Tank, 4),
        ];
        fill_water(&mut result[0], 3000.0, 293.15).unwrap();
        fill_water(&mut result[3], 4000.0, 303.15).unwrap();
        result[1].enabled = true;
        result
    }
    fn energy(ds: &[EquipmentSnapshot]) -> f64 {
        ds.iter().map(stored_energy_j).sum()
    }
    fn amount(ds: &[EquipmentSnapshot]) -> f64 {
        ds.iter().map(water_moles).sum()
    }
    #[test]
    fn differently_sized_water_tanks_follow_levels_and_conserve_energy() {
        let mut ds = vec![
            device(1, EquipmentKind::Tank, 0),
            device(2, EquipmentKind::Pipe, 4),
            device(3, EquipmentKind::Tank, 5),
        ];
        ds[0].tank_size = Some(protocol::TankSize {
            diameter: 4,
            height: 8,
        });
        // D4 nozzles use local z=1.5; align them with the narrow line at z=0.5.
        ds[0].origin[2] = -1;
        assert!(game_simulation::equipment::connected(
            ports(&ds[0])[1],
            ports(&ds[1])[0]
        ));
        assert!(game_simulation::equipment::connected(
            ports(&ds[1])[1],
            ports(&ds[2])[0]
        ));
        let a = tank_area_m2(&ds[0]);
        let b = tank_area_m2(&ds[2]);
        fill_water(&mut ds[0], a * 1500.0, 300.0).unwrap();
        fill_water(&mut ds[2], b * 1500.0, 300.0).unwrap();
        let balanced = solve(&ds, 0.2).unwrap();
        assert!((water_moles(&balanced.devices[0]) - water_moles(&ds[0])).abs() < 1e-8);
        // A much larger amount in the wide tank is not a pressure difference.
        assert!(water_moles(&ds[0]) > water_moles(&ds[2]) * 4.0);
        fill_water(&mut ds[0], 1000.0, 300.0).unwrap();
        let initial_n = amount(&ds);
        let initial_e = energy(&ds);
        let source_n = water_moles(&ds[0]);
        for _ in 0..200 {
            ds = solve(&ds, 0.2).unwrap().devices;
            assert!(volume(&ds[0]) / a >= volume(&ds[2]) / b - 1e-12);
        }
        assert!(water_moles(&ds[0]) < source_n);
        assert!((amount(&ds) - initial_n).abs() < initial_n * 1e-12);
        assert!((energy(&ds) - initial_e).abs() < initial_e * 1e-12);
        assert_eq!(
            ds[0].reading.as_ref().unwrap().capacity_m3,
            tank_capacity_m3(&ds[0])
        );
        let mut full = device(4, EquipmentKind::Tank, 10);
        full.tank_size = ds[0].tank_size;
        let liters = tank_capacity_m3(&full) * 1000.0;
        fill_water(&mut full, liters, 300.0).unwrap();
        let before = full.clone();
        assert!(fill_water(&mut full, 1.0, 300.0).is_err());
        assert_eq!(full, before);
    }

    #[test]
    fn explicit_default_tanks_keep_legacy_water_results() {
        let ds = circuit(true);
        let mut explicit = ds.clone();
        for d in &mut explicit {
            if d.kind == EquipmentKind::Tank {
                d.tank_size = Some(protocol::TankSize {
                    diameter: 2,
                    height: 3,
                });
            }
        }
        let legacy = solve(&ds, 0.2).unwrap();
        let mut sized = solve(&explicit, 0.2).unwrap();
        for d in &mut sized.devices {
            d.tank_size = None;
        }
        assert_eq!(legacy.devices, sized.devices);
        assert_eq!(legacy.pump_work_j, sized.pump_work_j);
    }
    #[test]
    fn pumped_reverse_gradient_conserves_mass_and_total_energy() {
        let initial = circuit(true);
        let mut ds = initial.clone();
        let mut work = 0.0;
        for _ in 0..500 {
            let step = solve(&ds, 0.2).unwrap();
            work += step.pump_work_j;
            ds = step.devices;
        }
        assert!(water_moles(&ds[0]) < water_moles(&initial[0]));
        assert!(work > 0.0);
        assert!((amount(&ds) - amount(&initial)).abs() < 1e-6);
        assert!((energy(&ds) - energy(&initial) - work).abs() < 1e-4);
        assert!(ds.iter().all(|d| d.revision == 7));
        assert!(ds[1].reading.as_ref().unwrap().pressure_pa.is_none());
    }
    #[test]
    fn natural_flow_reverses_and_controls_stop_the_entire_series() {
        let ds = circuit(false);
        let next = solve(&ds, 0.2).unwrap();
        assert!(water_moles(&next.devices[0]) > water_moles(&ds[0]));
        assert_eq!(next.pump_work_j, 0.0);
        assert!((energy(&next.devices) - energy(&ds)).abs() < 1e-6);
        let mut pumped = circuit(true);
        pumped[1].enabled = false;
        let off = solve(&pumped, 0.2).unwrap();
        assert_eq!(water_moles(&off.devices[0]), water_moles(&pumped[0]));
        pumped[1].enabled = true;
        pumped[2].opening = 0.0;
        let closed = solve(&pumped, 0.2).unwrap();
        assert_eq!(water_moles(&closed.devices[0]), water_moles(&pumped[0]));
        pumped[2].opening = 1.0;
        pumped[1].pump_reversed = true;
        assert!(water_moles(&solve(&pumped, 0.2).unwrap().devices[0]) > water_moles(&pumped[0]));
        let full = solve(&pumped, 0.2).unwrap().devices[1]
            .reading
            .as_ref()
            .unwrap()
            .flow_m3_s
            .abs();
        pumped[2].opening = 0.1;
        assert!(
            solve(&pumped, 0.2).unwrap().devices[1]
                .reading
                .as_ref()
                .unwrap()
                .flow_m3_s
                .abs()
                < full
        );
    }
    #[test]
    fn disconnects_capacity_and_nozzle_heel_do_not_destroy_water() {
        let mut ds = circuit(true);
        ds.remove(2);
        assert_eq!(
            water_moles(&solve(&ds, 0.2).unwrap().devices[0]),
            water_moles(&ds[0])
        );
        ds = circuit(true);
        drain_water(&mut ds[3]).unwrap();
        fill_water(&mut ds[3], TANK_CAPACITY_M3 * 1000.0, 293.15).unwrap();
        assert_eq!(
            water_moles(&solve(&ds, 0.2).unwrap().devices[0]),
            water_moles(&ds[0])
        );
        drain_water(&mut ds[0]).unwrap();
        fill_water(&mut ds[0], 100.0, 293.15).unwrap();
        drain_water(&mut ds[3]).unwrap();
        assert_eq!(
            water_moles(&solve(&ds, 0.2).unwrap().devices[0]),
            water_moles(&ds[0])
        );
    }
    #[test]
    fn unsupported_inventory_and_heating_fail_without_mutating_input() {
        let mut ds = circuit(true);
        ds[0].contents[0].material = "game:oil".into();
        let before = ds.clone();
        assert!(solve_water(&ds, 0.2).is_err());
        assert_eq!(ds, before);
        ds = circuit(true);
        ds[1].contents = ds[0].contents.clone();
        assert!(solve_water(&ds, 0.2).is_err());
        ds = circuit(true);
        drain_water(&mut ds[0]).unwrap();
        drain_water(&mut ds[3]).unwrap();
        fill_water(&mut ds[0], 3000.0, 373.15).unwrap();
        fill_water(&mut ds[3], 4000.0, 373.15).unwrap();
        let before = ds.clone();
        assert!(solve_water(&ds, 0.2).is_err());
        assert_eq!(ds, before);
        assert!(solve(&[], f64::NAN).is_err());
        assert!(solve(&[], 1.0).is_err());
    }
    #[test]
    fn test_source_and_sink_account_for_gravitational_energy() {
        let mut tank = device(1, EquipmentKind::Tank, 0);
        let (added_n, added_e) = fill_water(&mut tank, 1000.0, 293.15).unwrap();
        assert_eq!(added_n, water_moles(&tank));
        assert_eq!(added_e, stored_energy_j(&tank));
        assert!(added_e > tank.internal_energy_j);
        let before = tank.clone();
        assert!(fill_water(&mut tank, 1e20, 293.15).is_err());
        assert_eq!(tank, before);
        let removed = drain_water(&mut tank).unwrap();
        assert_eq!(removed, (added_n, added_e));
        assert!(tank.is_empty());
        assert_eq!(stored_energy_j(&tank), 0.0);
    }
    #[test]
    fn maximum_size_graph_is_bounded_and_order_independent() {
        let started = std::time::Instant::now();
        let mut ds = vec![device(1, EquipmentKind::Tank, 0)];
        ds.extend(
            (1..MAX_EQUIPMENT - 1).map(|i| device(i as u64 + 1, EquipmentKind::Pipe, i as i32 + 1)),
        );
        ds.push(device(
            MAX_EQUIPMENT as u64,
            EquipmentKind::Tank,
            MAX_EQUIPMENT as i32,
        ));
        fill_water(&mut ds[0], 3000.0, 293.15).unwrap();
        fill_water(&mut ds[MAX_EQUIPMENT - 1], 1000.0, 293.15).unwrap();
        let a = solve(&ds, 0.2).unwrap();
        assert!(water_moles(&a.devices[0]) < water_moles(&ds[0]));
        ds.reverse();
        let b = solve(&ds, 0.2).unwrap();
        assert_eq!(a.devices, b.devices);
        assert_eq!(a.devices.len(), MAX_EQUIPMENT);
        ds.push(device(9999, EquipmentKind::Pipe, 2000));
        assert!(solve(&ds, 0.2).is_err());
        eprintln!("two 1024-device process steps: {:?}", started.elapsed());
    }

    #[test]
    fn bent_water_line_descends_across_height_without_creating_mass_or_energy() {
        let mut ds = vec![
            device(1, EquipmentKind::Tank, 0),
            device(2, EquipmentKind::Pipe, 2),
            device(3, EquipmentKind::Pipe, 2),
            device(4, EquipmentKind::Pipe, 2),
            device(5, EquipmentKind::Pipe, 3),
            device(6, EquipmentKind::Pipe, 4),
            device(7, EquipmentKind::Tank, 5),
        ];
        ds[0].origin[1] = 12;
        ds[1].origin[1] = 12;
        ds[1].pipe_ends = Some([0, 2]);
        ds[2].origin[1] = 11;
        ds[2].axis = 1;
        ds[2].pipe_ends = Some([2, 3]);
        ds[3].pipe_ends = Some([1, 3]);
        fill_water(&mut ds[0], 4000.0, 300.0).unwrap();
        fill_water(&mut ds[6], 4000.0, 300.0).unwrap();
        let n: f64 = ds.iter().map(water_moles).sum();
        let e: f64 = ds.iter().map(stored_energy_j).sum();
        let donor = water_moles(&ds[0]);
        for _ in 0..20 {
            ds = solve(&ds, 0.2).unwrap().devices;
        }
        assert!(water_moles(&ds[0]) < donor);
        assert!((ds.iter().map(water_moles).sum::<f64>() - n).abs() < n * 1e-12);
        assert!((ds.iter().map(stored_energy_j).sum::<f64>() - e).abs() < e * 1e-10);
    }
}

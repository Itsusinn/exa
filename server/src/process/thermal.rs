//! Coupled, bounded lumped-vessel transport. Hydraulic circuits exchange heat but never material.
use super::*;
use crate::thermo;
use exa_protocol::{ControlState, HeatControlMode, ReadingQuality, ThermalReading};

pub fn charge(
    device: &mut EquipmentSnapshot,
    amounts: [f64; 2],
    temperature: f64,
) -> Result<f64, String> {
    if !device.valid()
        || device.kind != EquipmentKind::Vessel
        || !device.is_empty()
        || device.internal_energy_j != 0.0
    {
        return Err("charge requires an empty sealed vessel".into());
    }
    let state = thermo::tv(temperature, tank_capacity_m3(device), amounts)?;
    let mut next = device.clone();
    next.contents = thermo::materials_from_amounts(amounts);
    next.internal_energy_j = state.internal_energy_j;
    next.reading = None;
    if !next.valid() {
        return Err("invalid charged vessel".into());
    }
    let energy = stored_energy_j(&next);
    *device = next;
    Ok(energy)
}
pub fn heat(device: &mut EquipmentSnapshot, joules: f64) -> Result<(), String> {
    if !device.valid()
        || device.kind != EquipmentKind::Vessel
        || device.is_empty()
        || !joules.is_finite()
        || joules.abs() > 1e15
    {
        return Err("heat requires a charged sealed vessel and finite energy".into());
    }
    let u = device.internal_energy_j + joules;
    thermo::uv(
        u,
        tank_capacity_m3(device),
        thermo::amounts_from_materials(&device.contents)?,
    )?;
    device.internal_energy_j = u;
    device.reading = None;
    Ok(())
}

fn diagnostic(device: &mut EquipmentSnapshot, error: &str) {
    let mut status = String::from("暂停：");
    for c in error.chars() {
        if status.len() + c.len_utf8() > 96 {
            break;
        }
        status.push(c);
    }
    device.reading = Some(EquipmentReading {
        temperature_k: None,
        pressure_pa: None,
        volume_m3: 0.0,
        capacity_m3: 0.0,
        flow_m3_s: 0.0,
        status,
        thermal: matches!(
            device.kind,
            EquipmentKind::Vessel | EquipmentKind::HeatExchanger
        )
        .then(ThermalReading::default),
    });
}

type Links = Vec<Vec<Option<(usize, usize)>>>;
fn links(devices: &[EquipmentSnapshot]) -> Result<Links, String> {
    let mut endpoints = BTreeMap::<[i32; 3], Vec<(usize, usize, [i32; 3])>>::new();
    let mut links: Links = devices.iter().map(|d| vec![None; ports(d).len()]).collect();
    for (i, d) in devices.iter().enumerate() {
        for (p, port) in ports(d).iter().enumerate() {
            endpoints
                .entry(port.position.map(|v| (v * 2.0).round() as i32))
                .or_default()
                .push((i, p, port.normal));
        }
    }
    for group in endpoints.values() {
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
    Ok(links)
}

/// One worker, independent conservative groups. Heat-exchanger sides share a commit group.
pub fn solve(input: &[EquipmentSnapshot], dt: f64) -> Result<FlowStep, String> {
    if input.len() > MAX_EQUIPMENT || !dt.is_finite() || dt <= 0.0 || dt > 0.2 {
        return Err("invalid process step or equipment count".into());
    }
    let mut devices = input.to_vec();
    devices.sort_by_key(|d| d.id);
    if devices.iter().any(|d| !d.valid()) || devices.windows(2).any(|d| d[0].id == d[1].id) {
        return Err("invalid equipment facts or duplicate identity".into());
    }
    let connections = links(&devices)?;
    let mut seen = BTreeSet::new();
    let mut work = 0.0;
    for start in 0..devices.len() {
        if !seen.insert(start) {
            continue;
        }
        let mut group = vec![start];
        let mut cursor = 0;
        while cursor < group.len() {
            for &(next, _) in connections[group[cursor]].iter().flatten() {
                if seen.insert(next) {
                    group.push(next);
                }
            }
            cursor += 1;
        }
        group.sort_unstable();
        let batch: Vec<_> = group.iter().map(|&i| devices[i].clone()).collect();
        let result = if batch.iter().any(|d| d.kind.is_planning()) {
            Err("蒸馏塔尚未运行；冷凝、再沸及三通管组仅检查结构".into())
        } else if batch
            .iter()
            .any(|d| matches!(d.kind, EquipmentKind::Vessel | EquipmentKind::HeatExchanger))
        {
            solve_coupled(&batch, dt)
        } else {
            solve_water(&batch, dt)
        };
        let result = result.and_then(|step| {
            audit(&batch, &step)?;
            Ok(step)
        });
        match result {
            Ok(step) => {
                work += step.pump_work_j;
                for (&i, d) in group.iter().zip(step.devices) {
                    devices[i] = d;
                }
            }
            Err(error) => {
                for &i in &group {
                    diagnostic(&mut devices[i], &error);
                }
            }
        }
    }
    Ok(FlowStep {
        devices,
        pump_work_j: work,
    })
}
fn audit(before: &[EquipmentSnapshot], after: &FlowStep) -> Result<(), String> {
    let totals = |ds: &[EquipmentSnapshot]| {
        let mut n = BTreeMap::<&str, f64>::new();
        for d in ds {
            for c in &d.contents {
                *n.entry(&c.material).or_default() += c.moles;
            }
        }
        n.into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<BTreeMap<_, _>>()
    };
    let a = totals(before);
    let b = totals(&after.devices);
    let close = |a: f64, b: f64| {
        a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
    };
    if a.keys()
        .chain(b.keys())
        .any(|k| !close(*a.get(k).unwrap_or(&0.0), *b.get(k).unwrap_or(&0.0)))
        || !close(
            before.iter().map(stored_energy_j).sum::<f64>() + after.pump_work_j,
            after.devices.iter().map(stored_energy_j).sum(),
        )
    {
        return Err("group conservation check failed".into());
    }
    Ok(())
}

#[derive(Clone)]
struct State {
    temperature: f64,
    pressure: f64,
    volume: f64,
    hydro: Option<thermo::ThermoState>,
}
fn state(d: &EquipmentSnapshot) -> Result<Option<State>, String> {
    match d.kind {
        EquipmentKind::Tank => {
            validate(d)?;
            let v = volume(d);
            Ok((v > 0.0).then(|| State {
                temperature: 273.15
                    + d.internal_energy_j
                        / (water_moles(d) * WATER_MOLAR_MASS_KG * WATER_HEAT_CAPACITY_J_KG_K),
                pressure: ATM + WATER_DENSITY_KG_M3 * G * (v / tank_area_m2(d) - 0.5).max(0.0),
                volume: v,
                hydro: None,
            }))
        }
        EquipmentKind::Vessel => {
            if d.is_empty() {
                if d.internal_energy_j != 0.0 {
                    return Err("empty vessel has energy".into());
                }
                return Ok(None);
            }
            let s = thermo::uv(
                d.internal_energy_j,
                tank_capacity_m3(d),
                thermo::amounts_from_materials(&d.contents)?,
            )?;
            let liquid_volume = s.liquid.as_ref().map_or(0.0, |phase| {
                phase.amounts.iter().sum::<f64>() * phase.molar_volume_m3
            });
            Ok(Some(State {
                temperature: s.temperature_k,
                pressure: s.pressure_pa,
                volume: liquid_volume,
                hydro: Some(s),
            }))
        }
        _ => {
            if !d.is_empty() || d.internal_energy_j != 0.0 {
                return Err("inline inventory unsupported".into());
            }
            Ok(None)
        }
    }
}
fn reservoir(d: &EquipmentSnapshot) -> bool {
    matches!(d.kind, EquipmentKind::Tank | EquipmentKind::Vessel)
}
#[derive(Clone)]
struct Route {
    a: usize,
    b: usize,
    inline: Vec<(usize, usize, i8)>,
}
fn routes(ds: &[EquipmentSnapshot], ls: &Links) -> Result<Vec<Route>, String> {
    let mut result = vec![];
    for (a, d) in ds.iter().enumerate().filter(|(_, d)| reservoir(d)) {
        for start in 0..2 {
            let mut cursor = ls[a][start];
            let mut seen = BTreeSet::new();
            let mut inline = vec![];
            while let Some((i, port)) = cursor {
                if !seen.insert((i, port / 2)) {
                    break;
                }
                if reservoir(&ds[i]) {
                    if d.kind != ds[i].kind {
                        return Err(
                            "legacy water and sealed hydrocarbons cannot share a fluid circuit"
                                .into(),
                        );
                    }
                    if d.id < ds[i].id {
                        result.push(Route { a, b: i, inline });
                    }
                    break;
                }
                inline.push((i, port / 2, if port % 2 == 0 { 1 } else { -1 }));
                cursor = ls[i][port ^ 1];
            }
        }
    }
    Ok(result)
}
#[derive(Clone)]
struct Packet {
    from: usize,
    to: usize,
    amounts: [f64; 2],
    water: f64,
    h: f64,
    base_h: f64,
    pressure: f64,
    work: f64,
    rate: f64,
    inline: Vec<(usize, usize, i8)>,
}
/// A zero-holdup line has no density profile to resolve a density inversion.
/// Accept only a unique consistent donor, otherwise hold the group. Swapping
/// endpoint identities must change only the sign of the result.
fn hydrostatic_drive(base: f64, rho_a: f64, rho_b: f64, elevation: f64) -> Result<f64, String> {
    let a = base + rho_a * G * elevation;
    let b = base + rho_b * G * elevation;
    let tolerance = 1e-10
        * base
            .abs()
            .max((rho_a * G * elevation).abs())
            .max((rho_b * G * elevation).abs())
            .max(1.0);
    if !a.is_finite() || !b.is_finite() {
        return Err("non-finite hydraulic head".into());
    }
    if a.abs() <= tolerance || b.abs() <= tolerance {
        return Ok(0.0);
    }
    if a.signum() != b.signum() {
        return Err("unequal-density elevation needs a resolved line pressure profile".into());
    }
    Ok(if a > 0.0 { a } else { b })
}
impl Packet {
    fn temperature_at(&self, h: f64) -> Result<f64, String> {
        if self.water > 0.0 {
            Ok(273.15 + h / (self.water * WATER_MOLAR_MASS_KG * WATER_HEAT_CAPACITY_J_KG_K))
        } else {
            Ok(thermo::ph(self.pressure, h, self.amounts)?.temperature_k)
        }
    }
    fn enthalpy_at(&self, t: f64) -> Result<f64, String> {
        if self.water > 0.0 {
            if !(273.15..=373.15).contains(&t) {
                return Err("water heat limit".into());
            }
            Ok(self.water * WATER_MOLAR_MASS_KG * WATER_HEAT_CAPACITY_J_KG_K * (t - 273.15))
        } else {
            Ok(thermo::tp(t, self.pressure, self.amounts)?.enthalpy_j)
        }
    }
}
fn plan(
    ds: &[EquipmentSnapshot],
    states: &[Option<State>],
    route: Route,
    dt: f64,
) -> Result<Option<Packet>, String> {
    let mut head = 0.0;
    let mut resistance = 1e6;
    let mut pump = 0;
    for &(i, _, direction) in &route.inline {
        match ds[i].kind {
            EquipmentKind::Valve => {
                if ds[i].opening == 0.0 {
                    return Ok(None);
                }
                resistance += 1e6 / ds[i].opening.max(0.001).powi(2);
            }
            EquipmentKind::Pump => {
                if !ds[i].enabled {
                    return Ok(None);
                }
                let sign = direction * if ds[i].pump_reversed { -1 } else { 1 };
                if pump != 0 && pump != sign {
                    return Ok(None);
                }
                pump = sign;
                head += f64::from(sign) * PUMP_PRESSURE_PA;
                resistance += 1e6;
            }
            _ => resistance += 1e6,
        }
    }
    let pressure = |i: usize| {
        states[i].as_ref().map_or(
            if ds[i].kind == EquipmentKind::Tank {
                ATM
            } else {
                0.0
            },
            |s| s.pressure,
        )
    };
    let known_density = |i: usize| -> Option<f64> {
        if ds[i].kind == EquipmentKind::Tank {
            return Some(WATER_DENSITY_KG_M3);
        }
        states[i]
            .as_ref()
            .and_then(|s| s.hydro.as_ref())
            .and_then(|s| s.liquid.as_ref().or(s.vapor.as_ref()))
            .map(|phase| {
                let mass: f64 = phase
                    .amounts
                    .iter()
                    .zip(thermo::MOLAR_MASS_KG)
                    .map(|(n, m)| n * m)
                    .sum();
                mass / (phase.amounts.iter().sum::<f64>() * phase.molar_volume_m3)
            })
    };
    // An empty sealed receiver has no density of its own; a prospective line
    // contains the other endpoint's fluid, rather than an invented water phase.
    let rho_a = known_density(route.a)
        .or_else(|| known_density(route.b))
        .unwrap_or(WATER_DENSITY_KG_M3);
    let rho_b = known_density(route.b)
        .or_else(|| known_density(route.a))
        .unwrap_or(WATER_DENSITY_KG_M3);
    let density = |i: usize| if i == route.a { rho_a } else { rho_b };
    // Zero-holdup lines use a lumped upwind density, not a distributed pipe
    // pressure field. Both nozzles are at y+0.5, so their elevation difference
    // is the origins' difference. A liquid vessel also loads its bottom port.
    let port_pressure = |i: usize| {
        pressure(i)
            + if ds[i].kind == EquipmentKind::Vessel {
                states[i].as_ref().map_or(0.0, |s| {
                    density(i) * G * (s.volume / tank_area_m2(&ds[i]) - 0.5).max(0.0)
                })
            } else {
                0.0
            }
    };
    let elevation = f64::from(ds[route.a].origin[1] - ds[route.b].origin[1]);
    let base = port_pressure(route.a) - port_pressure(route.b) + head;
    let drive = hydrostatic_drive(base, rho_a, rho_b, elevation)?;
    let q = (drive / resistance).clamp(-MAX_FLOW, MAX_FLOW);
    if q == 0.0 || (pump != 0 && q.signum() != f64::from(pump)) {
        return Ok(None);
    }
    let (from, to) = if q > 0.0 {
        (route.a, route.b)
    } else {
        (route.b, route.a)
    };
    let Some(s) = states[from].as_ref() else {
        return Ok(None);
    };
    let mut dv = q.abs() * dt;
    let (amounts, water, h) = if let Some(hydro) = &s.hydro {
        let liquid = hydro.liquid.as_ref();
        if pump != 0 && liquid.is_none() {
            return Ok(None);
        }
        let phase = liquid
            .or(hydro.vapor.as_ref())
            .ok_or("missing donor phase")?;
        let total: f64 = phase.amounts.iter().sum();
        dv = dv.min(total * phase.molar_volume_m3 * 0.01);
        let dn = dv / phase.molar_volume_m3;
        (
            phase.amounts.map(|n| n * dn / total),
            0.0,
            dn * phase.molar_enthalpy_j,
        )
    } else {
        let available = (s.volume - tank_area_m2(&ds[from]) * 0.5).max(0.0);
        let capacity = tank_capacity_m3(&ds[to]) - states[to].as_ref().map_or(0.0, |s| s.volume);
        dv = dv
            .min(available * 0.01)
            .min(capacity)
            .min(water_equilibrium_volume(drive, &ds[from], &ds[to]));
        let dn = dv * WATER_DENSITY_KG_M3 / WATER_MOLAR_MASS_KG;
        (
            [0.0; 2],
            dn,
            ds[from].internal_energy_j * dn / water_moles(&ds[from]),
        )
    };
    if dv <= 0.0 {
        return Ok(None);
    }
    Ok(Some(Packet {
        from,
        to,
        amounts,
        water,
        h,
        base_h: h,
        pressure: s.pressure,
        work: dv * head.abs(),
        rate: dv / dt * q.signum(),
        inline: route.inline,
    }))
}
fn solve_coupled(input: &[EquipmentSnapshot], dt: f64) -> Result<FlowStep, String> {
    let states: Vec<_> = input.iter().map(state).collect::<Result<_, _>>()?;
    let ls = links(input)?;
    let rs = routes(input, &ls)?;
    // A packet has one accepted inlet per slice. Serial exchangers would need a
    // coupled dependency solve; equipment identity must never pick their order.
    if rs.iter().any(|route| {
        route
            .inline
            .iter()
            .filter(|(i, _, _)| input[*i].kind == EquipmentKind::HeatExchanger)
            .count()
            > 1
    }) {
        return Err("multiple exchangers on one fluid route are unsupported".into());
    }
    // Fixed bounded backtracking. Failed proposals never mutate accepted facts.
    let mut last = String::from("coupled step failed");
    for retry in 0..8 {
        match attempt(input, &states, &rs, dt, 0.5_f64.powi(retry)) {
            Ok(step) => return Ok(step),
            Err(e) => last = e,
        }
    }
    Err(last)
}
fn attempt(
    input: &[EquipmentSnapshot],
    states: &[Option<State>],
    routes: &[Route],
    dt: f64,
    scale: f64,
) -> Result<FlowStep, String> {
    let mut packets = vec![];
    for route in routes {
        if let Some(p) = plan(input, states, route.clone(), dt * scale)? {
            packets.push(p);
        }
    }
    let mut heat_readings = BTreeMap::new();
    for (i, d) in input
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == EquipmentKind::HeatExchanger)
    {
        let settings = d.thermal.as_ref().ok_or("missing heat settings")?;
        let mut pair = [None; 2];
        for (pi, p) in packets.iter().enumerate() {
            for &(j, side, _) in &p.inline {
                if i == j && pair[side].replace(pi).is_some() {
                    return Err("ambiguous heat passage".into());
                }
            }
        }
        let mut reading = ThermalReading {
            quality: ReadingQuality::NoFlow,
            control_state: match settings.mode {
                HeatControlMode::Off => ControlState::Off,
                HeatControlMode::Manual => ControlState::Manual,
                HeatControlMode::OutletTemperature => ControlState::Limited,
            },
            ..Default::default()
        };
        for (side, index) in pair.iter().enumerate() {
            if let Some(p) = index.map(|p| &packets[p]) {
                reading.inlet_temperature_k[side] = Some(p.temperature_at(p.h)?);
                reading.outlet_temperature_k[side] = reading.inlet_temperature_k[side];
                let direction = p
                    .inline
                    .iter()
                    .find(|(j, _, _)| *j == i)
                    .map_or(0, |(_, _, direction)| *direction);
                reading.flow_m3_s[side] = p.rate * scale * f64::from(direction);
            }
        }
        if let [Some(a), Some(b)] = pair {
            let ta = packets[a].temperature_at(packets[a].h)?;
            let tb = packets[b].temperature_at(packets[b].h)?;
            reading.quality = ReadingQuality::Available;
            if settings.mode != HeatControlMode::Off && (tb - ta).abs() > 1e-8 {
                // Secant heat capacities include latent heat. Epsilon-NTU bounds
                // finite-UA duty; final PH flashes check the nonlinear outlets.
                let max_a = (packets[a].enthalpy_at(tb)? - packets[a].h).abs();
                let max_b = (packets[b].enthalpy_at(ta)? - packets[b].h).abs();
                let minimum = max_a.min(max_b);
                let maximum = max_a.max(max_b);
                let ratio = if maximum > 0.0 {
                    minimum / maximum
                } else {
                    0.0
                };
                let ntu = if minimum > 0.0 {
                    1500.0 * (tb - ta).abs() * dt * scale / minimum
                } else {
                    0.0
                };
                let direction = |p: &Packet| {
                    p.inline
                        .iter()
                        .find(|(j, _, _)| *j == i)
                        .map_or(0.0, |(_, _, sign)| p.rate.signum() * f64::from(*sign))
                };
                let counter = direction(&packets[a]) != direction(&packets[b]);
                let effectiveness = if counter {
                    if (1.0 - ratio).abs() < 1e-8 {
                        ntu / (1.0 + ntu)
                    } else {
                        let e = (-ntu * (1.0 - ratio)).exp();
                        (1.0 - e) / (1.0 - ratio * e)
                    }
                } else {
                    (1.0 - (-ntu * (1.0 + ratio)).exp()) / (1.0 + ratio)
                };
                let full = minimum * effectiveness;
                let sign = (tb - ta).signum();
                let wanted = match settings.mode {
                    HeatControlMode::Manual => full * settings.duty_fraction,
                    HeatControlMode::OutletTemperature => {
                        let target = settings.target_temperature_k.clamp(ta.min(tb), ta.max(tb));
                        let h = packets[a].enthalpy_at(target)?;
                        let need = ((h - packets[a].h) * sign).max(0.0);
                        reading.control_state =
                            if need <= full && target == settings.target_temperature_k {
                                ControlState::Regulating
                            } else {
                                ControlState::Limited
                            };
                        need.min(full)
                    }
                    HeatControlMode::Off => 0.0,
                };
                let q = sign * wanted;
                packets[a].h += q;
                packets[b].h -= q;
                reading.heat_w = q / dt;
                reading.outlet_temperature_k = [
                    Some(packets[a].temperature_at(packets[a].h)?),
                    Some(packets[b].temperature_at(packets[b].h)?),
                ];
                // The finite stream capacities may be nonlinear near phase change.
                let oa = reading.outlet_temperature_k[0].unwrap();
                let ob = reading.outlet_temperature_k[1].unwrap();
                if oa < ta.min(tb) - 1e-5
                    || oa > ta.max(tb) + 1e-5
                    || ob < ta.min(tb) - 1e-5
                    || ob > ta.max(tb) + 1e-5
                {
                    return Err("heat outlet exceeds inlet bounds".into());
                }
            }
        }
        heat_readings.insert(i, reading);
    }
    let mut ds = input.to_vec();
    let mut rates = vec![0.0; ds.len()];
    let mut work = 0.0;
    for p in &packets {
        let before_pe = potential(&ds[p.from]) + potential(&ds[p.to]);
        if p.water > 0.0 {
            for (i, sign) in [(p.from, -1.0), (p.to, 1.0)] {
                let n = water_moles(&ds[i]) + sign * p.water;
                if n < 0.0 {
                    return Err("depleted water".into());
                }
                ds[i].contents = if n == 0.0 {
                    vec![]
                } else {
                    vec![MaterialAmount {
                        material: WATER_MATERIAL.into(),
                        moles: n,
                    }]
                };
            }
        } else {
            for (i, sign) in [(p.from, -1.0), (p.to, 1.0)] {
                let mut n = thermo::amounts_from_materials(&ds[i].contents)?;
                for (amount, moved) in n.iter_mut().zip(p.amounts) {
                    *amount += sign * moved;
                    if *amount < 0.0 {
                        return Err("depleted component".into());
                    }
                }
                ds[i].contents = thermo::materials_from_amounts(n);
            }
        }
        ds[p.from].internal_energy_j -= p.base_h;
        ds[p.to].internal_energy_j +=
            p.h + p.work + before_pe - potential(&ds[p.from]) - potential(&ds[p.to]);
        work += p.work;
        let signed = p.rate * scale;
        rates[p.from] -= signed.abs();
        rates[p.to] += signed.abs();
        for &(i, _, direction) in &p.inline {
            rates[i] += signed * f64::from(direction);
        }
    }
    for (i, d) in ds.iter_mut().enumerate() {
        let s = if d.contents == input[i].contents
            && d.internal_energy_j == input[i].internal_energy_j
        {
            states[i].clone()
        } else {
            state(d)?
        };
        let thermal = if let Some(r) = heat_readings.remove(&i) {
            Some(r)
        } else if d.kind == EquipmentKind::Vessel {
            let mut r = ThermalReading {
                quality: if s.is_some() {
                    ReadingQuality::Available
                } else {
                    ReadingQuality::NoFlow
                },
                ..Default::default()
            };
            if let Some(h) = s.as_ref().and_then(|s| s.hydro.as_ref()) {
                r.liquid_moles = h.liquid.as_ref().map_or([0.0; 2], |p| p.amounts);
                r.vapor_moles = h.vapor.as_ref().map_or([0.0; 2], |p| p.amounts);
                let n: f64 = r.liquid_moles.iter().chain(&r.vapor_moles).sum();
                r.vapor_fraction = Some(r.vapor_moles.iter().sum::<f64>() / n);
            }
            Some(r)
        } else {
            None
        };
        d.reading = Some(EquipmentReading {
            temperature_k: s.as_ref().map(|s| s.temperature),
            pressure_pa: s.as_ref().map(|s| s.pressure),
            volume_m3: s.as_ref().map_or(0.0, |s| s.volume),
            capacity_m3: if reservoir(d) {
                tank_capacity_m3(d)
            } else {
                0.0
            },
            flow_m3_s: rates[i],
            status: if rates[i] == 0.0 { "idle" } else { "flowing" }.into(),
            thermal,
        });
    }
    let step = FlowStep {
        devices: ds,
        pump_work_j: work,
    };
    audit(input, &step)?;
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: u64, kind: EquipmentKind, origin: [i32; 3]) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id,
            revision: 0,
            kind,
            origin,
            axis: 0,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: true,
            opening: 1.0,
            pump_reversed: false,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            thermal: (kind == EquipmentKind::HeatExchanger)
                .then(exa_protocol::ThermalSettings::default),
            reading: None,
        }
    }
    fn water_exchanger() -> Vec<EquipmentSnapshot> {
        let mut ds = vec![
            device(1, EquipmentKind::Tank, [0, 10, 0]),
            device(2, EquipmentKind::Pump, [2, 10, 0]),
            device(3, EquipmentKind::Pipe, [3, 10, 0]),
            device(4, EquipmentKind::HeatExchanger, [4, 10, 0]),
            device(5, EquipmentKind::Pipe, [6, 10, 0]),
            device(6, EquipmentKind::Tank, [7, 10, 0]),
            device(7, EquipmentKind::Tank, [0, 10, 2]),
            device(8, EquipmentKind::Pump, [2, 10, 2]),
            device(9, EquipmentKind::Pipe, [3, 10, 2]),
            device(10, EquipmentKind::Pipe, [6, 10, 2]),
            device(11, EquipmentKind::Tank, [7, 10, 2]),
        ];
        for (i, t) in [(0, 350.0), (5, 350.0), (6, 295.0), (10, 295.0)] {
            fill_water(&mut ds[i], 4000.0, t).unwrap();
        }
        ds[3].thermal.as_mut().unwrap().mode = HeatControlMode::Manual;
        ds
    }
    #[test]
    fn column_connected_group_preserves_facts_while_other_group_runs() {
        let mut ds = water_exchanger();
        let mut column = device(100, EquipmentKind::Column, [20, 10, 0]);
        column.column_design = Some(exa_protocol::ColumnDesign::default());
        let mut vessel = device(101, EquipmentKind::Vessel, [18, 14, 0]);
        charge(&mut vessel, [1000.0, 1000.0], 320.0).unwrap();
        assert!(exa_simulation::equipment::connected(
            ports(&vessel)[1],
            ports(&column)[0]
        ));
        ds.extend([column.clone(), vessel.clone()]);
        let before = ds[0].contents.clone();
        let step = solve(&ds, 0.2).unwrap();
        assert_ne!(step.devices[0].contents, before);
        for original in [column, vessel] {
            let current = step.devices.iter().find(|d| d.id == original.id).unwrap();
            assert_eq!(current.contents, original.contents);
            assert_eq!(current.internal_energy_j, original.internal_energy_j);
            assert!(current
                .reading
                .as_ref()
                .unwrap()
                .status
                .contains("蒸馏塔尚未运行"));
        }
    }
    #[test]
    fn every_new_planning_device_pauses_its_connected_group_without_touching_facts() {
        for kind in [
            EquipmentKind::RefluxColumn,
            EquipmentKind::Condenser,
            EquipmentKind::Reboiler,
            EquipmentKind::Tee,
        ] {
            let mut devices = water_exchanger();
            let mut planned = device(100, kind, [20, 10, 0]);
            if kind.is_column() {
                planned.column_design = Some(exa_protocol::ColumnDesign::default());
            }
            let connection = ports(&planned)[0];
            let mut source = device(
                101,
                EquipmentKind::Vessel,
                [18, connection.position[1] as i32, 0],
            );
            charge(&mut source, [1000.0, 1000.0], 320.0).unwrap();
            assert!(exa_simulation::equipment::connected(
                ports(&source)[1],
                connection
            ));
            devices.extend([planned.clone(), source.clone()]);
            let before = devices[0].contents.clone();
            let result = solve(&devices, 0.2).unwrap();
            assert_ne!(result.devices[0].contents, before);
            for old in [planned, source] {
                let new = result.devices.iter().find(|d| d.id == old.id).unwrap();
                assert_eq!(new.contents, old.contents);
                assert_eq!(new.internal_energy_j, old.internal_energy_j);
                assert!(new.reading.as_ref().unwrap().status.contains("尚未运行"));
            }
        }
    }
    #[test]
    fn sized_sealed_vessel_uses_its_volume_and_centroid() {
        let mut small = device(1, EquipmentKind::Vessel, [0, 10, 0]);
        let mut large = device(2, EquipmentKind::Vessel, [5, 10, 0]);
        large.tank_size = Some(exa_protocol::TankSize {
            diameter: 4,
            height: 8,
        });
        let ratio = tank_capacity_m3(&large) / tank_capacity_m3(&small);
        charge(&mut small, [1000.0, 1000.0], 320.0).unwrap();
        let external = charge(&mut large, [1000.0 * ratio; 2], 320.0).unwrap();
        let a = state(&small).unwrap().unwrap();
        let b = state(&large).unwrap().unwrap();
        assert!((a.temperature - b.temperature).abs() < 1e-5);
        assert!((a.pressure - b.pressure).abs() < 1.0);
        assert!((large.internal_energy_j / ratio - small.internal_energy_j).abs() < 1.0);
        let mass: f64 = [1000.0 * ratio; 2]
            .into_iter()
            .zip(thermo::MOLAR_MASS_KG)
            .map(|(n, m)| n * m)
            .sum();
        assert!((external - large.internal_energy_j - mass * G * (10.0 + 3.9)).abs() < 1e-6);
        heat(&mut large, 1000.0).unwrap();
        let result = solve(&[large.clone()], 0.2).unwrap();
        assert_eq!(result.devices[0].contents, large.contents);
        assert_eq!(result.devices[0].internal_energy_j, large.internal_energy_j);
        assert_eq!(
            result.devices[0].reading.as_ref().unwrap().capacity_m3,
            tank_capacity_m3(&large)
        );
    }
    #[test]
    fn coupled_water_transport_respects_different_tank_areas() {
        let mut ds = vec![
            device(1, EquipmentKind::Tank, [0, 10, 0]),
            device(2, EquipmentKind::Pipe, [4, 10, 0]),
            device(3, EquipmentKind::Tank, [5, 10, 0]),
        ];
        ds[0].tank_size = Some(exa_protocol::TankSize {
            diameter: 4,
            height: 8,
        });
        // D4 nozzles use local z=1.5; align them with the narrow line at z=0.5.
        ds[0].origin[2] = -1;
        assert!(exa_simulation::equipment::connected(
            ports(&ds[0])[1],
            ports(&ds[1])[0]
        ));
        assert!(exa_simulation::equipment::connected(
            ports(&ds[1])[1],
            ports(&ds[2])[0]
        ));
        let a = tank_area_m2(&ds[0]);
        let b = tank_area_m2(&ds[2]);
        fill_water(&mut ds[0], a * 2000.0, 300.0).unwrap();
        fill_water(&mut ds[2], b * 1000.0, 300.0).unwrap();
        let before = ds.clone();
        for _ in 0..20 {
            ds = solve_coupled(&ds, 0.2).unwrap().devices;
        }
        audit(
            &before,
            &FlowStep {
                devices: ds.clone(),
                pump_work_j: 0.0,
            },
        )
        .unwrap();
        assert!(water_moles(&ds[0]) < water_moles(&before[0]));
        assert!(volume(&ds[0]) / a > volume(&ds[2]) / b);
    }
    #[test]
    fn bent_three_dimensional_heat_route_uses_world_elevation_and_conserves_energy() {
        let mut ds = water_exchanger();
        ds[0].origin[1] = 12;
        ds[1].kind = EquipmentKind::Pipe;
        ds[1].origin = [2, 12, 0];
        ds[1].pipe_ends = Some([0, 2]);
        let mut vertical = device(20, EquipmentKind::Pipe, [2, 11, 0]);
        vertical.axis = 1;
        vertical.pipe_ends = Some([2, 3]);
        let mut elbow = device(21, EquipmentKind::Pipe, [2, 10, 0]);
        elbow.pipe_ends = Some([1, 3]);
        ds.extend([vertical, elbow]);
        let forward = solve(&ds, 0.2).unwrap();
        let mut renumbered = ds.clone();
        for d in &mut renumbered {
            d.id = 100 - d.id;
        }
        let reverse = solve(&renumbered, 0.2).unwrap();
        for actual in &forward.devices {
            let other = reverse
                .devices
                .iter()
                .find(|d| d.origin == actual.origin && d.kind == actual.kind)
                .unwrap();
            assert!((water_moles(actual) - water_moles(other)).abs() < 1e-8);
            if actual.kind == EquipmentKind::HeatExchanger {
                let a = actual.reading.as_ref().unwrap().thermal.as_ref().unwrap();
                let b = other.reading.as_ref().unwrap().thermal.as_ref().unwrap();
                assert!((a.heat_w - b.heat_w).abs() < 1e-8);
                for (qa, qb) in a.flow_m3_s.into_iter().zip(b.flow_m3_s) {
                    assert!((qa - qb).abs() < 1e-12);
                }
            }
        }
        let initial_n: f64 = ds.iter().map(water_moles).sum();
        let initial_e: f64 = ds.iter().map(stored_energy_j).sum();
        let donor = water_moles(&ds[0]);
        let mut work = 0.0;
        for _ in 0..20 {
            let step = solve(&ds, 0.2).unwrap();
            work += step.pump_work_j;
            ds = step.devices;
        }
        assert!(
            water_moles(ds.iter().find(|d| d.id == 1).unwrap()) < donor,
            "gravity must drive the elevated A loop without a pump"
        );
        assert!((ds.iter().map(water_moles).sum::<f64>() - initial_n).abs() < initial_n * 1e-12);
        assert!(
            (ds.iter().map(stored_energy_j).sum::<f64>() - initial_e - work).abs()
                < initial_e.abs() * 1e-10
        );
        assert!(
            ds.iter()
                .find(|d| d.id == 4)
                .unwrap()
                .reading
                .as_ref()
                .unwrap()
                .thermal
                .as_ref()
                .unwrap()
                .heat_w
                < 0.0
        );
        // A static A line with different bottom elevations has equal absolute
        // liquid surfaces. Local nozzle pressures alone would move it uphill.
        let mut equilibrium = water_exchanger();
        equilibrium[0].origin[1] = 11;
        equilibrium[1].enabled = true;
        equilibrium[1].kind = EquipmentKind::Pipe;
        equilibrium[1].origin = [2, 11, 0];
        equilibrium[1].pipe_ends = Some([0, 2]);
        let mut bottom = device(22, EquipmentKind::Pipe, [2, 10, 0]);
        bottom.pipe_ends = Some([1, 3]);
        equilibrium.push(bottom);
        for (index, liters) in [(0, 2000.0), (5, 2000.0 + TANK_AREA_M2 * 1000.0)] {
            equilibrium[index].contents.clear();
            equilibrium[index].internal_energy_j = 0.0;
            fill_water(&mut equilibrium[index], liters, 350.0).unwrap();
        }
        let before = water_moles(&equilibrium[0]);
        let result = solve(&equilibrium, 0.2).unwrap();
        assert!(
            (water_moles(result.devices.iter().find(|d| d.id == 1).unwrap()) - before).abs() < 1e-8
        );
    }
    #[test]
    fn unequal_density_head_requires_one_consistent_donor_independent_of_identity() {
        for (base, a, b, height) in [
            (0.0, 800.0, 500.0, 2.0),
            (100000.0, 800.0, 500.0, -2.0),
            (-100000.0, 800.0, 500.0, 2.0),
        ] {
            let forward = hydrostatic_drive(base, a, b, height).unwrap();
            let reverse = hydrostatic_drive(-base, b, a, -height).unwrap();
            assert!((forward + reverse).abs() < 1e-8);
            let expected = base + if forward > 0.0 { a } else { b } * G * height;
            assert!((forward - expected).abs() < 1e-8);
        }
        for (base, a, b, height) in [
            (6000.0, 1000.0, 100.0, -1.0),
            (-6000.0, 100.0, 1000.0, 1.0),
            (6000.0, 100.0, 1000.0, -1.0),
        ] {
            assert!(hydrostatic_drive(base, a, b, height).is_err());
            assert!(hydrostatic_drive(-base, b, a, -height).is_err());
        }
    }
    #[test]
    fn exchanger_conserves_two_circuits_and_total_energy_with_control_limits() {
        let initial = water_exchanger();
        let step = solve(&initial, 0.2).unwrap();
        let reading = step.devices[3]
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert!(reading.heat_w < 0.0);
        assert_eq!(reading.quality, ReadingQuality::Available);
        assert!(
            reading.outlet_temperature_k[0].unwrap() > reading.outlet_temperature_k[1].unwrap(),
            "finite co-current exchanger cannot cross its outlet temperatures"
        );
        for pair in [[0, 5], [6, 10]] {
            let before: f64 = pair.iter().map(|&i| water_moles(&initial[i])).sum();
            let after: f64 = pair.iter().map(|&i| water_moles(&step.devices[i])).sum();
            assert!((before - after).abs() < 1e-8);
        }
        audit(&initial, &step).unwrap();
        let mut target = initial.clone();
        let settings = target[3].thermal.as_mut().unwrap();
        settings.mode = HeatControlMode::OutletTemperature;
        settings.target_temperature_k = 250.0;
        let step = solve(&target, 0.2).unwrap();
        let r = step.devices[3]
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert_eq!(r.control_state, ControlState::Limited);
        assert!(r.outlet_temperature_k[0].unwrap() >= 295.0);
        target[1].enabled = false;
        let stopped = solve(&target, 0.2).unwrap();
        let r = stopped.devices[3]
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert_eq!(r.heat_w, 0.0);
        assert_eq!(r.quality, ReadingQuality::NoFlow);
    }
    #[test]
    fn bad_island_preserves_inventory_without_stopping_healthy_heat_group() {
        let mut ds = water_exchanger();
        let mut bad = device(99, EquipmentKind::Tank, [20, 10, 0]);
        bad.contents = vec![MaterialAmount {
            material: "unknown:oil".into(),
            moles: 10.0,
        }];
        bad.internal_energy_j = 42.0;
        ds.push(bad.clone());
        let result = solve(&ds, 0.2).unwrap();
        assert!(
            result.devices[3]
                .reading
                .as_ref()
                .unwrap()
                .thermal
                .as_ref()
                .unwrap()
                .heat_w
                < 0.0
        );
        let paused = result.devices.last().unwrap();
        assert_eq!(paused.contents, bad.contents);
        assert_eq!(paused.internal_energy_j, 42.0);
        assert!(paused.reading.as_ref().unwrap().status.starts_with("暂停"));
    }

    #[test]
    fn exchanger_axis_readings_do_not_depend_on_tank_identity_order() {
        let ds = water_exchanger();
        let reference = solve(&ds, 0.2).unwrap();
        let expected = reference
            .devices
            .iter()
            .find(|d| d.id == 4)
            .unwrap()
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        let mut reversed_ids = ds;
        reversed_ids[0].id = 100;
        reversed_ids[6].id = 101;
        let actual = solve(&reversed_ids, 0.2).unwrap();
        let actual = actual
            .devices
            .iter()
            .find(|d| d.id == 4)
            .unwrap()
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert_eq!(actual.flow_m3_s, expected.flow_m3_s);
        assert!(actual.flow_m3_s.iter().all(|q| *q > 0.0));
        assert!((actual.heat_w - expected.heat_w).abs() < 1e-8);
    }

    #[test]
    fn serial_exchangers_pause_without_identity_dependent_transport() {
        let mut ds = water_exchanger();
        ds[4].kind = EquipmentKind::HeatExchanger;
        ds[4].thermal = Some(exa_protocol::ThermalSettings {
            mode: HeatControlMode::Manual,
            ..Default::default()
        });
        ds[5].origin[0] = 8;
        ds[10].origin[0] = 8;
        ds.remove(9);
        for reorder in [false, true] {
            if reorder {
                ds[3].id = 90;
                ds[4].id = 89;
            }
            let step = solve(&ds, 0.2).unwrap();
            assert_eq!(step.pump_work_j, 0.0);
            for original in &ds {
                let actual = step.devices.iter().find(|d| d.id == original.id).unwrap();
                assert_eq!(actual.contents, original.contents);
                assert_eq!(actual.internal_energy_j, original.internal_energy_j);
                assert!(actual
                    .reading
                    .as_ref()
                    .unwrap()
                    .status
                    .contains("multiple exchangers"));
            }
        }
    }
    #[test]
    fn sealed_charge_heat_and_drain_facts_are_finite_and_failed_heat_is_atomic() {
        let mut vessel = device(1, EquipmentKind::Vessel, [0, 10, 0]);
        let supplied = charge(&mut vessel, [1000.0, 1000.0], 320.0).unwrap();
        assert_eq!(supplied, stored_energy_j(&vessel));
        let before = vessel.clone();
        assert!(charge(&mut vessel, [1.0, 1.0], 320.0).is_err());
        assert!(heat(&mut vessel, 1e15).is_err());
        assert_eq!(vessel, before);
        heat(&mut vessel, 1000.0).unwrap();
        assert!((stored_energy_j(&vessel) - supplied - 1000.0).abs() < 1e-7);
        let step = solve(&[vessel], 0.2).unwrap();
        let r = step.devices[0]
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert!(r.vapor_fraction.is_some());
        assert_eq!(r.quality, ReadingQuality::Available);
    }
    #[test]
    fn hydrocarbon_water_exchanger_transports_phases_without_cross_contamination() {
        let mut ds = water_exchanger();
        for i in [0, 5] {
            ds[i].contents.clear();
            ds[i].internal_energy_j = 0.0;
            ds[i].kind = EquipmentKind::Vessel;
            charge(&mut ds[i], [1000.0, 1000.0], 320.0).unwrap();
        }
        let before = ds.clone();
        let result = solve(&ds, 0.2).unwrap();
        assert!(
            result
                .devices
                .iter()
                .all(|d| !d.reading.as_ref().unwrap().status.starts_with("暂停")),
            "{:?}",
            result
                .devices
                .iter()
                .map(|d| &d.reading)
                .collect::<Vec<_>>()
        );
        let r = result.devices[3]
            .reading
            .as_ref()
            .unwrap()
            .thermal
            .as_ref()
            .unwrap();
        assert!(r.heat_w < 0.0, "{:?}", r);
        assert_eq!(r.quality, ReadingQuality::Available);
        assert!(thermo::amounts_from_materials(&result.devices[0].contents).unwrap()[0] < 1000.0);
        for component in 0..2 {
            let total: f64 = [0, 5]
                .iter()
                .map(|&i| {
                    thermo::amounts_from_materials(&result.devices[i].contents).unwrap()[component]
                })
                .sum();
            assert!((total - 2000.0).abs() < 1e-8);
        }
        for i in [6, 10] {
            assert!(result.devices[i]
                .contents
                .iter()
                .all(|c| c.material == WATER_MATERIAL));
        }
        audit(&before, &result).unwrap();
        // A pump is not a compressor: no material crosses a pure-gas suction.
        for i in [0, 5] {
            ds[i].contents.clear();
            ds[i].internal_energy_j = 0.0;
            charge(&mut ds[i], [10.0, 10.0], 320.0).unwrap();
        }
        let gas = solve(&ds, 0.2).unwrap();
        assert_eq!(gas.devices[0].contents, ds[0].contents);
        assert_eq!(
            gas.devices[3]
                .reading
                .as_ref()
                .unwrap()
                .thermal
                .as_ref()
                .unwrap()
                .heat_w,
            0.0
        );
    }
}

//! Limited real-fluid package: PR78 n-butane/n-pentane, classical mixing, kij=0.0174.
//!
//! The ideal-gas enthalpy datum is zero at 298.15 K for each component. This is NOT
//! the legacy water datum. No aqueous mixing, solids, critical-region continuation,
//! chemistry, or claim of parity with the complete IChemE property package.
//! See assets/thermo/README.md for data provenance and numerical qualification.
use exa_protocol::MaterialAmount;
use serde::Deserialize;
use std::sync::OnceLock;

pub const BUTANE_MATERIAL: &str = "icheme:n_butane";
pub const PENTANE_MATERIAL: &str = "icheme:n_pentane";
pub const MATERIAL_IDS: [&str; 2] = [BUTANE_MATERIAL, PENTANE_MATERIAL];
pub const MOLAR_MASS_KG: [f64; 2] = [0.0581222, 0.07214878];
pub const MODEL_REVISION: &str = "butane-pentane-pr78-cp298-r1";
pub const MIN_TEMPERATURE_K: f64 = 293.15;
pub const MAX_TEMPERATURE_K: f64 = 400.0;
pub const MIN_PRESSURE_PA: f64 = 1.0;
pub const MAX_PRESSURE_PA: f64 = 2_000_000.0;
const R: f64 = 8.31446261815324;
const KIJ: f64 = 0.0174;
const SQRT_2: f64 = std::f64::consts::SQRT_2;

#[derive(Clone, Debug)]
pub struct Phase {
    pub amounts: [f64; 2],
    pub molar_volume_m3: f64,
    pub molar_enthalpy_j: f64,
    pub molar_internal_energy_j: f64,
}
impl Phase {
    pub fn moles(&self) -> f64 {
        self.amounts.iter().sum()
    }
    pub fn volume_m3(&self) -> f64 {
        self.moles() * self.molar_volume_m3
    }
    pub fn internal_energy_j(&self) -> f64 {
        self.moles() * self.molar_internal_energy_j
    }
    pub fn enthalpy_j(&self) -> f64 {
        self.moles() * self.molar_enthalpy_j
    }
}

#[derive(Clone, Debug)]
pub struct ThermoState {
    pub temperature_k: f64,
    pub pressure_pa: f64,
    pub liquid: Option<Phase>,
    pub vapor: Option<Phase>,
    pub internal_energy_j: f64,
    pub volume_m3: f64,
    pub enthalpy_j: f64,
}
impl ThermoState {
    pub fn vapor_fraction(&self) -> f64 {
        let gas = self.vapor.as_ref().map_or(0.0, Phase::moles);
        gas / (gas + self.liquid.as_ref().map_or(0.0, Phase::moles))
    }
}

pub fn amounts_from_materials(materials: &[MaterialAmount]) -> Result<[f64; 2], String> {
    let mut result = [0.0; 2];
    for material in materials {
        let i = MATERIAL_IDS
            .iter()
            .position(|id| *id == material.material)
            .ok_or("unsupported material in hydrocarbon package")?;
        if !material.moles.is_finite() || material.moles < 0.0 {
            return Err("invalid material amount".into());
        }
        result[i] += material.moles;
    }
    if result.iter().any(|n| !n.is_finite()) {
        return Err("material amount overflow".into());
    }
    Ok(result)
}

pub fn materials_from_amounts(amounts: [f64; 2]) -> Vec<MaterialAmount> {
    amounts
        .into_iter()
        .enumerate()
        .filter(|(_, n)| *n > 0.0)
        .map(|(i, moles)| MaterialAmount {
            material: MATERIAL_IDS[i].into(),
            moles,
        })
        .collect()
}

#[derive(Deserialize)]
struct Property {
    ideal_gas_cp: HeatCapacity,
    models: Models,
}
#[derive(Deserialize)]
struct HeatCapacity {
    reference_kelvin: f64,
    coefficients: [f64; 6],
}
#[derive(Deserialize)]
struct Models {
    pr78: Critical,
}
#[derive(Deserialize)]
struct Critical {
    critical_temperature_kelvin: f64,
    critical_pressure_pascal: f64,
    acentric_factor: f64,
}
fn properties() -> Result<&'static [Property; 2], String> {
    static DATA: OnceLock<Result<[Property; 2], String>> = OnceLock::new();
    DATA.get_or_init(|| {
        Ok([
            serde_json::from_str(include_str!("../assets/thermo/n_butane.json"))
                .map_err(|e| format!("butane property data: {e}"))?,
            serde_json::from_str(include_str!("../assets/thermo/n_pentane.json"))
                .map_err(|e| format!("pentane property data: {e}"))?,
        ])
    })
    .as_ref()
    .map_err(Clone::clone)
}

#[derive(Clone, Copy, Debug)]
struct Evaluation {
    volume: f64,
    enthalpy: f64,
    ln_phi: [f64; 2],
    // For a single subcritical root, the critical covolume separates its branch.
    liquid_like: bool,
}

fn normalize(amounts: [f64; 2]) -> Result<([f64; 2], f64), String> {
    let total: f64 = amounts.iter().sum();
    if amounts.iter().any(|n| !n.is_finite() || *n < 0.0) || !total.is_finite() || total <= 0.0 {
        return Err("thermodynamic solve requires positive finite material".into());
    }
    Ok((amounts.map(|n| n / total), total))
}

fn check_temperature(t: f64) -> Result<(), String> {
    if !t.is_finite() || !(MIN_TEMPERATURE_K..=MAX_TEMPERATURE_K).contains(&t) {
        return Err("hydrocarbon temperature outside supported 293.15–400 K range".into());
    }
    Ok(())
}
fn check_pressure(p: f64) -> Result<(), String> {
    if !p.is_finite() || !(MIN_PRESSURE_PA..=MAX_PRESSURE_PA).contains(&p) {
        return Err("hydrocarbon pressure outside supported 1 Pa–2 MPa range".into());
    }
    Ok(())
}

// Independent implementation of the PR cubic; Newton polishing is kept within
// adjacent stationary points to avoid moving a liquid root to the vapour branch.
fn cubic_roots(a: f64, b: f64) -> Vec<f64> {
    let c2 = b - 1.0;
    let c1 = a - 3.0 * b * b - 2.0 * b;
    let c0 = -a * b + b * b + b * b * b;
    let q = c1 - c2 * c2 / 3.0;
    let r = 2.0 * c2.powi(3) / 27.0 - c2 * c1 / 3.0 + c0;
    let discriminant = r * r / 4.0 + q.powi(3) / 27.0;
    let mut roots = if discriminant >= 0.0 {
        let s = (-r / 2.0 + discriminant.sqrt().copysign(-r)).cbrt();
        let z = if s != 0.0 { s - q / (3.0 * s) } else { 0.0 };
        vec![z - c2 / 3.0]
    } else {
        let scale = 2.0 * (-q / 3.0).sqrt();
        let angle = ((3.0 * r / (2.0 * q)) * (-3.0 / q).sqrt())
            .clamp(-1.0, 1.0)
            .acos()
            / 3.0;
        (0..3)
            .map(|i| {
                scale * (angle - f64::from(i) * 2.0 * std::f64::consts::PI / 3.0).cos() - c2 / 3.0
            })
            .collect()
    };
    let extrema = (c2 * c2 - 3.0 * c1).max(0.0).sqrt();
    let turning = [(-c2 - extrema) / 3.0, (-c2 + extrema) / 3.0];
    for root in &mut roots {
        for _ in 0..3 {
            let derivative = (3.0 * *root + 2.0 * c2) * *root + c1;
            if derivative.abs() < 1e-12 {
                break;
            }
            let next = *root - (((*root + c2) * *root + c1) * *root + c0) / derivative;
            if turning.iter().any(|s| (*root - s) * (next - s) < 0.0) {
                break;
            }
            *root = next;
        }
    }
    roots.retain(|z| z.is_finite() && *z > b);
    roots.sort_by(f64::total_cmp);
    roots.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
    roots
}

type PureParameters = ([f64; 2], [f64; 2], [f64; 2]);

fn pure_parameters(t: f64) -> Result<PureParameters, String> {
    let mut a = [0.0; 2];
    let mut da = [0.0; 2];
    let mut b = [0.0; 2];
    for (i, property) in properties()?.iter().enumerate() {
        let c = &property.models.pr78;
        let tc = c.critical_temperature_kelvin;
        let pc = c.critical_pressure_pascal;
        let w = c.acentric_factor;
        let kappa = 0.37464 + 1.54226 * w - 0.26992 * w * w;
        let alpha_root = 1.0 + kappa * (1.0 - (t / tc).sqrt());
        let ac = 0.45724 * R * R * tc * tc / pc;
        a[i] = ac * alpha_root * alpha_root;
        da[i] = -ac * kappa * alpha_root / (t * tc).sqrt();
        b[i] = 0.07780 * R * tc / pc;
    }
    Ok((a, da, b))
}

struct MixtureParameters {
    a: f64,
    da: f64,
    b: f64,
    pure_b: [f64; 2],
    sum_a: [f64; 2],
    ideal_h: f64,
}

fn mixture_parameters(t: f64, x: [f64; 2]) -> Result<MixtureParameters, String> {
    let (a, da, b) = pure_parameters(t)?;
    let cross = (a[0] * a[1]).sqrt() * (1.0 - KIJ);
    let cross_dt = cross * 0.5 * (da[0] / a[0] + da[1] / a[1]);
    let am = x[0] * x[0] * a[0] + 2.0 * x[0] * x[1] * cross + x[1] * x[1] * a[1];
    let dam = x[0] * x[0] * da[0] + 2.0 * x[0] * x[1] * cross_dt + x[1] * x[1] * da[1];
    let bm = x[0] * b[0] + x[1] * b[1];
    let sum_a = [x[0] * a[0] + x[1] * cross, x[0] * cross + x[1] * a[1]];
    let ideal_h: f64 = properties()?
        .iter()
        .enumerate()
        .map(|(i, property)| {
            let dt = t - property.ideal_gas_cp.reference_kelvin;
            x[i] * property
                .ideal_gas_cp
                .coefficients
                .iter()
                .enumerate()
                .map(|(power, coefficient)| {
                    coefficient * dt.powi(power as i32 + 1) / (power + 1) as f64
                })
                .sum::<f64>()
        })
        .sum();
    Ok(MixtureParameters {
        a: am,
        da: dam,
        b: bm,
        pure_b: b,
        sum_a,
        ideal_h,
    })
}

fn evaluations(t: f64, p: f64, x: [f64; 2]) -> Result<Vec<Evaluation>, String> {
    let MixtureParameters {
        a: am,
        da: dam,
        b: bm,
        pure_b: b,
        sum_a,
        ideal_h,
    } = mixture_parameters(t, x)?;
    let ar = am * p / (R * R * t * t);
    let br = bm * p / (R * t);
    let roots = cubic_roots(ar, br);
    if roots.is_empty() {
        return Err("PR78 has no physical root".into());
    }
    // The middle root is mechanically unstable and is never an admitted phase.
    let selected = [roots[0], *roots.last().ok_or("missing PR78 root")?];
    let mut result = Vec::with_capacity(2);
    for (branch, z) in selected.into_iter().enumerate() {
        if branch == 1 && (z - selected[0]).abs() < 1e-10 {
            continue;
        }
        let logarithm = ((z + (1.0 + SQRT_2) * br) / (z + (1.0 - SQRT_2) * br)).ln();
        let ln_phi = std::array::from_fn(|i| {
            b[i] / bm * (z - 1.0)
                - (z - br).ln()
                - ar / (2.0 * SQRT_2 * br) * (2.0 * sum_a[i] / am - b[i] / bm) * logarithm
        });
        let enthalpy =
            ideal_h + R * t * (z - 1.0) + (t * dam - am) / (2.0 * SQRT_2 * bm) * logarithm;
        let volume = z * R * t / p;
        if !enthalpy.is_finite() || ln_phi.iter().any(|f| !f.is_finite()) || volume <= 0.0 {
            return Err("invalid PR78 phase".into());
        }
        result.push(Evaluation {
            volume,
            enthalpy,
            ln_phi,
            liquid_like: if roots.len() > 1 {
                branch == 0
            } else {
                volume < 3.95 * bm
            },
        });
    }
    Ok(result)
}

fn phase(e: Evaluation, amounts: [f64; 2], p: f64) -> Phase {
    Phase {
        amounts,
        molar_volume_m3: e.volume,
        molar_enthalpy_j: e.enthalpy,
        molar_internal_energy_j: e.enthalpy - p * e.volume,
    }
}
fn state(
    t: f64,
    p: f64,
    liquid: Option<Phase>,
    vapor: Option<Phase>,
) -> Result<ThermoState, String> {
    let mut result = ThermoState {
        temperature_k: t,
        pressure_pa: p,
        liquid,
        vapor,
        internal_energy_j: 0.0,
        volume_m3: 0.0,
        enthalpy_j: 0.0,
    };
    for phase in [&result.liquid, &result.vapor].into_iter().flatten() {
        result.internal_energy_j += phase.internal_energy_j();
        result.volume_m3 += phase.volume_m3();
        result.enthalpy_j += phase.enthalpy_j();
    }
    if !result.internal_energy_j.is_finite()
        || !result.enthalpy_j.is_finite()
        || !result.volume_m3.is_finite()
        || result.volume_m3 <= 0.0
    {
        return Err("non-finite thermodynamic totals".into());
    }
    Ok(result)
}
fn single(t: f64, p: f64, amounts: [f64; 2], e: Evaluation) -> Result<ThermoState, String> {
    if e.liquid_like {
        state(t, p, Some(phase(e, amounts, p)), None)
    } else {
        state(t, p, None, Some(phase(e, amounts, p)))
    }
}
fn gibbs(e: Evaluation, x: [f64; 2]) -> f64 {
    x[0] * e.ln_phi[0] + x[1] * e.ln_phi[1]
}
fn best(evaluations: &[Evaluation], x: [f64; 2]) -> Evaluation {
    *evaluations
        .iter()
        .min_by(|a, b| gibbs(**a, x).total_cmp(&gibbs(**b, x)))
        .expect("evaluations always returns at least one root")
}

/// A specified PR branch used by the steady equilibrium-stage design calculator.
/// This does not perform a flash or change the energy reference of the package.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DesignPhase {
    pub enthalpy_j_mol: f64,
    pub ln_phi: [f64; 2],
    pub volume_m3_mol: f64,
}

pub(crate) fn design_phase(t: f64, p: f64, x0: f64, vapor: bool) -> Result<DesignPhase, String> {
    check_temperature(t)?;
    check_pressure(p)?;
    if !x0.is_finite() || !(0.0..=1.0).contains(&x0) {
        return Err("invalid design phase composition".into());
    }
    let roots = evaluations(t, p, [x0, 1.0 - x0])?;
    let e = roots
        .iter()
        .find(|e| e.liquid_like != vapor)
        .ok_or("specified liquid/vapor PR branch is unavailable")?;
    Ok(DesignPhase {
        enthalpy_j_mol: e.enthalpy,
        ln_phi: e.ln_phi,
        volume_m3_mol: e.volume,
    })
}

pub(crate) fn design_phase_stable(t: f64, p: f64, x0: f64, vapor: bool) -> Result<bool, String> {
    let x = [x0, 1.0 - x0];
    let roots = evaluations(t, p, x)?;
    let e = *roots
        .iter()
        .find(|e| e.liquid_like != vapor)
        .ok_or("specified liquid/vapor PR branch is unavailable")?;
    Ok(stability(t, p, x, e)? >= -2e-8)
}

/// Wilson estimate is used only to initialize the rigorous phi-phi bubble solve.
pub(crate) fn design_wilson_k(t: f64, p: f64) -> Result<[f64; 2], String> {
    let data = properties()?;
    Ok(std::array::from_fn(|i| {
        let c = &data[i].models.pr78;
        c.critical_pressure_pascal / p
            * (5.373 * (1.0 + c.acentric_factor) * (1.0 - c.critical_temperature_kelvin / t)).exp()
    }))
}

// Binary tangent-plane distance. A deterministic coarse grid plus bounded local
// minimizations checks both roots, including trace-rich endpoints. A negative
// minimum proves instability; unresolved flash/product stability is an error.
fn stability(t: f64, p: f64, z: [f64; 2], feed: Evaluation) -> Result<f64, String> {
    let chemical = [z[0].ln() + feed.ln_phi[0], z[1].ln() + feed.ln_phi[1]];
    let objective = |x0: f64| -> Result<f64, String> {
        let x = [x0, 1.0 - x0];
        let e = best(&evaluations(t, p, x)?, x);
        Ok((0..2)
            .filter(|i| x[*i] > 0.0)
            .map(|i| x[i] * (x[i].ln() + e.ln_phi[i] - chemical[i]))
            .sum())
    };
    let mut grid = vec![0.0, 1e-10, 1e-6, z[0], 1.0 - 1e-6, 1.0 - 1e-10, 1.0];
    grid.extend((1..16).map(|i| f64::from(i) / 16.0));
    grid.sort_by(f64::total_cmp);
    grid.dedup_by(|a, b| *a == *b);
    let values: Vec<f64> = grid
        .iter()
        .map(|x| objective(*x))
        .collect::<Result<_, _>>()?;
    let mut minimum = values.iter().copied().fold(0.0, f64::min);
    for i in 1..grid.len() - 1 {
        if values[i] > values[i - 1] || values[i] > values[i + 1] {
            continue;
        }
        let (mut lo, mut hi) = (grid[i - 1], grid[i + 1]);
        for _ in 0..32 {
            let a = lo + (hi - lo) * 0.3819660112501051;
            let b = hi - (hi - lo) * 0.3819660112501051;
            let fa = objective(a)?;
            let fb = objective(b)?;
            minimum = minimum.min(fa).min(fb);
            if fa < fb {
                hi = b;
            } else {
                lo = a;
            }
        }
    }
    Ok(minimum)
}

fn vapor_fraction(z: [f64; 2], k: [f64; 2]) -> f64 {
    let rr = |beta: f64| {
        (0..2)
            .map(|i| z[i] * (k[i] - 1.0) / (1.0 + beta * (k[i] - 1.0)))
            .sum::<f64>()
    };
    if rr(0.0) <= 0.0 {
        return 0.0;
    }
    if rr(1.0) >= 0.0 {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..48 {
        let mid = (lo + hi) * 0.5;
        if rr(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5
}

/// Temperature/pressure flash. At exact pure saturation TP cannot specify the
/// phase amount; the lower-Gibbs root is returned. Use PH/TV/UV for coexistence.
pub fn tp(t: f64, p: f64, amounts: [f64; 2]) -> Result<ThermoState, String> {
    check_temperature(t)?;
    check_pressure(p)?;
    let (z, total) = normalize(amounts)?;
    let feed = best(&evaluations(t, p, z)?, z);
    if z.contains(&0.0) || stability(t, p, z, feed)? >= -1e-8 {
        return single(t, p, amounts, feed);
    }
    let mut ln_k = [0.0; 2];
    for (i, property) in properties()?.iter().enumerate() {
        let c = &property.models.pr78;
        ln_k[i] = (c.critical_pressure_pascal / p).ln()
            + 5.373 * (1.0 + c.acentric_factor) * (1.0 - c.critical_temperature_kelvin / t);
    }
    for _ in 0..160 {
        let k = ln_k.map(|v| v.clamp(-40.0, 40.0).exp());
        let beta = vapor_fraction(z, k);
        let x_raw = std::array::from_fn(|i| z[i] / (1.0 + beta * (k[i] - 1.0)));
        let (x, _) = normalize(x_raw)?;
        let (y, _) = normalize(std::array::from_fn(|i| x[i] * k[i]))?;
        let liquid = evaluations(t, p, x)?[0];
        let vapor = *evaluations(t, p, y)?.last().ok_or("missing vapor root")?;
        let next = std::array::from_fn::<_, 2, _>(|i| liquid.ln_phi[i] - vapor.ln_phi[i]);
        let error = (0..2)
            .map(|i| (next[i] - ln_k[i]).abs())
            .fold(0.0, f64::max);
        if error < 1e-10 {
            if beta <= 0.0 || beta >= 1.0 || (x[0] - y[0]).abs() < 1e-8 {
                return Err("PR78 flash collapsed to unstable feed".into());
            }
            if stability(t, p, x, liquid)? < -2e-7 || stability(t, p, y, vapor)? < -2e-7 {
                return Err("PR78 flash products failed tangent-plane stability".into());
            }
            let gas = std::array::from_fn(|i| total * beta * y[i]);
            // The subtraction preserves each component exactly, not just total moles.
            let condensed = std::array::from_fn(|i| amounts[i] - gas[i]);
            if condensed.iter().any(|n| *n < 0.0) {
                return Err("PR78 flash component balance failed".into());
            }
            return state(
                t,
                p,
                Some(phase(liquid, condensed, p)),
                Some(phase(vapor, gas, p)),
            );
        }
        for i in 0..2 {
            ln_k[i] += 0.8 * (next[i] - ln_k[i]);
        }
    }
    Err("PR78 TP flash did not converge within 160 iterations".into())
}

// Sign for a pure subcritical phase: liquid is stable above Ps, vapour below Ps.
fn saturation_sign(t: f64, p: f64, component: usize) -> Result<f64, String> {
    let mut x = [0.0; 2];
    x[component] = 1.0;
    let roots = evaluations(t, p, x)?;
    Ok(if roots.len() == 2 {
        roots[0].ln_phi[component] - roots[1].ln_phi[component]
    } else if roots[0].liquid_like {
        -1.0
    } else {
        1.0
    })
}
fn saturation_pressure(t: f64, component: usize) -> Result<Option<f64>, String> {
    let (mut lo, mut hi) = (MIN_PRESSURE_PA.ln(), MAX_PRESSURE_PA.ln());
    if saturation_sign(t, MIN_PRESSURE_PA, component)? < 0.0
        || saturation_sign(t, MAX_PRESSURE_PA, component)? > 0.0
    {
        return Ok(None);
    }
    for _ in 0..48 {
        let mid = (lo + hi) * 0.5;
        if saturation_sign(t, mid.exp(), component)? > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(Some(((lo + hi) * 0.5).exp()))
}
fn pure_coexistence(t: f64, p: f64, amounts: [f64; 2], beta: f64) -> Result<ThermoState, String> {
    let (x, _) = normalize(amounts)?;
    let roots = evaluations(t, p, x)?;
    if roots.len() != 2 || !(0.0..=1.0).contains(&beta) {
        return Err("pure coexistence requires distinct physical roots".into());
    }
    state(
        t,
        p,
        (beta < 1.0).then(|| phase(roots[0], amounts.map(|n| n * (1.0 - beta)), p)),
        (beta > 0.0).then(|| phase(roots[1], amounts.map(|n| n * beta), p)),
    )
}

/// Equilibrium in a rigid vessel at a specified temperature. Includes pure
/// coexistence (volume lever rule), which a pressure-only root cannot express.
pub fn tv(t: f64, volume: f64, amounts: [f64; 2]) -> Result<ThermoState, String> {
    check_temperature(t)?;
    let (z, total) = normalize(amounts)?;
    if !volume.is_finite() || volume <= 0.0 {
        return Err("positive finite vessel volume required".into());
    }
    if z.contains(&0.0) {
        let i = usize::from(z[0] == 0.0);
        let v = volume / total;
        if let Some(p) = saturation_pressure(t, i)? {
            let roots = evaluations(t, p, z)?;
            if roots.len() == 2 && v >= roots[0].volume && v <= roots[1].volume {
                return pure_coexistence(
                    t,
                    p,
                    amounts,
                    (v - roots[0].volume) / (roots[1].volume - roots[0].volume),
                );
            }
        }
        let (a, _, b) = pure_parameters(t)?;
        if v <= b[i] {
            return Err("vessel volume below PR78 covolume".into());
        }
        let p = R * t / (v - b[i]) - a[i] / (v * (v + b[i]) + b[i] * (v - b[i]));
        check_pressure(p)?;
        let answer = tp(t, p, amounts)?;
        if (answer.volume_m3 / volume - 1.0).abs() > 1e-7 {
            return Err("requested pure TV state is unstable".into());
        }
        return Ok(answer);
    }
    let (mut lo, mut hi) = (MIN_PRESSURE_PA.ln(), MAX_PRESSURE_PA.ln());
    let low = tp(t, MIN_PRESSURE_PA, amounts)?;
    let high = tp(t, MAX_PRESSURE_PA, amounts)?;
    if volume > low.volume_m3 * (1.0 + 1e-9) || volume < high.volume_m3 * (1.0 - 1e-9) {
        return Err("requested TV state needs pressure outside package range".into());
    }
    let (mut low_volume, mut high_volume) = (low.volume_m3.ln(), high.volume_m3.ln());
    let target_volume = volume.ln();
    for _ in 0..44 {
        let fraction = ((low_volume - target_volume) / (low_volume - high_volume)).clamp(0.1, 0.9);
        let mid = lo + fraction * (hi - lo);
        let candidate = tp(t, mid.exp(), amounts)?;
        if (candidate.volume_m3 / volume - 1.0).abs() < 2e-9 {
            return Ok(candidate);
        }
        if candidate.volume_m3 > volume {
            lo = mid;
            low_volume = candidate.volume_m3.ln();
        } else {
            hi = mid;
            high_volume = candidate.volume_m3.ln();
        }
    }
    Err("PR78 TV pressure inversion did not converge".into())
}

fn inverse_temperature(
    target: f64,
    total: f64,
    evaluate: impl Fn(f64) -> Result<ThermoState, String>,
    value: impl Fn(&ThermoState) -> f64,
) -> Result<ThermoState, String> {
    if !target.is_finite() {
        return Err("finite target energy required".into());
    }
    let tolerance = 2e-9 * target.abs().max(total * 1000.0);
    let mut previous: Option<(f64, f64)> = None;
    // Capability errors at the endpoints need not preclude an interior state,
    // e.g. a dense liquid reaches the pressure ceiling before the T ceiling.
    for segment in 0..=20 {
        let t =
            MIN_TEMPERATURE_K + (MAX_TEMPERATURE_K - MIN_TEMPERATURE_K) * f64::from(segment) / 20.0;
        let Ok(candidate) = evaluate(t) else {
            previous = None;
            continue;
        };
        let error = value(&candidate) - target;
        if error.abs() <= tolerance {
            return Ok(candidate);
        }
        if let Some((previous_t, previous_error)) = previous.filter(|(_, e)| e * error < 0.0) {
            let (mut lo, mut hi) = (previous_t, t);
            let mut low_error = previous_error;
            for _ in 0..40 {
                let mid = (lo + hi) * 0.5;
                let result = evaluate(mid)?;
                let e = value(&result) - target;
                if e.abs() <= tolerance {
                    return Ok(result);
                }
                if e * low_error > 0.0 {
                    lo = mid;
                    low_error = e;
                } else {
                    hi = mid;
                }
            }
            return Err("thermodynamic energy inversion did not converge".into());
        }
        previous = Some((t, error));
    }
    Err("energy/volume has no resolved state inside the supported package range".into())
}

pub fn uv(energy: f64, volume: f64, amounts: [f64; 2]) -> Result<ThermoState, String> {
    let (z, total) = normalize(amounts)?;
    if !energy.is_finite() || !volume.is_finite() || volume <= 0.0 {
        return Err("finite energy and positive finite volume required".into());
    }
    // At fixed volume a homogeneous PR state has an explicit caloric equation.
    // This also resolves dense liquid states whose admitted pressure interval is
    // narrower than a temperature search grid. It is accepted ONLY after the TP
    // flash confirms phase stability and the original U/V specifications.
    if let Ok(result) = uv_homogeneous(energy, volume, amounts, z, total) {
        return Ok(result);
    }
    // A coupled Newton finish avoids dozens of nested pressure flashes in the
    // ordinary binary case. It only accepts converged, stable TP answers. Pure
    // coexistence is rank-deficient in TP and deliberately uses TV lever rules.
    if !z.contains(&0.0) {
        if let Ok(result) = uv_newton(energy, volume, amounts, total) {
            return Ok(result);
        }
    }
    inverse_temperature(
        energy,
        total,
        |t| tv(t, volume, amounts),
        |s| s.internal_energy_j,
    )
}

fn uv_homogeneous(
    energy: f64,
    volume: f64,
    amounts: [f64; 2],
    z: [f64; 2],
    total: f64,
) -> Result<ThermoState, String> {
    let v = volume / total;
    let evaluate = |t: f64| -> Result<(f64, f64), String> {
        let m = mixture_parameters(t, z)?;
        if v <= m.b {
            return Err("volume below mixture covolume".into());
        }
        let p = R * t / (v - m.b) - m.a / (v * (v + m.b) + m.b * (v - m.b));
        let logarithm = ((v + (1.0 + SQRT_2) * m.b) / (v + (1.0 - SQRT_2) * m.b)).ln();
        let u = m.ideal_h - R * t + (t * m.da - m.a) / (2.0 * SQRT_2 * m.b) * logarithm;
        Ok((u * total, p))
    };
    let (mut lo, mut hi) = (MIN_TEMPERATURE_K, MAX_TEMPERATURE_K);
    if energy < evaluate(lo)?.0 || energy > evaluate(hi)?.0 {
        return Err("homogeneous energy outside temperature bracket".into());
    }
    for _ in 0..44 {
        let t = (lo + hi) * 0.5;
        let (u, p) = evaluate(t)?;
        if (u - energy).abs() <= 1e-10 * energy.abs().max(total * 1000.0) {
            // Inverting an exactly-on-boundary state may miss by a few ulps.
            // Only roundoff-sized pressure excursions are clipped; the final
            // independent volume/energy checks still have to pass.
            let p =
                if (MIN_PRESSURE_PA * (1.0 - 1e-9)..=MAX_PRESSURE_PA * (1.0 + 1e-9)).contains(&p) {
                    p.clamp(MIN_PRESSURE_PA, MAX_PRESSURE_PA)
                } else {
                    p
                };
            let result = tp(t, p, amounts)?;
            if (result.volume_m3 / volume - 1.0).abs() < 2e-9
                && (result.internal_energy_j - energy).abs()
                    <= 2e-9 * energy.abs().max(total * 1000.0)
            {
                return Ok(result);
            }
            return Err("homogeneous UV root is not stable equilibrium".into());
        }
        if u < energy {
            lo = t;
        } else {
            hi = t;
        }
    }
    Err("homogeneous UV inversion did not converge".into())
}

fn uv_newton(
    energy: f64,
    volume: f64,
    amounts: [f64; 2],
    total: f64,
) -> Result<ThermoState, String> {
    let scale = total * 10_000.0;
    let residual = |s: &ThermoState| {
        [
            (s.internal_energy_j - energy) / scale,
            (s.volume_m3 / volume).ln(),
        ]
    };
    let norm = |f: [f64; 2]| f[0].abs().max(f[1].abs());
    let mut current = tv(330.0, volume, amounts)?;
    for _ in 0..16 {
        let f = residual(&current);
        if f[0].abs() < 2e-10 && f[1].abs() < 2e-9 {
            return Ok(current);
        }
        let dt = if current.temperature_k + 0.005 <= MAX_TEMPERATURE_K {
            0.005
        } else {
            -0.005
        };
        let dp: f64 = if current.pressure_pa * 1.00005 <= MAX_PRESSURE_PA {
            0.00005
        } else {
            -0.00005
        };
        let ft = residual(&tp(
            current.temperature_k + dt,
            current.pressure_pa,
            amounts,
        )?);
        let fp = residual(&tp(
            current.temperature_k,
            current.pressure_pa * dp.exp(),
            amounts,
        )?);
        let a = (ft[0] - f[0]) / dt;
        let c = (ft[1] - f[1]) / dt;
        let b = (fp[0] - f[0]) / dp;
        let d = (fp[1] - f[1]) / dp;
        let determinant = a * d - b * c;
        if !determinant.is_finite() || determinant.abs() < 1e-14 {
            return Err("UV Newton singular derivative".into());
        }
        let step_t = ((-f[0] * d + b * f[1]) / determinant).clamp(-30.0, 30.0);
        let step_p = ((c * f[0] - a * f[1]) / determinant).clamp(-2.0, 2.0);
        let mut accepted = None;
        for line in 0..10 {
            let fraction = 0.5_f64.powi(line);
            let t = current.temperature_k + fraction * step_t;
            let p = current.pressure_pa * (fraction * step_p).exp();
            if let Ok(candidate) = tp(t, p, amounts) {
                if norm(residual(&candidate)) < norm(f) {
                    accepted = Some(candidate);
                    break;
                }
            }
        }
        current = accepted.ok_or("UV Newton line search did not decrease residual")?;
    }
    Err("UV Newton iteration budget exhausted".into())
}

pub fn ph(p: f64, enthalpy: f64, amounts: [f64; 2]) -> Result<ThermoState, String> {
    check_pressure(p)?;
    let (z, total) = normalize(amounts)?;
    if !enthalpy.is_finite() {
        return Err("finite target enthalpy required".into());
    }
    if z.contains(&0.0) {
        let i = usize::from(z[0] == 0.0);
        if saturation_sign(MIN_TEMPERATURE_K, p, i)? <= 0.0
            && saturation_sign(MAX_TEMPERATURE_K, p, i)? >= 0.0
        {
            let (mut lo, mut hi) = (MIN_TEMPERATURE_K, MAX_TEMPERATURE_K);
            for _ in 0..44 {
                let mid = (lo + hi) * 0.5;
                if saturation_sign(mid, p, i)? > 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let t = (lo + hi) * 0.5;
            let roots = evaluations(t, p, z)?;
            if roots.len() == 2 {
                let beta = (enthalpy / total - roots[0].enthalpy)
                    / (roots[1].enthalpy - roots[0].enthalpy);
                if (0.0..=1.0).contains(&beta) {
                    return pure_coexistence(t, p, amounts, beta);
                }
            }
        }
    }
    inverse_temperature(enthalpy, total, |t| tp(t, p, amounts), |s| s.enthalpy_j)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64, relative: f64) {
        assert!(
            (actual - expected).abs() <= relative * expected.abs().max(1.0),
            "{actual} != {expected}"
        );
    }

    #[test]
    fn matches_unchanged_java_pr78_kernel_fixture() {
        for row in include_str!("../assets/thermo/pr78-java-oracle.csv")
            .lines()
            .skip(1)
        {
            let columns: Vec<&str> = row.split(',').collect();
            let number = |i: usize| columns[i].parse::<f64>().unwrap();
            let (t, p, x0) = (number(0), number(1), number(2));
            let x = [x0, 1.0 - x0];
            let roots = evaluations(t, p, x).unwrap();
            let e = if columns[3] == "LIQUID" {
                roots[0]
            } else {
                *roots.last().unwrap()
            };
            close(e.volume * p / (R * t), number(4), 3e-11);
            close(e.ln_phi[0], number(5), 3e-10);
            close(e.ln_phi[1], number(6), 3e-10);
            let ideal_h: f64 = properties()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, property)| {
                    let dt = t - property.ideal_gas_cp.reference_kelvin;
                    x[i] * property
                        .ideal_gas_cp
                        .coefficients
                        .iter()
                        .enumerate()
                        .map(|(j, c)| c * dt.powi(j as i32 + 1) / (j + 1) as f64)
                        .sum::<f64>()
                })
                .sum();
            close(e.enthalpy - ideal_h, number(7), 3e-10);
        }
    }

    #[test]
    fn phase_balance_fugacity_and_inverse_solutions() {
        let n = [500.0, 500.0];
        let temperature = 320.0;
        let pressure = 277_509.9900520067;
        let two_phase = tp(temperature, pressure, n).unwrap();
        let liquid = two_phase.liquid.as_ref().expect("binary liquid");
        let vapor = two_phase.vapor.as_ref().expect("binary vapor");
        let x = normalize(liquid.amounts).unwrap().0;
        let y = normalize(vapor.amounts).unwrap().0;
        let l = evaluations(temperature, pressure, x).unwrap()[0];
        let v = *evaluations(temperature, pressure, y)
            .unwrap()
            .last()
            .unwrap();
        for i in 0..2 {
            close(liquid.amounts[i] + vapor.amounts[i], n[i], 1e-12);
            close(x[i].ln() + l.ln_phi[i], y[i].ln() + v.ln_phi[i], 2e-9);
        }
        let h = ph(pressure, two_phase.enthalpy_j, n).unwrap();
        close(h.temperature_k, temperature, 2e-6);
        let u = uv(two_phase.internal_energy_j, two_phase.volume_m3, n).unwrap();
        close(u.temperature_k, temperature, 2e-6);
        close(u.pressure_pa, pressure, 3e-6);
        close(u.volume_m3, two_phase.volume_m3, 1e-8);
    }

    #[test]
    fn pure_latent_heat_and_rigid_vessel_coexistence() {
        for component in 0..2 {
            let mut n = [0.0; 2];
            n[component] = 1000.0;
            let p = saturation_pressure(330.0, component).unwrap().unwrap();
            let low = pure_coexistence(330.0, p, n, 0.2).unwrap();
            let high = pure_coexistence(330.0, p, n, 0.8).unwrap();
            assert!(high.enthalpy_j - low.enthalpy_j > 1e7);
            for initial in [low, high] {
                let by_ph = ph(p, initial.enthalpy_j, n).unwrap();
                close(by_ph.temperature_k, 330.0, 1e-9);
                close(by_ph.vapor_fraction(), initial.vapor_fraction(), 1e-8);
                let by_uv = uv(initial.internal_energy_j, initial.volume_m3, n).unwrap();
                close(by_uv.temperature_k, 330.0, 2e-6);
                close(by_uv.vapor_fraction(), initial.vapor_fraction(), 2e-6);
            }
        }
    }

    #[test]
    fn dense_liquid_and_dilute_gas_uv_are_not_lost_between_temperature_grid_points() {
        for amounts in [[1000.0, 0.0], [0.0, 1000.0], [500.0, 500.0]] {
            for (t, p) in [(301.234, 1_234_567.0), (350.125, 1.0), (399.0, 50_000.0)] {
                let initial = tp(t, p, amounts).unwrap();
                let recovered = uv(initial.internal_energy_j, initial.volume_m3, amounts).unwrap();
                close(recovered.temperature_k, t, 1e-7);
                close(recovered.pressure_pa, p, 2e-6);
                close(recovered.volume_m3, initial.volume_m3, 1e-8);
                close(recovered.internal_energy_j, initial.internal_energy_j, 3e-9);
            }
        }
    }

    #[test]
    fn rigid_vessel_fixture_and_repeated_heat_input() {
        let volume = std::f64::consts::PI * 0.9 * 0.9 * 2.8;
        let amounts = [1000.0, 1000.0];
        let initial = tv(320.0, volume, amounts).unwrap();
        assert!(initial.liquid.is_some() && initial.vapor.is_some());
        eprintln!(
            "thermal fixture T={} K P={} Pa U={} J vapor={}",
            initial.temperature_k,
            initial.pressure_pa,
            initial.internal_energy_j,
            initial.vapor_fraction()
        );
        let start = std::time::Instant::now();
        let mut final_state = initial.clone();
        for step in 1..=10 {
            let target = initial.internal_energy_j + f64::from(step) * 100_000.0;
            final_state = uv(target, volume, amounts).unwrap();
            close(final_state.internal_energy_j, target, 3e-7);
            close(final_state.volume_m3, volume, 1e-8);
            assert!(final_state.temperature_k > initial.temperature_k);
        }
        eprintln!(
            "10 bounded UV heat steps: {:?}; final T={} K P={} Pa vapor={}",
            start.elapsed(),
            final_state.temperature_k,
            final_state.pressure_pa,
            final_state.vapor_fraction()
        );
    }

    #[test]
    fn thermodynamic_identity_scaling_and_capability_errors() {
        let a = tp(350.0, 100_000.0, [2.0, 3.0]).unwrap();
        let b = tp(350.0, 100_000.0, [20.0, 30.0]).unwrap();
        close(
            a.enthalpy_j,
            a.internal_energy_j + a.pressure_pa * a.volume_m3,
            1e-12,
        );
        close(b.internal_energy_j, 10.0 * a.internal_energy_j, 1e-12);
        close(b.volume_m3, 10.0 * a.volume_m3, 1e-12);
        assert!(tp(280.0, 100_000.0, [1.0, 1.0]).is_err());
        assert!(tp(350.0, 3e6, [1.0, 1.0]).is_err());
        assert!(tp(350.0, 1e5, [0.0, 0.0]).is_err());
        assert!(uv(f64::NAN, 1.0, [1.0, 0.0]).is_err());
        assert!(tv(330.0, 1e-10, [1.0, 1.0]).is_err());
        assert!(amounts_from_materials(&[MaterialAmount {
            material: "icheme:water".into(),
            moles: 1.0
        }])
        .is_err());
    }
}

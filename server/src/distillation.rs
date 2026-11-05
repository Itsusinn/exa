//! Fixed-pressure binary equilibrium-stage design calculator.
//!
//! All tray compositions, flows and enthalpies solve the steady MESH equations.
//! Total flow balances are eliminated algebraically; phi-phi bubble points
//! eliminate temperature and vapour composition. Constant molar overflow is
//! only a seed, never the published model. The column has no world inventory.
use crate::thermo;
use exa_protocol::{DistillationResult, DistillationSpec, DistillationStage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ENERGY_SCALE: f64 = 30_000.0;
const MAX_ITERATIONS: usize = 70;
const TOLERANCE: f64 = 2e-10;
const MIN_X: f64 = 1e-12;
const MAX_SECONDS: u64 = 8;

struct Budget<'a> {
    cancel: &'a AtomicBool,
    started: Instant,
}
impl Budget<'_> {
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if self.started.elapsed() > Duration::from_secs(MAX_SECONDS) {
            return Err("distillation calculation exceeded its bounded time budget".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Bubble {
    t: f64,
    y: f64,
    hl: f64,
    hv: f64,
}
fn logistic(a: f64) -> f64 {
    1.0 / (1.0 + (-a).exp())
}
fn logit(x: f64) -> f64 {
    (x / (1.0 - x)).ln()
}
fn norm(a: &[f64]) -> f64 {
    a.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}
fn phase_residual(t: f64, p: f64, x: f64, y: f64) -> Result<([f64; 2], Bubble), String> {
    let liquid = thermo::design_phase(t, p, x, false)?;
    let vapor = thermo::design_phase(t, p, y, true)?;
    if vapor.volume_m3_mol <= liquid.volume_m3_mol * 1.01 {
        return Err("distinct equilibrium phases are unavailable".into());
    }
    let residual = [
        x.ln() + liquid.ln_phi[0] - y.ln() - vapor.ln_phi[0],
        (1.0 - x).ln() + liquid.ln_phi[1] - (1.0 - y).ln() - vapor.ln_phi[1],
    ];
    Ok((
        residual,
        Bubble {
            t,
            y,
            hl: liquid.enthalpy_j_mol,
            hv: vapor.enthalpy_j_mol,
        },
    ))
}

fn bubble(p: f64, x: f64, budget: &Budget<'_>) -> Result<Bubble, String> {
    budget.check()?;
    if !(MIN_X..=1.0 - MIN_X).contains(&x) {
        return Err("column composition reached the trace-component numerical boundary".into());
    }
    let mut low = thermo::MIN_TEMPERATURE_K;
    let mut high = thermo::MAX_TEMPERATURE_K;
    for _ in 0..32 {
        let t = 0.5 * (low + high);
        let k = thermo::design_wilson_k(t, p)?;
        if x * k[0] + (1.0 - x) * k[1] > 1.0 {
            high = t;
        } else {
            low = t;
        }
    }
    let mut t = 0.5 * (low + high);
    let k = thermo::design_wilson_k(t, p)?;
    let mut ly = logit((x * k[0] / (x * k[0] + (1.0 - x) * k[1])).clamp(MIN_X, 1.0 - MIN_X));
    for _ in 0..40 {
        budget.check()?;
        let (r, state) = phase_residual(t, p, x, logistic(ly))?;
        if norm(&r) < 2e-11 {
            return Ok(state);
        }
        let dt = if t + 1e-3 <= thermo::MAX_TEMPERATURE_K {
            1e-3
        } else {
            -1e-3
        };
        let rt = phase_residual(t + dt, p, x, logistic(ly))?.0;
        let ry = phase_residual(t, p, x, logistic(ly + 1e-4))?.0;
        let a = (rt[0] - r[0]) / dt;
        let b = (ry[0] - r[0]) / 1e-4;
        let c = (rt[1] - r[1]) / dt;
        let d = (ry[1] - r[1]) / 1e-4;
        let determinant = a * d - b * c;
        if !determinant.is_finite() || determinant.abs() < 1e-12 {
            break;
        }
        let step_t = (-r[0] * d + b * r[1]) / determinant;
        let step_y = (-a * r[1] + r[0] * c) / determinant;
        let mut alpha = 1.0f64.min(8.0 / step_t.abs()).min(2.0 / step_y.abs());
        let mut accepted = false;
        for _ in 0..16 {
            let next_t = t + alpha * step_t;
            let next_ly = ly + alpha * step_y;
            if let Ok((rr, _)) = phase_residual(next_t, p, x, logistic(next_ly)) {
                if norm(&rr) < norm(&r) {
                    t = next_t;
                    ly = next_ly;
                    accepted = true;
                    break;
                }
            }
            alpha *= 0.5;
        }
        if !accepted {
            break;
        }
    }
    Err("equilibrium bubble point is outside the supported domain or did not converge".into())
}

struct Column {
    x: Vec<f64>,
    v: Vec<f64>,
    l: Vec<f64>,
    phases: Vec<Bubble>,
    condenser: Bubble,
    residual: Vec<f64>,
}

fn evaluate(
    spec: &DistillationSpec,
    variables: &[f64],
    feed_h: f64,
    budget: &Budget<'_>,
) -> Result<Column, String> {
    let count = usize::from(spec.trays) + 1;
    let d = spec.distillate_fraction;
    let reflux = spec.reflux_ratio * d;
    let x: Vec<_> = variables[..count].iter().map(|u| logistic(*u)).collect();
    let phases = x
        .iter()
        .map(|x| bubble(spec.pressure_pa, *x, budget))
        .collect::<Result<Vec<_>, _>>()?;
    let condenser = bubble(spec.pressure_pa, phases[0].y, budget)?;
    let mut v = vec![(1.0 + spec.reflux_ratio) * d];
    v.extend(variables[count..].iter().map(|u| u.exp()));
    let l: Vec<f64> = (0..count)
        .map(|j| {
            if j + 1 == count {
                1.0 - d
            } else {
                v[j + 1]
                    + if j + 1 >= usize::from(spec.feed_tray) {
                        1.0
                    } else {
                        0.0
                    }
                    - d
            }
        })
        .collect();
    if l.iter()
        .chain(&v)
        .any(|q| !q.is_finite() || *q <= 1e-9 || *q > 100.0)
    {
        return Err("dry tray or internal flow outside bounded design domain".into());
    }
    let mut residual = Vec::with_capacity(2 * count - 1);
    for j in 0..count {
        let (incoming_l, lx, lh) = if j == 0 {
            (reflux, phases[0].y, condenser.hl)
        } else {
            (l[j - 1], x[j - 1], phases[j - 1].hl)
        };
        let (incoming_v, vy, vh) = if j + 1 == count {
            (0.0, 0.0, 0.0)
        } else {
            (v[j + 1], phases[j + 1].y, phases[j + 1].hv)
        };
        let feed = if j + 1 == usize::from(spec.feed_tray) {
            1.0
        } else {
            0.0
        };
        residual.push(
            incoming_l * lx + incoming_v * vy + feed * spec.butane_fraction
                - l[j] * x[j]
                - v[j] * phases[j].y,
        );
        if j + 1 < count {
            residual.push(
                (incoming_l * lh + incoming_v * vh + feed * feed_h
                    - l[j] * phases[j].hl
                    - v[j] * phases[j].hv)
                    / ENERGY_SCALE,
            );
        }
    }
    Ok(Column {
        x,
        v,
        l,
        phases,
        condenser,
        residual,
    })
}

fn linear_solve(mut matrix: Vec<Vec<f64>>, mut rhs: Vec<f64>) -> Option<Vec<f64>> {
    let n = rhs.len();
    for col in 0..n {
        let row =
            (col..n).max_by(|a, b| matrix[*a][col].abs().total_cmp(&matrix[*b][col].abs()))?;
        if matrix[row][col].abs() < 1e-12 {
            return None;
        }
        matrix.swap(col, row);
        rhs.swap(col, row);
        let pivot = matrix[col][col];
        for value in &mut matrix[col][col..] {
            *value /= pivot;
        }
        rhs[col] /= pivot;
        let pivot_rhs = rhs[col];
        let (completed, remaining) = matrix.split_at_mut(col + 1);
        let pivot_row = &completed[col];
        for (row, row_rhs) in remaining.iter_mut().zip(&mut rhs[col + 1..]) {
            let factor = row[col];
            for (value, pivot_value) in row[col + 1..].iter_mut().zip(&pivot_row[col + 1..]) {
                *value -= factor * pivot_value;
            }
            *row_rhs -= factor * pivot_rhs;
        }
    }
    for col in (0..n).rev() {
        for row in 0..col {
            rhs[row] -= matrix[row][col] * rhs[col];
        }
    }
    rhs.iter().all(|v| v.is_finite()).then_some(rhs)
}

fn seed(
    spec: &DistillationSpec,
    feed_h: f64,
    spread: f64,
    budget: &Budget<'_>,
) -> Result<Vec<f64>, String> {
    let count = usize::from(spec.trays) + 1;
    let z = spec.butane_fraction;
    let d = spec.distillate_fraction;
    let f = bubble(spec.pressure_pa, z, budget)?;
    let alpha = (f.y / (1.0 - f.y)) / (z / (1.0 - z));
    let xd = z + spread * (1.0 - z).min(z * (1.0 - d) / d);
    let xb = ((z - d * xd) / (1.0 - d)).clamp(1e-7, 1.0 - 1e-7);
    let top = xd / (alpha - (alpha - 1.0) * xd);
    let mut variables: Vec<_> = (0..count)
        .map(|j| logit(top) + j as f64 / (count - 1) as f64 * (logit(xb) - logit(top)))
        .collect();
    let q = ((f.hv - feed_h) / (f.hv - f.hl)).clamp(-2.0, 3.0);
    let upper_v = (1.0 + spec.reflux_ratio) * d;
    let lower_v = (upper_v + q - 1.0).max(0.02);
    for j in 1..count {
        variables.push(if j + 1 > usize::from(spec.feed_tray) {
            lower_v.ln()
        } else {
            upper_v.ln()
        });
    }
    Ok(variables)
}

/// Returns only a rigorously audited solution. Cancellation is cooperative and
/// checked in every bubble-point and Newton iteration; no global state is kept.
pub fn solve(spec: &DistillationSpec, cancel: &AtomicBool) -> Result<DistillationResult, String> {
    let budget = Budget {
        cancel,
        started: Instant::now(),
    };
    budget.check()?;
    if !spec.valid() {
        return Err("invalid distillation input".into());
    }
    let feed_h = thermo::tp(
        spec.feed_temperature_k,
        spec.pressure_pa,
        [spec.butane_fraction, 1.0 - spec.butane_fraction],
    )?
    .enthalpy_j;
    let mut iterations = 0;
    let mut last_error = String::from("MESH equations did not converge");
    for spread in [0.65, 0.35, 0.9, 0.1] {
        budget.check()?;
        let mut variables = seed(spec, feed_h, spread, &budget)?;
        for _ in 0..MAX_ITERATIONS {
            budget.check()?;
            iterations += 1;
            let state = match evaluate(spec, &variables, feed_h, &budget) {
                Ok(s) => s,
                Err(e) => {
                    last_error = e;
                    break;
                }
            };
            if norm(&state.residual) < TOLERANCE {
                let mut result = result(spec, &state, iterations);
                audit_with_budget(&mut result, &budget)?;
                return Ok(result);
            }
            let n = variables.len();
            let mut matrix = vec![vec![0.0; n]; n];
            let mut failed = false;
            for col in 0..n {
                let mut trial = variables.clone();
                let mut delta = 2e-5;
                trial[col] += delta;
                let shifted = match evaluate(spec, &trial, feed_h, &budget) {
                    Ok(s) => s,
                    Err(_) => {
                        delta = -delta;
                        trial[col] = variables[col] + delta;
                        match evaluate(spec, &trial, feed_h, &budget) {
                            Ok(s) => s,
                            Err(e) => {
                                last_error = e;
                                failed = true;
                                break;
                            }
                        }
                    }
                };
                for (row, values) in matrix.iter_mut().enumerate() {
                    values[col] = (shifted.residual[row] - state.residual[row]) / delta;
                }
            }
            if failed {
                break;
            }
            let Some(step) = linear_solve(matrix, state.residual.iter().map(|r| -r).collect())
            else {
                last_error = "singular MESH Jacobian near a dry tray or pinch".into();
                break;
            };
            let mut alpha = 1.0f64.min(3.0 / norm(&step));
            let mut accepted = false;
            for _ in 0..18 {
                budget.check()?;
                let trial: Vec<_> = variables
                    .iter()
                    .zip(&step)
                    .map(|(x, dx)| x + alpha * dx)
                    .collect();
                if let Ok(next) = evaluate(spec, &trial, feed_h, &budget) {
                    if norm(&next.residual) < norm(&state.residual) * (1.0 - 1e-4 * alpha) {
                        variables = trial;
                        accepted = true;
                        break;
                    }
                }
                alpha *= 0.5;
            }
            if !accepted {
                last_error = "MESH line search stalled; specifications may lie outside the supported operating domain".into();
                break;
            }
        }
    }
    budget.check()?;
    Err(last_error)
}

fn result(spec: &DistillationSpec, state: &Column, iterations: u32) -> DistillationResult {
    let last = state.x.len() - 1;
    let f = spec.feed_mol_s;
    let d = spec.distillate_fraction;
    DistillationResult {
        spec: spec.clone(),
        stages: state
            .phases
            .iter()
            .enumerate()
            .map(|(j, phase)| DistillationStage {
                number: j as u8 + 1,
                temperature_k: phase.t,
                liquid_butane_fraction: state.x[j],
                vapor_butane_fraction: phase.y,
                liquid_mol_s: f * state.l[j],
                vapor_mol_s: f * state.v[j],
            })
            .collect(),
        distillate_mol_s: f * d,
        bottoms_mol_s: f * (1.0 - d),
        distillate_butane_fraction: state.phases[0].y,
        bottoms_butane_fraction: state.x[last],
        condenser_temperature_k: state.condenser.t,
        condenser_duty_w: f * state.v[0] * (state.phases[0].hv - state.condenser.hl),
        reboiler_duty_w: f
            * (state.l[last] * state.phases[last].hl + state.v[last] * state.phases[last].hv
                - state.l[last - 1] * state.phases[last - 1].hl),
        max_component_residual_mol_s: 0.0,
        max_energy_residual_w: 0.0,
        max_equilibrium_residual: 0.0,
        iterations,
    }
}

/// Independent audit of reported physical streams, not the solver's reduced
/// residual vector. Both components and every stage plus the column boundary
/// are checked again using fresh EOS phase evaluations and a fresh feed flash.
pub fn audit(result: &mut DistillationResult, cancel: &AtomicBool) -> Result<(), String> {
    audit_with_budget(
        result,
        &Budget {
            cancel,
            started: Instant::now(),
        },
    )
}
fn audit_with_budget(result: &mut DistillationResult, budget: &Budget<'_>) -> Result<(), String> {
    budget.check()?;
    let spec = &result.spec;
    if !result.valid() {
        return Err("invalid column audit input".into());
    }
    let f = spec.feed_mol_s;
    let d = result.distillate_mol_s;
    let b = result.bottoms_mol_s;
    let reflux = spec.reflux_ratio * d;
    let stages = &result.stages;
    let last = stages.len() - 1;
    let condenser = thermo::design_phase(
        result.condenser_temperature_k,
        spec.pressure_pa,
        result.distillate_butane_fraction,
        false,
    )?;
    let mut liquid = vec![];
    let mut vapor = vec![];
    let mut component = 0.0f64;
    let mut energy = 0.0f64;
    let mut equilibrium = 0.0f64;
    for (j, stage) in stages.iter().enumerate() {
        budget.check()?;
        if stage.number != j as u8 + 1
            || !stage.liquid_mol_s.is_finite()
            || !stage.vapor_mol_s.is_finite()
            || stage.liquid_mol_s <= 0.0
            || stage.vapor_mol_s <= 0.0
        {
            return Err("invalid reported stage flows".into());
        }
        let l = thermo::design_phase(
            stage.temperature_k,
            spec.pressure_pa,
            stage.liquid_butane_fraction,
            false,
        )?;
        let v = thermo::design_phase(
            stage.temperature_k,
            spec.pressure_pa,
            stage.vapor_butane_fraction,
            true,
        )?;
        if v.volume_m3_mol <= l.volume_m3_mol * 1.01 {
            return Err("degenerate reported equilibrium phases".into());
        }
        for k in 0..2 {
            let x = if k == 0 {
                stage.liquid_butane_fraction
            } else {
                1.0 - stage.liquid_butane_fraction
            };
            let y = if k == 0 {
                stage.vapor_butane_fraction
            } else {
                1.0 - stage.vapor_butane_fraction
            };
            equilibrium = equilibrium.max((x.ln() + l.ln_phi[k] - y.ln() - v.ln_phi[k]).abs());
        }
        if !thermo::design_phase_stable(
            stage.temperature_k,
            spec.pressure_pa,
            stage.liquid_butane_fraction,
            false,
        )? || !thermo::design_phase_stable(
            stage.temperature_k,
            spec.pressure_pa,
            stage.vapor_butane_fraction,
            true,
        )? {
            return Err("unstable reported tray phase".into());
        }
        liquid.push(l.enthalpy_j_mol);
        vapor.push(v.enthalpy_j_mol);
    }
    // A total condenser delivers saturated liquid, not the top tray liquid.
    let saturation = bubble(spec.pressure_pa, result.distillate_butane_fraction, budget)?;
    if (saturation.t - result.condenser_temperature_k).abs() > 1e-6 {
        return Err("condenser saturation audit failed".into());
    }
    if !thermo::design_phase_stable(
        result.condenser_temperature_k,
        spec.pressure_pa,
        result.distillate_butane_fraction,
        false,
    )? {
        return Err("unstable condenser liquid".into());
    }
    let feed = thermo::tp(
        spec.feed_temperature_k,
        spec.pressure_pa,
        [spec.butane_fraction, 1.0 - spec.butane_fraction],
    )?;
    for (j, stage) in stages.iter().enumerate() {
        let (il, ix, ih) = if j == 0 {
            (
                reflux,
                result.distillate_butane_fraction,
                condenser.enthalpy_j_mol,
            )
        } else {
            (
                stages[j - 1].liquid_mol_s,
                stages[j - 1].liquid_butane_fraction,
                liquid[j - 1],
            )
        };
        let (iv, iy, vh) = if j == last {
            (0.0, 0.0, 0.0)
        } else {
            (
                stages[j + 1].vapor_mol_s,
                stages[j + 1].vapor_butane_fraction,
                vapor[j + 1],
            )
        };
        let input = if j + 1 == usize::from(spec.feed_tray) {
            f
        } else {
            0.0
        };
        for k in 0..2 {
            let part = |x: f64| if k == 0 { x } else { 1.0 - x };
            component = component.max(
                (il * part(ix) + iv * part(iy) + input * part(spec.butane_fraction)
                    - stage.liquid_mol_s * part(stage.liquid_butane_fraction)
                    - stage.vapor_mol_s * part(stage.vapor_butane_fraction))
                .abs(),
            );
        }
        energy = energy.max(
            (il * ih
                + iv * vh
                + input * feed.enthalpy_j
                + if j == last {
                    result.reboiler_duty_w
                } else {
                    0.0
                }
                - stage.liquid_mol_s * liquid[j]
                - stage.vapor_mol_s * vapor[j])
                .abs(),
        );
    }
    for k in 0..2 {
        let part = |x: f64| if k == 0 { x } else { 1.0 - x };
        component = component.max(
            (f * part(spec.butane_fraction)
                - d * part(result.distillate_butane_fraction)
                - b * part(result.bottoms_butane_fraction))
            .abs(),
        );
        component = component.max(
            (stages[0].vapor_mol_s * part(stages[0].vapor_butane_fraction)
                - (d + reflux) * part(result.distillate_butane_fraction))
            .abs(),
        );
        component = component.max(
            (stages[last].liquid_mol_s * part(stages[last].liquid_butane_fraction)
                - b * part(result.bottoms_butane_fraction))
            .abs(),
        );
    }
    component = component
        .max((d - f * spec.distillate_fraction).abs())
        .max((d + b - f).abs());
    energy = energy.max(
        (stages[0].vapor_mol_s * vapor[0]
            - (d + reflux) * condenser.enthalpy_j_mol
            - result.condenser_duty_w)
            .abs(),
    );
    energy = energy.max(
        (f * feed.enthalpy_j + result.reboiler_duty_w
            - result.condenser_duty_w
            - d * condenser.enthalpy_j_mol
            - b * liquid[last])
            .abs(),
    );
    if !component.is_finite()
        || !energy.is_finite()
        || !equilibrium.is_finite()
        || component > f * 2e-8
        || energy > f * 2e-4
        || equilibrium > 2e-8
        || result.condenser_duty_w < 0.0
        || result.reboiler_duty_w < 0.0
        || !result.condenser_duty_w.is_finite()
        || !result.reboiler_duty_w.is_finite()
    {
        return Err(format!("independent MESH audit failed: component={component:e}, energy={energy:e}, equilibrium={equilibrium:e}"));
    }
    result.max_component_residual_mol_s = component;
    result.max_energy_residual_w = energy;
    result.max_equilibrium_residual = equilibrium;
    budget.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rigorous_binary_column_separates_and_independently_closes_mesh() {
        let started = Instant::now();
        let mut result = solve(&DistillationSpec::default(), &AtomicBool::new(false)).unwrap();
        eprintln!("default binary column: xD={:.10}, xB={:.10}, Qc={:.6} W, Qr={:.6} W, iterations={}, elapsed={:?}", result.distillate_butane_fraction, result.bottoms_butane_fraction, result.condenser_duty_w, result.reboiler_duty_w, result.iterations, started.elapsed());
        assert!(result.distillate_butane_fraction > 0.75, "{result:?}");
        assert!(result.bottoms_butane_fraction < 0.25);
        assert!(result.condenser_duty_w > 0.0 && result.reboiler_duty_w > 0.0);
        assert!(
            result.stages.first().unwrap().temperature_k
                < result.stages.last().unwrap().temperature_k
        );
        audit(&mut result, &AtomicBool::new(false)).unwrap();
        result.stages[2].liquid_mol_s *= 1.01;
        assert!(audit(&mut result, &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn independent_audit_refuses_nonfinite_streams_and_tampered_physical_outputs() {
        let cancel = AtomicBool::new(false);
        let baseline = solve(&DistillationSpec::default(), &cancel).unwrap();
        for change in [
            |r: &mut DistillationResult| r.bottoms_mol_s = f64::NAN,
            |r: &mut DistillationResult| r.reboiler_duty_w += 10.0,
            |r: &mut DistillationResult| r.stages[1].vapor_butane_fraction += 0.01,
            |r: &mut DistillationResult| r.condenser_temperature_k += 1.0,
        ] {
            let mut changed = baseline.clone();
            change(&mut changed);
            assert!(audit(&mut changed, &cancel).is_err());
        }
    }
    #[test]
    fn feed_scale_changes_flows_and_duties_not_equilibrium() {
        let cancel = AtomicBool::new(false);
        let base = solve(&DistillationSpec::default(), &cancel).unwrap();
        let scaled = solve(
            &DistillationSpec {
                feed_mol_s: 7.0,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
        assert!(
            (base.distillate_butane_fraction - scaled.distillate_butane_fraction).abs() < 1e-10
        );
        assert!((scaled.reboiler_duty_w / base.reboiler_duty_w - 7.0).abs() < 1e-8);
    }
    #[test]
    fn distinct_feed_and_tray_designs_close_and_cancellation_never_returns_a_result() {
        let cancel = AtomicBool::new(false);
        for (trays, feed_tray, reflux_ratio, temperature) in [
            (2, 1, 2.0, 320.0),
            (3, 3, 2.0, 320.0),
            (8, 4, 3.0, 340.0),
            (12, 6, 4.0, 330.0),
        ] {
            let spec = DistillationSpec {
                trays,
                feed_tray,
                reflux_ratio,
                feed_temperature_k: temperature,
                ..Default::default()
            };
            let result = solve(&spec, &cancel).unwrap();
            assert_eq!(result.stages.len(), usize::from(trays) + 1);
            assert!(result.max_component_residual_mol_s < 2e-8);
        }
        assert_eq!(
            solve(&DistillationSpec::default(), &AtomicBool::new(true)).unwrap_err(),
            "cancelled"
        );
        assert!(solve(
            &DistillationSpec {
                feed_mol_s: f64::NAN,
                ..Default::default()
            },
            &cancel
        )
        .is_err());
        assert!(solve(
            &DistillationSpec {
                pressure_pa: 100_000.0,
                ..Default::default()
            },
            &cancel
        )
        .is_err());
        assert!(solve(
            &DistillationSpec {
                pressure_pa: 2_000_000.0,
                ..Default::default()
            },
            &cancel
        )
        .is_err());
    }
    #[test]
    fn higher_reflux_improves_separation_and_cancellation_is_checked_inside_properties() {
        let cancel = AtomicBool::new(false);
        let low = solve(&DistillationSpec::default(), &cancel).unwrap();
        let high = solve(
            &DistillationSpec {
                reflux_ratio: 4.0,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
        assert!(high.distillate_butane_fraction > low.distillate_butane_fraction);
        assert!(high.reboiler_duty_w > low.reboiler_duty_w);
        let budget = Budget {
            cancel: &cancel,
            started: Instant::now(),
        };
        bubble(400_000.0, 0.5, &budget).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(bubble(400_000.0, 0.5, &budget).unwrap_err(), "cancelled");
    }
}

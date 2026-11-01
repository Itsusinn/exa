#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"
---
// Generate an independent PR fixture by compiling the unchanged reference Java
// kernel, never the Rust implementation under test. Requires JDK 21+ and refs/.
use std::{error::Error, fs, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::current_dir()?;
    let source =
        root.join("refs/Industrial-Chemical-Engineering/src/main/java/com/wormzjl/icheme/science");
    let output = root.join("target/thermo-oracle");
    fs::create_dir_all(&output)?;
    let harness = output.join("ThermoOracle.java");
    fs::write(
        &harness,
        r#"
import java.util.Locale;
import com.wormzjl.icheme.science.thermo.PengRobinsonKernel;
public class ThermoOracle {
  public static void main(String[] args) {
    Locale.setDefault(Locale.ROOT);
    var kernel = new PengRobinsonKernel(new double[]{425.12,469.7},new double[]{3796000,3370000},
      new double[]{.199,.251},new double[][]{{0,.0174},{.0174,0}},293.15,400,1,2000000);
    System.out.println("t_k,p_pa,x_butane,branch,z,ln_phi_butane,ln_phi_pentane,residual_h_j_mol");
    for(double t: new double[]{293.15,320,350,390,400})
      for(double p: new double[]{10000,100000,200000,1000000,2000000})
        for(double x: new double[]{0,.2,.5,.8,1})
          for(var branch: PengRobinsonKernel.Root.values()) {
            var e = kernel.newEvaluation();
            kernel.evaluate(t,p,new double[]{x,1-x},branch,kernel.newWorkspace(),e);
            System.out.printf("%.17g,%.17g,%.17g,%s,%.17g,%.17g,%.17g,%.17g%n",
              t,p,x,branch,e.compressibility(),e.logFugacityCoefficient(0),e.logFugacityCoefficient(1),e.residualEnthalpyJoulesPerMol());
          }
  }
}
"#,
    )?;
    let classes = [
        "thermo/PengRobinsonKernel.java",
        "thermo/PairInteractions.java",
        "math/DeterministicMath.java",
        "math/DeterministicPow.java",
        "math/FdlibmPow.java",
    ];
    let compile = Command::new("javac")
        .arg("-d")
        .arg(&output)
        .args(classes.map(|f| source.join(f)))
        .arg(&harness)
        .status()?;
    if !compile.success() {
        return Err("reference Java compilation failed".into());
    }
    let run = Command::new("java")
        .arg("-cp")
        .arg(&output)
        .arg("ThermoOracle")
        .output()?;
    if !run.status.success() {
        return Err(String::from_utf8_lossy(&run.stderr).to_string().into());
    }
    let fixture = root.join("server/assets/thermo/pr78-java-oracle.csv");
    fs::write(&fixture, run.stdout)?;
    println!("Generated {}", fixture.display());
    Ok(())
}

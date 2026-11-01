# Limited hydrocarbon thermodynamics

Model revision: `butane-pentane-pr78-cp298-r1`.
Reference snapshot: `79b853e0d244cd96c3375f73300a97787d4792ff`.

`n_butane.json` and `n_pentane.json` are unmodified property records from
`refs/Industrial-Chemical-Engineering/src/main/resources/data/icheme/materials/properties/tjl19_n_{butane,pentane}.json`.
The source repository's MIT notice is retained in `LICENSE-IChemE` (copyright 2026 wormzjl).
The source records describe their DWSIM/ChemSep reconstruction and original data provenance.
Their PR78 critical constants and shifted ideal-gas heat-capacity coefficients are used;
the viscosity records are retained for provenance but are **not** used by this implementation.

The constant binary interaction `kij = 0.0174` is from
`materials/interactions/tjl19.json`, revision `tjl19-frozen-kij-r1`.
PR78 uses classical quadratic mixing, analytic cubic roots, fugacity coefficients,
and residual enthalpy, following the equations implemented by the reference
`science/thermo/PengRobinsonKernel.java`. The numerical solvers here are separate Rust
implementations, not ports of the full reference equilibrium engine.

The supported domain is deliberately limited to 293.15–400 K and 1 Pa–2 MPa.
This is an implementation acceptance envelope, not a new experimental qualification
of the source material data. The lower 5 K of the ideal-gas fit uses the reference
record's declared ambient continuation. Below the source fluid data's 100 Pa floor,
PR78 is continued toward the dilute-gas limit so an empty vessel can receive a small
amount; these states are not independently qualified. No continuation outside this envelope is
accepted. Near-critical, liquid-liquid, solid, aqueous and reacting systems are not
supported. Water remains entirely on the pre-existing legacy water model and must
never be silently interpreted on this energy datum.

Ideal-gas enthalpy is zero at 298.15 K separately for each component. Phase enthalpy
adds the PR residual; phase internal energy is H − PV. Energies can therefore be
negative. No formation enthalpies or reaction energetics are provided. PR molar
volumes are untranslated: this limited package does not claim calibrated liquid
density or complete reference-package parity.

TP uses binary tangent-plane searches over both physical roots, then a bounded
fugacity/Rachford–Rice iteration and a product-stability check. TV inverts pressure;
UV first tests the homogeneous fixed-volume caloric root against a stable TP
flash, then attempts a bounded coupled Newton solve, with nested TV temperature
inversion as fallback; PH inverts temperature. The homogeneous fast path avoids
losing dense liquid states whose pressure-admissible temperature interval is
narrower than the fallback search grid. Pure-component coexistence uses equality of phase
fugacities and energy/volume lever rules; TP alone cannot specify the phase fraction
at exact saturation. All iterations are bounded, and unresolved or unsupported
states return errors so callers can preserve the last accepted inventory.

Validation covers phase/element balances, equality of fugacities, PH/UV recovery,
pure-fluid latent heat, scale invariance, and capability failures. These are numerical
consistency checks, not independent experimental validation. The separate Java-kernel
oracle fixture checks PR volume, residual enthalpy and fugacity against the unchanged
reference kernel; it does not prove the entire flash solver correct.

Regenerate the 250-state oracle fixture from the repository root with
`cargo +nightly -Zscript scripts/thermo_oracle.rs` (JDK 21+ and the reference checkout
required). It compiles the original Java kernel and deterministic math classes in
`target/thermo-oracle/`; it does not call or copy results from the Rust solver.
The fixture and data are tracked; compiled Java classes are build outputs.

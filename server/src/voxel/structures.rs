//! Embedded, validated structure definitions and reusable cuboid templates.
//! Plans depend only on seed/coordinates. Writes are clipped to the new column.
use super::ecology::{hash, surface, Biome};
use anyhow::{ensure, Context, Result};
use exa_protocol::*;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

const DEFINITIONS: &str = include_str!("../../assets/structures/definitions.json");
const TEMPLATES: &str = include_str!("../../assets/structures/templates.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Catalog {
    version: u32,
    placement: Placement,
    structures: Vec<Definition>,
    #[serde(skip)]
    templates: BTreeMap<String, Template>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    spacing: i32,
    separation: i32,
    salt: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    id: String,
    weight: u32,
    biomes: Vec<Biome>,
    terrain_adaptation: Adaptation,
    max_slope: i32,
    max_distance_from_center: i32,
    pieces: Vec<Piece>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Adaptation {
    Terrace,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Piece {
    offset: [i32; 2],
    rotation: u8,
    pool: Vec<Choice>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    template: String,
    weight: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Template {
    fills: Vec<Fill>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fill {
    from: [i32; 3],
    to: [i32; 3],
    block: String,
    #[serde(skip)]
    block_id: u8,
}

impl Catalog {
    fn parse(definitions: &str, templates: &str) -> Result<Self> {
        let mut c: Self =
            serde_json::from_str(definitions).context("structure definitions JSON")?;
        c.templates = serde_json::from_str(templates).context("structure templates JSON")?;
        ensure!(c.version == 1, "unsupported structure catalog version");
        let p = &c.placement;
        ensure!(
            (8..=128).contains(&p.spacing) && (4..p.spacing).contains(&p.separation),
            "invalid structure spacing/separation"
        );
        ensure!(
            !c.templates.is_empty() && c.templates.len() <= 128,
            "invalid template count"
        );
        for (name, t) in &mut c.templates {
            ensure!(
                !name.is_empty() && !t.fills.is_empty() && t.fills.len() <= 256,
                "invalid template {name}"
            );
            for f in &mut t.fills {
                ensure!(
                    (0..3).all(|i| f.from[i] <= f.to[i]),
                    "reversed fill in {name}"
                );
                ensure!(
                    [0, 2]
                        .into_iter()
                        .all(|i| f.from[i] >= -32 && f.to[i] <= 32)
                        && f.from[1] >= 0
                        && f.to[1] <= 24,
                    "template bounds exceeded: {name}"
                );
                f.block_id = BLOCK_IDS
                    .iter()
                    .position(|id| *id == f.block)
                    .with_context(|| format!("unknown block {} in {name}", f.block))?
                    as u8;
                // Stateful blocks need a block-entity generation pass before enabling them.
                ensure!(
                    f.block_id != CHEST,
                    "chest templates require container generation"
                );
            }
        }
        ensure!(
            !c.structures.is_empty() && c.structures.len() <= 32,
            "invalid structure count"
        );
        let mut ids = std::collections::BTreeSet::new();
        for d in &c.structures {
            ensure!(
                !d.id.is_empty() && ids.insert(&d.id),
                "duplicate/empty structure ID"
            );
            ensure!(
                (1..=100).contains(&d.weight) && !d.biomes.is_empty(),
                "invalid structure weight/biomes"
            );
            ensure!((0..=12).contains(&d.max_slope), "invalid slope");
            let r = d.max_distance_from_center;
            ensure!(
                (8..=32).contains(&r) && r * 2 < (p.separation + 1) * 16,
                "structure footprints can overlap"
            );
            ensure!(
                !d.pieces.is_empty() && d.pieces.len() <= 64,
                "invalid piece count"
            );
            for piece in &d.pieces {
                ensure!(
                    piece.rotation < 4 && piece.offset.iter().all(|v| (-r..=r).contains(v)),
                    "invalid piece transform"
                );
                ensure!(
                    !piece.pool.is_empty() && piece.pool.len() <= 32,
                    "invalid template pool"
                );
                for choice in &piece.pool {
                    ensure!((1..=100).contains(&choice.weight), "invalid pool weight");
                    let t = c
                        .templates
                        .get(&choice.template)
                        .context("unknown template reference")?;
                    for f in &t.fills {
                        for x in [f.from[0], f.to[0]] {
                            for z in [f.from[2], f.to[2]] {
                                let q = rotate([x, z], piece.rotation);
                                ensure!(
                                    (0..2).all(|i| (q[i] + piece.offset[i]).abs() <= r - 4),
                                    "piece outside flat footprint: {}",
                                    d.id
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(c)
    }
}

pub(super) fn catalog() -> Result<&'static Catalog> {
    static CATALOG: OnceLock<std::result::Result<Catalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| Catalog::parse(DEFINITIONS, TEMPLATES).map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow::anyhow!("invalid embedded structures: {e}"))
}

fn rotate([x, z]: [i32; 2], turns: u8) -> [i32; 2] {
    match turns % 4 {
        0 => [x, z],
        1 => [-z, x],
        2 => [-x, -z],
        _ => [z, -x],
    }
}

struct Site<'a> {
    definition: &'a Definition,
    center: [i32; 2],
    ground: i32,
    entropy: u64,
}

impl Catalog {
    fn anchor(&self, seed: u64, region: [i32; 2]) -> ([i32; 2], u64) {
        let p = &self.placement;
        let h = hash(seed ^ p.salt, region[0], 0, region[1]);
        let range = (p.spacing - p.separation) as u64;
        (
            [
                (region[0] * p.spacing + (h % range) as i32) * 16 + 8,
                (region[1] * p.spacing + ((h >> 32) % range) as i32) * 16 + 8,
            ],
            h,
        )
    }

    fn site(&self, seed: u64, region: [i32; 2]) -> Option<Site<'_>> {
        let (center, entropy) = self.anchor(seed, region);
        let s = surface(seed, center[0], center[1]);
        let eligible: Vec<_> = self
            .structures
            .iter()
            .filter(|d| d.biomes.contains(&s.biome))
            .collect();
        let total: u64 = eligible.iter().map(|d| u64::from(d.weight)).sum();
        if total == 0 {
            return None;
        }
        let mut roll = hash(entropy, 0, 1, 0) % total;
        let d = eligible.into_iter().find(|d| {
            if roll < u64::from(d.weight) {
                true
            } else {
                roll -= u64::from(d.weight);
                false
            }
        })?;
        let r = d.max_distance_from_center;
        if center
            .iter()
            .any(|&v| v - r < -WORLD_LIMIT || v + r >= WORLD_LIMIT)
        {
            return None;
        }
        // Sample the whole footprint, not just the anchor. Reject water, cliffs
        // and high sites before any writes. Every column makes the same decision.
        let mut heights = Vec::with_capacity(25);
        for dz in [-r, -r / 2, 0, r / 2, r] {
            for dx in [-r, -r / 2, 0, r / 2, r] {
                let s = surface(seed, center[0] + dx, center[1] + dz);
                if s.water || s.height < 25 || s.height > WORLD_HEIGHT - 28 {
                    return None;
                }
                heights.push(s.height);
            }
        }
        heights.sort_unstable();
        if heights[24] - heights[0] > d.max_slope {
            return None;
        }
        Some(Site {
            definition: d,
            center,
            ground: heights[12],
            entropy,
        })
    }
}

pub(super) fn apply_column(seed: u64, column: [i32; 2], blocks: &mut [u8]) -> Result<()> {
    let catalog = catalog()?;
    let p = &catalog.placement;
    // Radius <= 32 voxels; a one-region halo covers all crossing structures.
    let region = column.map(|v| v.div_euclid(p.spacing));
    let origin = column.map(|v| v * 16);
    for z in region[1] - 1..=region[1] + 1 {
        for x in region[0] - 1..=region[0] + 1 {
            let (a, _) = catalog.anchor(seed, [x, z]);
            if (0..2).any(|i| a[i] + 32 < origin[i] || a[i] - 32 > origin[i] + 15) {
                continue;
            }
            if let Some(site) = catalog.site(seed, [x, z]) {
                paint(catalog, seed, &site, origin, blocks);
            }
        }
    }
    Ok(())
}

fn paint(c: &Catalog, seed: u64, site: &Site<'_>, origin: [i32; 2], blocks: &mut [u8]) {
    let d = site.definition;
    let r = d.max_distance_from_center;
    let [cx, cz] = site.center;
    let index = |x: i32, y: i32, z: i32| {
        (x - origin[0]) as usize + 16 * ((z - origin[1]) as usize + 16 * y as usize)
    };
    match d.terrain_adaptation {
        Adaptation::Terrace => {
            for z in origin[1].max(cz - r)..=(origin[1] + 15).min(cz + r) {
                for x in origin[0].max(cx - r)..=(origin[0] + 15).min(cx + r) {
                    let s = surface(seed, x, z);
                    let edge = (x - cx).abs().max((z - cz).abs());
                    let weight = ((r - edge) as f32 / 4.0).clamp(0.0, 1.0);
                    if weight == 0.0 {
                        continue;
                    }
                    let ground =
                        (s.height as f32 + (site.ground - s.height) as f32 * weight).round() as i32;
                    // Solid foundations seal shallow caves; deep caves are retained.
                    for y in (ground.min(s.height) - 6).max(1)..=ground {
                        blocks[index(x, y, z)] = if y < ground - 2 {
                            STONE
                        } else if y < ground {
                            DIRT
                        } else {
                            match s.biome {
                                Biome::Desert => SAND,
                                Biome::Snow => SNOW,
                                _ => GRASS,
                            }
                        };
                    }
                    for y in ground + 1..WORLD_HEIGHT {
                        blocks[index(x, y, z)] = AIR;
                    }
                }
            }
        }
    }
    let turn = (site.entropy >> 48) as u8 % 4;
    for (n, piece) in d.pieces.iter().enumerate() {
        let total: u64 = piece.pool.iter().map(|p| u64::from(p.weight)).sum();
        let mut roll = hash(site.entropy, n as i32, 2, 0) % total;
        let choice = piece.pool.iter().find(|p| {
            if roll < u64::from(p.weight) {
                true
            } else {
                roll -= u64::from(p.weight);
                false
            }
        });
        let Some(choice) = choice else {
            continue;
        };
        let offset = rotate(piece.offset, turn);
        for f in &c.templates[&choice.template].fills {
            let a = rotate([f.from[0], f.from[2]], piece.rotation + turn);
            let b = rotate([f.to[0], f.to[2]], piece.rotation + turn);
            let lo = [
                cx + offset[0] + a[0].min(b[0]),
                cz + offset[1] + a[1].min(b[1]),
            ];
            let hi = [
                cx + offset[0] + a[0].max(b[0]),
                cz + offset[1] + a[1].max(b[1]),
            ];
            for z in lo[1].max(origin[1])..=hi[1].min(origin[1] + 15) {
                for x in lo[0].max(origin[0])..=hi[0].min(origin[0] + 15) {
                    for y in site.ground + f.from[1]..=site.ground + f.to[1] {
                        blocks[index(x, y, z)] = f.block_id;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::{terrain::ensure_column, VoxelWorld};
    use exa_simulation::grid::Grid;

    fn sites(c: &Catalog) -> Vec<Site<'_>> {
        let mut found = BTreeMap::new();
        for z in -12..=12 {
            for x in -12..=12 {
                if let Some(site) = c.site(12345, [x, z]) {
                    let score = site
                        .center
                        .iter()
                        .map(|v| i64::from(*v).pow(2))
                        .sum::<i64>();
                    let key = (site.definition.id.clone(), site.center[0] < 0);
                    let entry = found.entry(key).or_insert((i64::MAX, None));
                    if score < entry.0 {
                        *entry = (score, Some(site));
                    }
                }
            }
        }
        assert_eq!(
            found.len(),
            4,
            "both structures must occur in positive and negative coordinates"
        );
        found.into_values().map(|(_, s)| s.unwrap()).collect()
    }
    fn columns(site: &Site<'_>) -> Vec<[i32; 2]> {
        let r = site.definition.max_distance_from_center;
        let [x, z] = site.center;
        ((z - r).div_euclid(16)..=(z + r).div_euclid(16))
            .flat_map(|z| ((x - r).div_euclid(16)..=(x + r).div_euclid(16)).map(move |x| [x, z]))
            .collect()
    }
    fn local(site: &Site<'_>, x: i32, y: i32, z: i32) -> [i32; 3] {
        let offset = rotate([x, z], (site.entropy >> 48) as u8 % 4);
        [
            site.center[0] + offset[0],
            site.ground + y,
            site.center[1] + offset[1],
        ]
    }
    #[test]
    fn definitions_reject_unknown_blocks_invalid_bounds_and_references() {
        Catalog::parse(DEFINITIONS, TEMPLATES).unwrap();
        for definitions in [
            DEFINITIONS.replace("\"spacing\": 24", "\"spacing\": 8"),
            DEFINITIONS.replace("\"rotation\": 0", "\"rotation\": 4"),
            DEFINITIONS.replace("\"cottage\"", "\"missing\""),
            DEFINITIONS.replace("\"terrace\"", "\"beard_thin\""),
            DEFINITIONS.replace(
                "\"max_distance_from_center\": 24",
                "\"max_distance_from_center\": 8",
            ),
        ] {
            assert!(Catalog::parse(&definitions, TEMPLATES).is_err());
        }
        for templates in [
            TEMPLATES.replace("game:stone", "game:unknown"),
            TEMPLATES.replace("game:stone", "game:chest"),
            TEMPLATES.replace("[20,0,1]", "[200,0,1]"),
        ] {
            assert!(Catalog::parse(DEFINITIONS, &templates).is_err());
        }
    }
    #[test]
    fn structure_sites_emit_fixture_and_have_accessible_interiors() {
        let c = catalog().unwrap();
        let mut fixtures = Vec::new();
        for site in sites(c) {
            let mut grid = Grid::unknown();
            for col in columns(&site) {
                ensure_column(&mut grid, 12345, col).unwrap();
            }
            let route: Vec<_> = if site.definition.id == "castle" {
                (-5..=20)
                    .map(|z| (0, z))
                    .chain((-15..=-13).map(|z| (-15, z)))
                    .collect()
            } else {
                (-11..=-4)
                    .map(|x| (x, 0))
                    .chain((-11..=0).map(|z| (-11, z)))
                    .collect()
            };
            for (x, z) in route {
                let feet = local(&site, x, 1, z);
                assert!(
                    !exa_simulation::overlaps(feet.map(|v| v as f32 + 0.5), &|p| grid.solid(p)),
                    "blocked path in {} at {feet:?}",
                    site.definition.id
                );
                assert!(grid.solid(local(&site, x, 0, z)), "path lacks foundation");
            }
            let building = if site.definition.id == "castle" {
                (6, 1, -5)
            } else {
                (-7, 1, -7)
            };
            assert!(grid.solid(local(&site, building.0, building.1, building.2)));
            assert!(grid.blocks.values().flatten().all(|&b| valid_block(b)));
            let turn = (site.entropy >> 48) as u8 % 4;
            let camera = rotate([26, 32], turn);
            let (stand, target) = if site.definition.id == "castle" {
                (local(&site, 4, 1, 14), local(&site, 4, 2, 16))
            } else {
                (local(&site, -8, 1, -5), local(&site, -8, 2, -7))
            };
            assert!(grid.solid(target));
            let feet = [
                stand[0] as f32 + 0.5,
                stand[1] as f32,
                stand[2] as f32 + 0.5,
            ];
            let offset = [
                target[0] as f32 + 0.5 - feet[0],
                target[1] as f32 + 0.5 - feet[1] - exa_simulation::EYE_HEIGHT,
                target[2] as f32 + 0.5 - feet[2],
            ];
            let yaw = offset[0].atan2(-offset[2]);
            let pitch = offset[1].atan2(offset[0].hypot(offset[2]));
            assert_eq!(
                exa_simulation::targeting::target_block(&grid, feet, yaw, pitch)
                    .unwrap()
                    .position,
                target
            );
            assert!(!exa_simulation::overlaps(
                stand.map(|v| v as f32 + 0.5),
                &|p| grid.solid(p)
            ));
            fixtures.push(serde_json::json!({"id":site.definition.id,
                "name":format!("{}-{}",site.definition.id,if site.center[0]<0 {"negative"} else {"positive"}),
                "center":site.center,"ground":site.ground,"edit_stand":stand,"edit_target":target,
                "x":site.center[0]+camera[0],"y":site.ground+29,"z":site.center[1]+camera[1],
                "yaw":-0.68+std::f32::consts::FRAC_PI_2*f32::from(turn),"pitch":-0.58}));
        }
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("structure-sites.json"),
            serde_json::to_vec_pretty(&fixtures).unwrap(),
        )
        .unwrap();
        println!(
            "STRUCTURE_SITES={}",
            serde_json::to_string(&fixtures).unwrap()
        );
    }
    #[test]
    fn structures_are_order_independent_and_preserve_saved_edits() {
        for site in sites(catalog().unwrap()) {
            let columns = columns(&site);
            let mut a = VoxelWorld::new(12345).unwrap();
            let mut b = Grid::unknown();
            for &col in &columns {
                ensure_column(&mut a.terrain, 12345, col).unwrap();
            }
            for &col in columns.iter().rev() {
                ensure_column(&mut b, 12345, col).unwrap();
            }
            for &col in &columns {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let o = [col[0] * 16, y, col[1] * 16];
                    assert_eq!(a.terrain.chunk(o), b.chunk(o));
                }
            }
            let edited = local(&site, 0, 0, 0);
            a.terrain.set(edited, BASALT);
            let root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/structure-tests");
            std::fs::create_dir_all(&root).unwrap();
            let dir = tempfile::tempdir_in(root).unwrap();
            let path = dir.path().join("world3d.bin");
            a.save(&path).unwrap();
            let mut loaded = VoxelWorld::load(&path).unwrap();
            for &col in &columns {
                ensure_column(&mut loaded.terrain, 12345, col).unwrap();
            }
            assert_eq!(a.terrain, loaded.terrain);
            assert_eq!(loaded.terrain.get(edited), BASALT);

            // A checkpoint halfway through exploration must not prevent the
            // remaining pieces from matching the original deterministic plan.
            for &col in &columns[columns.len() / 2..] {
                a.terrain.unload_column(col);
            }
            a.save(&path).unwrap();
            let mut partial = VoxelWorld::load(&path).unwrap();
            for &col in &columns {
                ensure_column(&mut partial.terrain, 12345, col).unwrap();
            }
            for &col in &columns[..columns.len() / 2] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let o = [col[0] * 16, y, col[1] * 16];
                    assert_eq!(partial.terrain.chunk(o), a.terrain.chunk(o));
                }
            }
            for &col in &columns[columns.len() / 2..] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let o = [col[0] * 16, y, col[1] * 16];
                    assert_eq!(partial.terrain.chunk(o), b.chunk(o));
                }
            }
        }
    }
    #[test]
    fn generation_rejects_water_cliffs_and_world_edges_and_is_seeded() {
        let c = catalog().unwrap();
        assert_ne!(c.anchor(1, [2, -3]), c.anchor(2, [2, -3]));
        let mut rejected = 0;
        for z in -5..=5 {
            for x in -5..=5 {
                if let Some(s) = c.site(12345, [x, z]) {
                    let p = surface(12345, s.center[0], s.center[1]);
                    assert!(!p.water);
                    assert!(s.definition.biomes.contains(&p.biome));
                } else {
                    rejected += 1;
                }
            }
        }
        assert!(rejected > 0);
        assert!(c.site(12345, [-100, -100]).is_none());
        assert!(c.site(12345, [100, 100]).is_none());
        let mut g = super::super::generate(12345).unwrap();
        let before = g.clone();
        for z in 0..4 {
            for x in 0..4 {
                ensure_column(&mut g, 12345, [x, z]).unwrap();
            }
        }
        assert_eq!(g, before);
    }
}

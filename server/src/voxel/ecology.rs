//! Deterministic, coordinate-based climate, landforms, strata and vegetation.
//! No global RNG, cached world state or generation-order-dependent writes.
use exa_protocol::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Biome {
    Grassland,
    Forest,
    Taiga,
    Desert,
    Wetland,
    Snow,
}

pub(super) fn hash(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9e3779b185ebca87)
        ^ (y as u64).wrapping_mul(0xc2b2ae3d27d4eb4f)
        ^ (z as u64).wrapping_mul(0x165667b19e3779f9);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}
fn value(seed: u64, x: i32, y: i32, z: i32) -> f32 {
    (hash(seed, x, y, z) >> 40) as f32 / 8_388_607.5 - 1.0
}
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn noise(seed: u64, x: f32, z: f32, scale: f32) -> f32 {
    let x = x / scale;
    let z = z / scale;
    let ix = x.floor() as i32;
    let iz = z.floor() as i32;
    let fx = smooth(x - x.floor());
    let fz = smooth(z - z.floor());
    mix(
        mix(value(seed, ix, 0, iz), value(seed, ix + 1, 0, iz), fx),
        mix(
            value(seed, ix, 0, iz + 1),
            value(seed, ix + 1, 0, iz + 1),
            fx,
        ),
        fz,
    )
}
fn fbm(seed: u64, x: f32, z: f32, scale: f32) -> f32 {
    noise(seed, x, z, scale) * 0.62
        + noise(seed ^ 81, x, z, scale * 0.5) * 0.26
        + noise(seed ^ 137, x, z, scale * 0.25) * 0.12
}

#[derive(Clone, Copy)]
pub(super) struct Surface {
    pub height: i32,
    pub water: bool,
    pub biome: Biome,
}
fn carved(s: Surface, y: usize, field: [f32; 2]) -> bool {
    let depth = s.height - y as i32;
    let mouth = !s.water && s.height > 35 && field[1].abs() < 0.12;
    y > 3
        && (depth > 4 || mouth)
        && ((field[0].abs() < 0.16 && field[1].abs() < 0.23) || (field[0] > 0.48 && depth > 8))
}
pub(super) fn surface(seed: u64, x: i32, z: i32) -> Surface {
    let (xf, zf) = (x as f32, z as f32);
    let temp = fbm(seed ^ 0x1234, xf, zf, 176.0);
    let wet = fbm(seed ^ 0x9876, xf, zf, 144.0);
    let ridge = 1.0 - noise(seed ^ 0x5555, xf, zf, 112.0).abs();
    let uplift = smooth(((noise(seed ^ 0x8888, xf, zf, 240.0) + 0.2) * 1.6).clamp(0.0, 1.0));
    let base = 29.0 + fbm(seed ^ 0x3333, xf, zf, 96.0) * 16.0 + ridge.powi(3) * uplift * 53.0;
    // Warped river contours form winding valleys, with wider shallow wetland basins.
    let warp = noise(seed ^ 0x7777, xf, zf, 84.0) * 24.0;
    let river = noise(seed ^ 0x4567, xf + warp, zf - warp, 128.0).abs();
    let valley = smooth((river / 0.13).min(1.0));
    let shaped = mix(21.0, base, valley);
    let height = shaped.round().clamp(2.0, 100.0) as i32;
    let biome = if temp < -0.32 || height > 70 {
        Biome::Snow
    } else if temp < -0.10 {
        Biome::Taiga
    } else if temp > 0.18 && wet < -0.06 {
        Biome::Desert
    } else if wet > 0.27 && height < 34 {
        Biome::Wetland
    } else if wet > -0.05 {
        Biome::Forest
    } else {
        Biome::Grassland
    };
    Surface {
        height,
        biome,
        water: height < 24,
    }
}

// Coarse continuous fields are sampled once per column. Interpolation in the
// voxel loop avoids hashing eight lattice points for every cave voxel.
struct CaveField {
    values: Vec<[f32; 2]>,
}
impl CaveField {
    fn new(seed: u64, c: [i32; 2]) -> Self {
        let mut values = Vec::with_capacity(3 * 17 * 3);
        for y in 0..=16 {
            for z in 0..=2 {
                for x in 0..=2 {
                    values.push([
                        value(seed ^ 710, c[0] * 2 + x, y, c[1] * 2 + z),
                        value(seed ^ 991, c[0] * 2 + x, y, c[1] * 2 + z),
                    ]);
                }
            }
        }
        Self { values }
    }
    fn at(&self, x: usize, y: usize, z: usize) -> [f32; 2] {
        let (ix, iy, iz) = (x / 8, y / 8, z / 8);
        let (fx, fy, fz) = (
            smooth((x % 8) as f32 / 8.0),
            smooth((y % 8) as f32 / 8.0),
            smooth((z % 8) as f32 / 8.0),
        );
        std::array::from_fn(|channel| {
            let v = |dx: usize, dy: usize, dz: usize| {
                self.values[(ix + dx) + 3 * ((iz + dz) + 3 * (iy + dy))][channel]
            };
            mix(
                mix(
                    mix(v(0, 0, 0), v(1, 0, 0), fx),
                    mix(v(0, 0, 1), v(1, 0, 1), fx),
                    fz,
                ),
                mix(
                    mix(v(0, 1, 0), v(1, 1, 0), fx),
                    mix(v(0, 1, 1), v(1, 1, 1), fx),
                    fz,
                ),
                fy,
            )
        })
    }
}

pub(super) fn column(seed: u64, c: [i32; 2]) -> Vec<u8> {
    let [ox, oz] = c.map(|v| v * 16);
    let mut blocks = vec![AIR; 16 * 16 * WORLD_HEIGHT as usize];
    let caves = CaveField::new(seed, c);
    for z in 0..16usize {
        for x in 0..16usize {
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let s = surface(seed, wx, wz);
            for y in 0..=s.height as usize {
                let depth = s.height - y as i32;
                let field = caves.at(x, y, z);
                // Keep bedrock and shallow water beds intact.
                // Dry uplands occasionally expose cave mouths at the surface.
                if carved(s, y, field) {
                    continue;
                }
                let block = if y == 0 {
                    STONE
                } else if depth == 0 {
                    if s.water {
                        if s.biome == Biome::Wetland {
                            CLAY
                        } else {
                            GRAVEL
                        }
                    } else {
                        match s.biome {
                            Biome::Desert => SAND,
                            Biome::Snow => SNOW,
                            Biome::Wetland => CLAY,
                            _ => {
                                if s.height > 62 {
                                    STONE
                                } else {
                                    GRASS
                                }
                            }
                        }
                    }
                } else if depth < 4 {
                    match s.biome {
                        Biome::Desert => SANDSTONE,
                        Biome::Wetland => CLAY,
                        _ => DIRT,
                    }
                } else {
                    let ore = hash(
                        seed ^ 446,
                        wx.div_euclid(3),
                        (y as i32).div_euclid(3),
                        wz.div_euclid(3),
                    ) % 101;
                    if depth > 6 && ore < 3 {
                        COAL_ORE
                    } else if depth > 9 && ore < 5 {
                        IRON_ORE
                    } else if depth > 5 && ore < 7 {
                        COPPER_ORE
                    } else if y < 12 {
                        BASALT
                    } else {
                        STONE
                    }
                };
                blocks[x + 16 * (z + 16 * y)] = block;
            }
            if s.water {
                for y in s.height + 1..=24 {
                    blocks[x + 16 * (z + 16 * y as usize)] = if y == 24 && s.biome == Biome::Snow {
                        ICE
                    } else {
                        WATER
                    };
                }
            }
            if s.biome == Biome::Wetland
                && !s.water
                && hash(seed ^ 510, wx, 0, wz).is_multiple_of(37)
            {
                blocks[x + 16 * (z + 16 * s.height as usize)] = MOSS_STONE;
            }
        }
    }
    // Enumerate roots in a halo; trunk-first writes and identical root order make
    // tree silhouettes identical on either side of positive/negative chunk seams.
    let mut trees = Vec::new();
    for gz in (oz - 3).div_euclid(5)..=(oz + 18).div_euclid(5) {
        for gx in (ox - 3).div_euclid(5)..=(ox + 18).div_euclid(5) {
            let h = hash(seed ^ 0xaaaa, gx, 0, gz);
            let x = gx * 5 + (h % 4) as i32;
            let z = gz * 5 + ((h >> 8) % 4) as i32;
            let s = surface(seed, x, z);
            let density = match s.biome {
                Biome::Forest => 82,
                Biome::Taiga => 70,
                Biome::Wetland => 35,
                Biome::Grassland => 12,
                Biome::Snow => 18,
                Biome::Desert => 0,
            };
            if s.water || s.height > 72 || (h >> 16) % 100 >= density {
                continue;
            }
            let root_caves = CaveField::new(seed, [x.div_euclid(16), z.div_euclid(16)]);
            if carved(
                s,
                s.height as usize,
                root_caves.at(
                    x.rem_euclid(16) as usize,
                    s.height as usize,
                    z.rem_euclid(16) as usize,
                ),
            ) {
                continue;
            }
            let pine = matches!(s.biome, Biome::Taiga | Biome::Snow);
            let birch = !pine && (h >> 24).is_multiple_of(3);
            let trunk = if pine {
                PINE_WOOD
            } else if birch {
                BIRCH_WOOD
            } else {
                WOOD
            };
            let leaf = if pine {
                PINE_LEAVES
            } else if birch {
                BIRCH_LEAVES
            } else {
                LEAVES
            };
            let height = if pine { 8 } else { 5 } + ((h >> 32) % 3) as i32;
            trees.push((x, z, s.height, height, trunk, leaf, pine));
        }
    }
    let index = |x: i32, y: i32, z: i32| -> Option<usize> {
        ((ox..ox + 16).contains(&x) && (oz..oz + 16).contains(&z) && (0..WORLD_HEIGHT).contains(&y))
            .then(|| (x - ox) as usize + 16 * ((z - oz) as usize + 16 * y as usize))
    };
    for &(x, z, ground, h, trunk, _, _) in &trees {
        for y in ground + 1..=ground + h {
            if let Some(i) = index(x, y, z) {
                if blocks[i] == AIR {
                    blocks[i] = trunk;
                }
            }
        }
    }
    for &(x, z, ground, h, _, leaf, pine) in &trees {
        for dy in if pine { 3 } else { h - 2 }..=h + 2 {
            let radius = if pine {
                ((h + 2 - dy) / 3).clamp(0, 3)
            } else if dy == h + 2 {
                1
            } else {
                2
            };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs() + dz.abs() > radius + 1 {
                        continue;
                    }
                    if let Some(i) = index(x + dx, ground + dy, z + dz) {
                        if blocks[i] == AIR {
                            blocks[i] = leaf;
                        }
                    }
                }
            }
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ecology_sites_cover_materials_and_emit_graphics_fixture() {
        let mut sites = std::collections::BTreeMap::new();
        let mut frozen = None;
        for z in (-768i32..768).step_by(16) {
            for x in (-768i32..768).step_by(16) {
                if x.abs() < 192 && z.abs() < 192 {
                    continue;
                }
                let s = surface(12345, x, z);
                if s.water && s.biome == Biome::Snow && frozen.is_none() {
                    frozen = Some((x, z));
                }
                if s.water || (s.biome == Biome::Snow && s.height < 65) {
                    continue;
                }
                let score = x * x + z * z;
                let entry = sites.entry(s.biome).or_insert((i32::MAX, 0, 0, 0));
                if score < entry.0 {
                    *entry = (score, x, z, s.height);
                }
            }
        }
        assert_eq!(sites.len(), 6);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        std::fs::create_dir_all(&root).unwrap();
        let mut found = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        for (biome, (_, x, z, h)) in sites {
            entries.push(format!(
                "{{\"biome\":\"{biome:?}\",\"x\":{},\"y\":{},\"z\":{}}}",
                x as f32 + 0.5,
                h + 18,
                z as f32 + 0.5
            ));
            for dz in -1..=1 {
                for dx in -1..=1 {
                    found.extend(column(
                        12345,
                        [x.div_euclid(16) + dx, z.div_euclid(16) + dz],
                    ));
                }
            }
        }
        let (x, z) = frozen.expect("cold climate must include frozen shallow lakes");
        found.extend(column(12345, [x.div_euclid(16), z.div_euclid(16)]));
        entries.push(format!(
            "{{\"biome\":\"FrozenLake\",\"x\":{},\"y\":42,\"z\":{}}}",
            x as f32 + 0.5,
            z as f32 + 0.5
        ));
        let mut cave = None;
        'columns: for c in [[20, 20], [-30, 50], [70, -80]] {
            let b = column(12345, c);
            for y in 5..60 {
                for z in 2..14 {
                    for x in 2..14 {
                        let air = (y..y + 4).all(|yy| {
                            (z - 1..=z + 1).all(|zz| {
                                (x - 1..=x + 1).all(|xx| b[xx + 16 * (zz + 16 * yy)] == AIR)
                            })
                        });
                        if air && b[x + 16 * (z + 16 * (y - 1))] != AIR {
                            cave = Some((c[0] * 16 + x as i32, y, c[1] * 16 + z as i32));
                            break 'columns;
                        }
                    }
                }
            }
        }
        let (x, y, z) = cave.expect("cave network must contain standing-height chambers");
        entries.push(format!(
            "{{\"biome\":\"Cave\",\"x\":{},\"y\":{y},\"z\":{}}}",
            x as f32 + 0.5,
            z as f32 + 0.5
        ));
        for block in [
            SAND,
            SANDSTONE,
            GRAVEL,
            CLAY,
            SNOW,
            ICE,
            MOSS_STONE,
            BASALT,
            BIRCH_WOOD,
            BIRCH_LEAVES,
            PINE_WOOD,
            PINE_LEAVES,
            COAL_ORE,
            IRON_ORE,
            COPPER_ORE,
        ] {
            assert!(
                found.contains(&block),
                "missing {}",
                BLOCK_IDS[block as usize]
            );
        }
        std::fs::write(
            root.join("ecology-sites.json"),
            format!("[{}]", entries.join(",")),
        )
        .unwrap();
    }
    #[test]
    fn climate_relief_and_seeds_are_diverse() {
        let mut biomes = std::collections::BTreeSet::new();
        let (mut lo, mut hi) = (128, 0);
        for z in (-2048..2048).step_by(32) {
            for x in (-2048..2048).step_by(32) {
                let s = surface(12345, x, z);
                biomes.insert(s.biome);
                lo = lo.min(s.height);
                hi = hi.max(s.height);
            }
        }
        assert_eq!(biomes.len(), 6);
        assert!(hi - lo > 55);
        assert_ne!(column(1, [20, -30]), column(2, [20, -30]));
    }
    #[test]
    fn caves_water_beds_ores_and_height_are_valid() {
        let mut found = std::collections::BTreeSet::new();
        let mut caves = 0;
        for c in [[20, 20], [-30, 50], [70, -80], [-90, -60], [40, 30]] {
            let b = column(12345, c);
            assert_eq!(b.len(), 128 * 256);
            assert!(b.iter().all(|&b| valid_block(b)));
            found.extend(b.iter().copied());
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(b[x + 16 * z], STONE);
                    let s = surface(12345, c[0] * 16 + x as i32, c[1] * 16 + z as i32);
                    for y in 4..s.height as usize - 4 {
                        if b[x + 16 * (z + 16 * y)] == AIR {
                            caves += 1;
                        }
                    }
                    if s.water {
                        assert_ne!(b[x + 16 * (z + 16 * s.height as usize)], AIR);
                    }
                }
            }
        }
        assert!(caves > 100);
        assert!(found.contains(&COAL_ORE));
        assert!(found.contains(&IRON_ORE));
        assert!(found.contains(&COPPER_ORE));
        assert!(found.contains(&BASALT));
    }
}

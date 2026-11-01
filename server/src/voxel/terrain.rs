use game_simulation::grid::Grid;
use protocol::*;

#[path = "ecology.rs"]
mod ecology;
#[path = "structures.rs"]
mod structures;

/// Highest dry, walkable surface with sky above and room for a player.
pub(super) fn standing_position(map: &Grid, x: i32, z: i32) -> Option<[f32; 3]> {
    for y in (0..WORLD_HEIGHT).rev() {
        let block = map.get([x, y, z]);
        if block == AIR {
            continue;
        }
        if matches!(
            block,
            255 | WATER
                | ICE
                | WOOD
                | LEAVES
                | BIRCH_WOOD
                | BIRCH_LEAVES
                | PINE_WOOD
                | PINE_LEAVES
                | CHEST
        ) || y + 3 >= WORLD_HEIGHT
        {
            return None;
        }
        let position = [x as f32 + 0.5, (y + 1) as f32, z as f32 + 0.5];
        return (!game_simulation::overlaps_grid(position, map)).then_some(position);
    }
    None
}

pub(super) fn spawn_sites(map: &Grid) -> Vec<[f32; 3]> {
    // Search only the bounded initial region; exploration must not move /spawn.
    let mut cells = Vec::new();
    for [cx, cz] in map.columns() {
        if !(-4..8).contains(&cx) || !(-4..8).contains(&cz) {
            continue;
        }
        for z in cz * 16..cz * 16 + 16 {
            for x in cx * 16..cx * 16 + 16 {
                cells.push([x, z]);
            }
        }
    }
    cells.sort_by_key(|p| {
        let [x, z] = p.map(|v| v.div_euclid(16));
        (
            (-x).max(x - 3).max(-z).max(z - 3).max(0),
            (p[0] - 28).pow(2) + (p[1] - 32).pow(2),
            p[1],
            p[0],
        )
    });
    cells
        .into_iter()
        .filter_map(|[x, z]| standing_position(map, x, z))
        .collect()
}

pub(super) fn generate(seed: u64) -> anyhow::Result<Grid> {
    let mut map = Grid::unknown();
    // Usually the first 4x4 columns suffice. Expand only if the origin is water
    // or lacks safe ground. Never create an artificial island/platform fallback.
    for radius in 0..=4 {
        for z in -radius..4 + radius {
            for x in -radius..4 + radius {
                ensure_column(&mut map, seed, [x, z])?;
            }
        }
        if spawn_sites(&map).len() >= 64 {
            return Ok(map);
        }
    }
    anyhow::bail!("出生区域没有足够的安全陆地，请使用其他世界种子。")
}

pub(super) fn validate_structures() -> anyhow::Result<()> {
    structures::catalog().map(|_| ())
}

/// World-coordinate generation is independent of exploration order.
pub(super) fn ensure_column(map: &mut Grid, seed: u64, c: [i32; 2]) -> anyhow::Result<()> {
    if map.has_column(c) || !valid_column(c) {
        return Ok(());
    }
    let mut blocks = ecology::column(seed, c);
    structures::apply_column(seed, c, &mut blocks)?;
    for (y, section) in blocks.as_chunks::<4096>().0.iter().enumerate() {
        map.apply_chunk([c[0] * 16, y as i32 * 16, c[1] * 16], section);
    }
    Ok(())
}

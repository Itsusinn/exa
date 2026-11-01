use super::*;
use std::io::Write;

pub(super) fn empty_containers(map: &Grid) -> BTreeMap<[i32; 3], ContainerSnapshot> {
    let mut result = BTreeMap::new();
    for origin in map.blocks.keys() {
        for y in origin[1]..origin[1] + 16 {
            for z in origin[2]..origin[2] + 16 {
                for x in origin[0]..origin[0] + 16 {
                    let position = [x, y, z];
                    if map.get(position) == CHEST {
                        result.insert(
                            position,
                            ContainerSnapshot {
                                position,
                                revision: 0,
                                items: vec![],
                            },
                        );
                    }
                }
            }
        }
    }
    result
}

pub(super) fn backup_version(path: &Path, bytes: &[u8], version: u32) -> Result<()> {
    for suffix in 0..1000 {
        let backup = path.with_extension(if suffix == 0 {
            format!("v{version}.bak")
        } else {
            format!("v{version}.{suffix}.bak")
        });
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(mut file) => {
                // Keep an independent copy of every immutable archive directory,
                // not only a manifest that still depends on the live directories.
                for extension in ["chunks", "players", "entities", "industry"] {
                    let source = path.with_extension(extension);
                    if !source.exists() {
                        continue;
                    }
                    let destination =
                        std::path::PathBuf::from(format!("{}.{extension}", backup.display()));
                    std::fs::create_dir_all(&destination)?;
                    for entry in std::fs::read_dir(source)? {
                        let entry = entry?;
                        anyhow::ensure!(
                            entry.file_type()?.is_file(),
                            "unexpected archive directory entry"
                        );
                        let dest = destination.join(entry.file_name());
                        let data = std::fs::read(entry.path())?;
                        let mut copy = std::fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(dest)?;
                        copy.write_all(&data)?;
                        copy.sync_all()?;
                    }
                }
                file.write_all(bytes)
                    .context("back up old world before migration")?;
                file.sync_all()?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(&backup)? == bytes && archive_backup_complete(path, &backup)? {
                    return Ok(());
                }
            }
            Err(e) => return Err(e).context("create old world backup"),
        }
    }
    anyhow::bail!("no free world backup filename")
}

fn archive_backup_complete(path: &Path, backup: &Path) -> Result<bool> {
    for extension in ["chunks", "players", "entities", "industry"] {
        let source = path.with_extension(extension);
        if !source.exists() {
            continue;
        }
        let destination = std::path::PathBuf::from(format!("{}.{extension}", backup.display()));
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let dest = destination.join(entry.file_name());
            if !dest.is_file() || std::fs::read(dest)? != std::fs::read(entry.path())? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl VoxelWorld {
    pub(super) fn add_migration_chest(&mut self) -> Result<()> {
        // Prefer a visible location near spawn; never overwrite a voxel or spawn slot.
        let preferred = std::iter::once([26, 7, 30]);
        let candidates =
            (1..31).flat_map(|y| (1..31).flat_map(move |z| (1..27).map(move |x| [x, y, z])));
        let position = preferred
            .chain(candidates)
            .find(|&p| {
                self.terrain.get(p) == AIR
                    && self.terrain.get([p[0], p[1] + 1, p[2]]) == AIR
                    && self.terrain.solid([p[0], p[1] - 1, p[2]])
                    && [[1, 0], [-1, 0], [0, 1], [0, -1]].iter().any(|d| {
                        let side = [p[0] + d[0], p[1], p[2] + d[1]];
                        self.terrain.get(side) == AIR
                            && self.terrain.get([side[0], side[1] + 1, side[2]]) == AIR
                            && self.terrain.solid([side[0], side[1] - 1, side[2]])
                    })
            })
            .context("no safe location for migration chest; original save was not changed")?;
        self.terrain.set(position, CHEST);
        self.containers = empty_containers(&self.terrain);
        Ok(())
    }

    pub(super) fn restore_containers(&mut self, containers: Vec<ContainerSnapshot>) -> Result<()> {
        anyhow::ensure!(
            containers.len() == self.containers.len(),
            "container/voxel count mismatch"
        );
        let mut restored = BTreeMap::new();
        for c in containers {
            anyhow::ensure!(c.revision <= i64::MAX as u64, "invalid container revision");
            anyhow::ensure!(
                self.terrain.get(c.position) == CHEST,
                "container has no chest voxel"
            );
            anyhow::ensure!(
                c.items.len() <= CONTAINER_SLOTS,
                "container is over capacity"
            );
            let mut names = std::collections::HashSet::new();
            for item in &c.items {
                anyhow::ensure!(
                    !item.name.is_empty()
                        && item.name.len() <= 128
                        && (1..=CONTAINER_STACK_LIMIT).contains(&item.quantity)
                        && names.insert(&item.name),
                    "invalid container item"
                );
            }
            anyhow::ensure!(
                restored.insert(c.position, c).is_none(),
                "duplicate container coordinate"
            );
        }
        self.containers = restored;
        Ok(())
    }

    pub fn container_command(&mut self, id: u32, command: ContainerCommand) {
        let result = self.apply_container_command(id, command);
        if let Some(p) = self.players.get_mut(&id) {
            p.container_error = result.err().map_or_else(String::new, |e| e.to_string());
        }
    }

    fn apply_container_command(&mut self, id: u32, command: ContainerCommand) -> Result<()> {
        let p = self.players.get_mut(&id).context("unknown player")?;
        if command == ContainerCommand::Close {
            p.open_container = None;
            return Ok(());
        }
        let position = match &command {
            ContainerCommand::Open { position } | ContainerCommand::Transfer { position, .. } => {
                *position
            }
            ContainerCommand::Close => unreachable!(),
        };
        anyhow::ensure!(
            p.hp > 0
                && game_simulation::container_reachable(&self.terrain, p.body.position, position),
            "箱子太远或被遮挡。"
        );
        let c = self.containers.get_mut(&position).context("箱子不存在。")?;
        match command {
            ContainerCommand::Open { .. } => p.open_container = Some(position),
            ContainerCommand::Transfer {
                revision,
                inventory_revision,
                slot,
                target,
                quantity,
                deposit,
                ..
            } => {
                anyhow::ensure!(p.open_container == Some(position), "请先打开箱子。");
                anyhow::ensure!(revision == c.revision, "箱子内容已变化，请重试。");
                anyhow::ensure!(
                    inventory_revision == p.inventory.revision,
                    "物品栏已变化，请重试。"
                );
                anyhow::ensure!(
                    (1..=CONTAINER_STACK_LIMIT).contains(&quantity),
                    "无效物品或数量。"
                );
                let next_revision = c
                    .revision
                    .checked_add(1)
                    .filter(|r| *r <= i64::MAX as u64)
                    .context("container revision exhausted")?;
                let next_inventory_revision = inventory::next_revision(inventory_revision)?;
                let mut bag = p.inventory.clone();
                let mut contents = c.items.clone();
                let index = slot as usize;
                if deposit {
                    let source = bag
                        .slots
                        .get(index)
                        .and_then(|s| s.as_ref())
                        .context("源槽位为空或无效。")?
                        .clone();
                    anyhow::ensure!(source.quantity >= quantity, "物品数量不足。");
                    let existing = contents.iter().position(|s| s.name == source.name);
                    let total = existing.map_or(quantity, |i| contents[i].quantity + quantity);
                    anyhow::ensure!(
                        total <= CONTAINER_STACK_LIMIT
                            && (existing.is_some() || contents.len() < CONTAINER_SLOTS),
                        "箱子已满（24 类物品，每类最多 999 个）。"
                    );
                    if let Some(i) = existing {
                        contents[i].quantity = total;
                    } else {
                        contents.push(InventoryEntry {
                            name: source.name,
                            quantity,
                        });
                    }
                    if source.quantity == quantity {
                        bag.slots[index] = None;
                    } else {
                        bag.slots[index].as_mut().unwrap().quantity -= quantity;
                    }
                } else {
                    let source = contents.get(index).context("箱内物品已不存在。")?.clone();
                    anyhow::ensure!(source.quantity >= quantity, "物品数量不足。");
                    let item = InventoryEntry {
                        name: source.name,
                        quantity,
                    };
                    let moved = if let Some(to) = target {
                        let destination =
                            bag.slots.get_mut(to as usize).context("无效目标槽位。")?;
                        game_simulation::inventory::insert(std::slice::from_mut(destination), &item)
                    } else {
                        game_simulation::inventory::insert(&mut bag.slots, &item)
                    };
                    anyhow::ensure!(moved == quantity, "物品栏空间不足或目标物品不同。");
                    contents[index].quantity -= quantity;
                    if contents[index].quantity == 0 {
                        contents.remove(index);
                    }
                }
                // Commit both copies only after all validation succeeds.
                bag.revision = next_inventory_revision;
                p.inventory = bag;
                c.items = contents;
                c.revision = next_revision;
            }
            ContainerCommand::Close => unreachable!(),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CHEST_POS: [i32; 3] = [26, 7, 30];

    fn world() -> VoxelWorld {
        let mut w = VoxelWorld::fixture(7);
        w.npcs.clear();
        for id in 1..=2 {
            let (tx, _rx) = mpsc::channel(64);
            w.register_player(id, tx).unwrap();
            w.players.get_mut(&id).unwrap().body.position = [25.5 + id as f32, 7.0, 32.0];
            w.apply_container_command(
                id,
                ContainerCommand::Open {
                    position: CHEST_POS,
                },
            )
            .unwrap();
        }
        w.players.get_mut(&1).unwrap().inventory.slots[0] = Some(InventoryEntry {
            name: "Supplies".into(),
            quantity: 3,
        });
        w
    }
    fn transfer(revision: u64, quantity: u32, deposit: bool) -> ContainerCommand {
        ContainerCommand::Transfer {
            position: CHEST_POS,
            revision,
            inventory_revision: 0,
            slot: 0,
            target: None,
            quantity,
            deposit,
        }
    }
    fn tempdir() -> tempfile::TempDir {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/container-tests");
        std::fs::create_dir_all(&dir).unwrap();
        tempfile::tempdir_in(dir).unwrap()
    }

    #[test]
    fn death_and_leaving_reach_close_the_authoritative_container() {
        let mut w = world();
        let (tx, mut rx) = mpsc::channel(64);
        w.players.get_mut(&1).unwrap().tx = tx;
        w.players.get_mut(&1).unwrap().body.position = [50.0, 7.0, 50.0];
        w.tick();
        assert!(w.players[&1].open_container.is_none());
        while rx.try_recv().is_ok() {}
        w.players.get_mut(&1).unwrap().body.position = [26.5, 7.0, 32.0];
        w.apply_container_command(
            1,
            ContainerCommand::Open {
                position: CHEST_POS,
            },
        )
        .unwrap();
        w.players.get_mut(&1).unwrap().hp = 0;
        w.tick();
        assert!(w.players[&1].open_container.is_none());
        assert_eq!(w.players[&1].hp, 100);
    }

    #[test]
    fn old_large_chest_stacks_split_into_player_slots_and_full_bag_rolls_back() {
        let mut w = world();
        let position = CHEST_POS;
        w.containers.get_mut(&position).unwrap().items = vec![InventoryEntry {
            name: "Supplies".into(),
            quantity: 130,
        }];
        w.players.get_mut(&1).unwrap().inventory = PlayerInventory::default();
        w.apply_container_command(
            1,
            ContainerCommand::Transfer {
                position,
                revision: 0,
                inventory_revision: 0,
                slot: 0,
                target: None,
                quantity: 130,
                deposit: false,
            },
        )
        .unwrap();
        let bag = &w.players[&1].inventory;
        assert_eq!(
            bag.iter().map(|s| s.quantity).collect::<Vec<_>>(),
            vec![64, 64, 2]
        );
        w.containers.get_mut(&position).unwrap().items = vec![InventoryEntry {
            name: "Supplies".into(),
            quantity: 130,
        }];
        w.players
            .get_mut(&1)
            .unwrap()
            .inventory
            .slots
            .fill(Some(InventoryEntry {
                name: "Supplies".into(),
                quantity: 64,
            }));
        w.players.get_mut(&1).unwrap().inventory.slots[35] = None;
        let before = w.players[&1].inventory.clone();
        assert!(w
            .apply_container_command(
                1,
                ContainerCommand::Transfer {
                    position,
                    revision: 1,
                    inventory_revision: 1,
                    slot: 0,
                    target: None,
                    quantity: 65,
                    deposit: false
                }
            )
            .is_err());
        assert_eq!(w.players[&1].inventory, before);
        assert_eq!(w.containers[&position].items[0].quantity, 130);
        w.apply_container_command(
            1,
            ContainerCommand::Transfer {
                position,
                revision: 1,
                inventory_revision: 1,
                slot: 0,
                target: Some(35),
                quantity: 64,
                deposit: false,
            },
        )
        .unwrap();
        assert_eq!(w.containers[&position].items[0].quantity, 66);
        assert_eq!(
            w.players[&1].inventory.slots[35].as_ref().unwrap().quantity,
            64
        );
    }

    #[test]
    fn competing_transfers_conserve_items_and_reject_stale_revision() {
        let mut w = world();
        w.apply_container_command(1, transfer(0, 2, true)).unwrap();
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            1
        );
        assert_eq!(w.containers[&CHEST_POS].items[0].quantity, 2);
        w.apply_container_command(2, transfer(1, 2, false)).unwrap();
        assert!(w.apply_container_command(1, transfer(1, 2, false)).is_err());
        assert!(w.containers[&CHEST_POS].items.is_empty());
        assert_eq!(
            w.players[&2].inventory.slots[0].as_ref().unwrap().quantity,
            2
        );
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            1
        );
        assert_eq!(w.containers[&CHEST_POS].revision, 2);
    }

    #[test]
    fn invalid_and_full_transfers_leave_both_sides_unchanged() {
        let mut w = world();
        for quantity in [0, 4, u32::MAX] {
            assert!(w
                .apply_container_command(1, transfer(0, quantity, true))
                .is_err());
        }
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            3
        );
        assert_eq!(w.containers[&CHEST_POS].revision, 0);
        let c = w.containers.get_mut(&CHEST_POS).unwrap();
        c.items = (0..CONTAINER_SLOTS)
            .map(|i| InventoryEntry {
                name: format!("Item {i}"),
                quantity: 1,
            })
            .collect();
        assert!(w.apply_container_command(1, transfer(0, 1, true)).is_err());
        w.containers.get_mut(&CHEST_POS).unwrap().items[0] = InventoryEntry {
            name: "Supplies".into(),
            quantity: CONTAINER_STACK_LIMIT,
        };
        assert!(w.apply_container_command(1, transfer(0, 1, true)).is_err());
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            3
        );
        assert_eq!(
            w.containers[&CHEST_POS].items[0].quantity,
            CONTAINER_STACK_LIMIT
        );
    }

    #[test]
    fn transfers_require_open_container_reach_and_unobstructed_sight() {
        let mut w = world();
        w.apply_container_command(1, ContainerCommand::Close)
            .unwrap();
        assert!(w.apply_container_command(1, transfer(0, 1, true)).is_err());
        w.apply_container_command(
            1,
            ContainerCommand::Open {
                position: CHEST_POS,
            },
        )
        .unwrap();
        w.terrain.set([26, 8, 31], STONE);
        assert!(w.apply_container_command(1, transfer(0, 1, true)).is_err());
        w.terrain.set([26, 8, 31], AIR);
        w.players.get_mut(&1).unwrap().body.position = [50.0, 7.0, 50.0];
        assert!(w.apply_container_command(1, transfer(0, 1, true)).is_err());
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            3
        );
    }

    #[test]
    fn version_one_is_backed_up_and_migrated_without_overwriting_blocks() {
        let dir = tempdir();
        let path = dir.path().join("world3d.bin");
        let mut terrain = legacy_fixture::generate(7);
        terrain.set(CHEST_POS, STONE); // Occupied preferred position must be preserved.
        let mut bytes = b"DNG3D001".to_vec();
        bytes.extend(
            rmp_serde::to_vec(&(1u32, WORLD_SIZE, 7u64, &terrain.legacy_blocks())).unwrap(),
        );
        std::fs::write(&path, &bytes).unwrap();
        let mut w = VoxelWorld::load(&path).unwrap();
        assert_eq!(std::fs::read(path.with_extension("v1.bak")).unwrap(), bytes);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "loading never replaces the original"
        );
        assert_eq!(w.terrain.get(CHEST_POS), STONE);
        assert_eq!(w.containers.len(), 1);
        let pos = *w.containers.keys().next().unwrap();
        for (old, new) in terrain
            .legacy_blocks()
            .iter()
            .zip(w.terrain.legacy_blocks())
        {
            assert!(*old == new || (*old == AIR && new == CHEST));
        }
        w.containers
            .get_mut(&pos)
            .unwrap()
            .items
            .push(InventoryEntry {
                name: "Supplies".into(),
                quantity: 5,
            });
        w.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.containers, w.containers);
        assert_eq!(restored.terrain.blocks, w.terrain.blocks);
        assert_eq!(std::fs::read(path.with_extension("v1.bak")).unwrap(), bytes);
    }

    #[test]
    fn corrupt_container_save_and_failed_backup_preserve_original() {
        let dir = tempdir();
        let path = dir.path().join("world3d.bin");
        let w = VoxelWorld::fixture(7);
        let mut saved = Save {
            version: 2,
            size: WORLD_SIZE,
            seed: 7,
            blocks: w.terrain.legacy_blocks(),
            containers: vec![],
            player_archive: None,
            items: None,
            chunk_archive: None,
            entity_archive: None,
            generated_columns: None,
            industry_archive: None,
        };
        let mut bytes = b"DNG3D001".to_vec();
        bytes.extend(rmp_serde::to_vec(&saved).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        saved.version = 1;
        saved.player_archive = None;
        saved.items = None;
        saved.blocks[Grid::index(CHEST_POS).unwrap()] = AIR;
        bytes.truncate(8);
        bytes.extend(rmp_serde::to_vec(&saved).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        std::fs::create_dir(path.with_extension("v1.bak")).unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

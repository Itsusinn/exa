use super::*;
use game_simulation::{inventory::insert, overlaps, targeting::target_block};

impl VoxelWorld {
    pub fn edit_block(&mut self, id: u32, command: BlockEdit) {
        let result = self.apply_block_edit(id, command);
        if let Some(p) = self.players.get_mut(&id) {
            p.container_error = result.err().map_or_else(String::new, |e| e.to_string());
        }
    }

    fn apply_block_edit(&mut self, id: u32, c: BlockEdit) -> Result<()> {
        let p = self.players.get(&id).context("unknown player")?;
        anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法编辑方块。");
        anyhow::ensure!(self.tick >= p.edit_after, "操作过快，请稍后重试。");
        anyhow::ensure!(
            c.inventory_revision == p.inventory.revision,
            "物品栏已变化，请重试。"
        );
        let hit = target_block(&self.terrain, p.body.position, c.yaw, c.pitch)
            .context("目标太远或被遮挡。")?;
        anyhow::ensure!(
            hit.position == c.target && self.terrain.get(hit.position) == c.expected,
            "目标方块已变化，请重新瞄准。"
        );
        let mut bag = p.inventory.clone();
        bag.revision = inventory::next_revision(bag.revision)?;
        let (position, block) = if c.place {
            anyhow::ensure!(
                c.slot < HOTBAR_SLOTS as u8 && c.slot == bag.selected,
                "请选择当前快捷栏方块。"
            );
            let item = bag.slots[c.slot as usize]
                .as_mut()
                .context("当前快捷栏为空。")?;
            let block = item_block(&item.name).context("当前物品不是可放置方块。")?;
            anyhow::ensure!(item.quantity > 0 && hit.normal != [0; 3], "无法放置方块。");
            let position = std::array::from_fn(|i| hit.position[i] + hit.normal[i]);
            anyhow::ensure!(
                protocol::valid_voxel(position)
                    && position[1] > 0
                    && self.terrain.get(position) == AIR
                    && !self.equipment_occupies(position),
                "放置位置被占用或超出世界边界。"
            );
            let collides = |feet| overlaps(feet, &|cell| cell == position);
            anyhow::ensure!(
                !self.players.values().any(|p| collides(p.body.position))
                    && !self
                        .npcs
                        .query::<&Npc>()
                        .iter()
                        .any(|n| collides(n.body.position)),
                "不能在玩家或角色身体内放置方块。"
            );
            anyhow::ensure!(
                !self
                    .items
                    .iter()
                    .any(|item| item.position.map(|v| v.floor() as i32) == position),
                "请先拾取放置位置的物品。"
            );
            item.quantity -= 1;
            if item.quantity == 0 {
                bag.slots[c.slot as usize] = None;
            }
            (position, block)
        } else {
            anyhow::ensure!(hit.position[1] > 0, "世界底层不可破坏。");
            let name = block_item_name(c.expected).context("该方块不可破坏。")?;
            if c.expected == CHEST {
                let chest = self
                    .containers
                    .get(&hit.position)
                    .context("箱子状态不可用。")?;
                anyhow::ensure!(chest.items.is_empty(), "请先取空箱子再破坏。");
            }
            anyhow::ensure!(
                insert(
                    &mut bag.slots,
                    &InventoryEntry {
                        name: name.into(),
                        quantity: 1
                    }
                ) == 1,
                "物品栏已满，无法收取方块。"
            );
            (hit.position, AIR)
        };
        // Commit terrain, container and inventory together after all checks pass.
        self.terrain.set(position, block);
        if c.expected == CHEST && !c.place {
            self.containers.remove(&position);
            for player in self.players.values_mut() {
                if player.open_container == Some(position) {
                    player.open_container = None;
                    player.container_error = "箱子已被破坏。".into();
                }
            }
        }
        if block == CHEST {
            self.containers.insert(
                position,
                ContainerSnapshot {
                    position,
                    revision: 0,
                    items: vec![],
                },
            );
        }
        let p = self.players.get_mut(&id).unwrap();
        p.inventory = bag;
        p.edit_after = self.tick + 6;
        self.dirty_chunks.insert(Grid::origin(position));
        Ok(())
    }

    pub(super) fn broadcast_terrain(&mut self) {
        let dirty = std::mem::take(&mut self.dirty_chunks);
        let messages: Vec<_> = dirty
            .into_iter()
            .map(|origin| ServerMessage::Chunk3d {
                origin,
                blocks: self.terrain.chunk(origin),
            })
            .collect();
        let mut slow = vec![];
        for (&id, p) in &self.players {
            if messages
                .iter()
                .filter(|m| match m {
                    ServerMessage::Chunk3d { origin, .. } => {
                        p.sent_columns.contains(&[origin[0] / 16, origin[2] / 16])
                    }
                    _ => false,
                })
                .any(|m| p.tx.try_send(m.clone()).is_err())
            {
                slow.push(id);
            }
        }
        for id in slow {
            self.unregister_player(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn world() -> (VoxelWorld, mpsc::Receiver<ServerMessage>) {
        let mut map = Grid::empty();
        for z in 0..64 {
            for x in 0..64 {
                map.set([x, 0, z], STONE);
            }
        }
        let mut w = VoxelWorld::with_map(1, map);
        w.npcs.clear();
        w.items.clear();
        let (tx, mut rx) = mpsc::channel(128);
        w.register_profile(1, [1; 16], "Builder".into(), tx)
            .unwrap();
        while rx.try_recv().is_ok() {}
        w.players.get_mut(&1).unwrap().body.position = [10.5, 1.0, 10.5];
        w.terrain.set([10, 2, 7], STONE);
        w.terrain.set([10, 2, 6], STONE);
        w.stream_chunks();
        while rx.try_recv().is_ok() {}
        (w, rx)
    }
    fn command(w: &VoxelWorld, place: bool) -> BlockEdit {
        let p = &w.players[&1];
        let hit = target_block(&w.terrain, p.body.position, 0.0, 0.0).unwrap();
        BlockEdit {
            target: hit.position,
            expected: w.terrain.get(hit.position),
            yaw: 0.0,
            pitch: 0.0,
            place,
            slot: p.inventory.selected,
            inventory_revision: p.inventory.revision,
        }
    }
    #[test]
    fn negative_coordinate_edits_use_euclidean_chunk_ownership() {
        let (mut w, mut rx) = world();
        super::super::terrain::ensure_column(&mut w.terrain, 1, [-1, 0]).unwrap();
        // Explicit empty test chamber; natural terrain can now occupy this height.
        for z in 5..=11 {
            for x in -12..=-9 {
                for y in 20..=23 {
                    w.terrain.set([x, y, z], AIR);
                }
            }
        }
        w.players.get_mut(&1).unwrap().body.position = [-10.5, 20.0, 10.5];
        w.terrain.set([-11, 21, 7], STONE);
        w.terrain.set([-11, 21, 6], STONE);
        for _ in 0..13 {
            w.stream_chunks();
            while rx.try_recv().is_ok() {}
        }
        w.apply_block_edit(1, command(&w, false)).unwrap();
        w.broadcast_terrain();
        match rx.try_recv().unwrap() {
            ServerMessage::Chunk3d { origin, blocks } => {
                assert_eq!(origin, [-16, 16, 0]);
                let mut map = Grid::unknown();
                map.apply_chunk(origin, &blocks);
                assert_eq!(map.get([-11, 21, 7]), AIR);
            }
            _ => panic!("expected negative section edit"),
        }
        w.tick += 6;
        w.apply_block_edit(1, command(&w, true)).unwrap();
        assert_eq!(w.terrain.get([-11, 21, 7]), STONE);
        assert!(w.players[&1].inventory.is_empty());
    }
    #[test]
    fn every_new_block_can_be_collected_placed_and_saved_above_old_ceiling() {
        let (mut w, _rx) = world();
        w.players.get_mut(&1).unwrap().body.position = [10.5, 65.0, 10.5];
        w.terrain.set([10, 66, 6], STONE);
        for block in SAND..=COPPER_ORE {
            w.terrain.set([10, 66, 7], block);
            w.tick += 6;
            w.apply_block_edit(1, command(&w, false)).unwrap();
            assert_eq!(w.terrain.get([10, 66, 7]), AIR);
            assert_eq!(
                w.players[&1].inventory.slots[0].as_ref().unwrap().name,
                block_item_name(block).unwrap()
            );
            w.tick += 6;
            w.apply_block_edit(1, command(&w, true)).unwrap();
            assert_eq!(w.terrain.get([10, 66, 7]), block);
            assert!(w.players[&1].inventory.is_empty());
            w.terrain.set([block as i32, 100, 12], block);
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ecology-edit");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        assert_eq!(VoxelWorld::load(&path).unwrap().terrain, w.terrain);
    }
    #[test]
    fn break_place_conserve_inventory_broadcast_and_persist() {
        let (mut w, mut rx) = world();
        let c = command(&w, false);
        w.apply_block_edit(1, c.clone()).unwrap();
        assert_eq!(w.terrain.get([10, 2, 7]), AIR);
        assert_eq!(
            w.players[&1].inventory.slots[0],
            Some(InventoryEntry {
                name: "石块".into(),
                quantity: 1
            })
        );
        assert!(w.apply_block_edit(1, c.clone()).is_err());
        w.broadcast_terrain();
        match rx.try_recv().unwrap() {
            ServerMessage::Chunk3d { origin, blocks } => {
                assert_eq!(origin, [0, 0, 0]);
                let mut map = Grid::unknown();
                assert!(map.apply_chunk(origin, &blocks));
                assert_eq!(map.get([10, 2, 7]), AIR);
            }
            other => panic!("expected terrain update: {other:?}"),
        }
        w.tick += 6;
        assert!(w.apply_block_edit(1, c).is_err()); // Stale inventory/target cannot duplicate drops.
        w.apply_block_edit(1, command(&w, true)).unwrap();
        assert_eq!(w.terrain.get([10, 2, 7]), STONE);
        assert!(w.players[&1].inventory.is_empty());
        w.tick += 6;
        w.apply_block_edit(1, command(&w, false)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let mut restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.get([10, 2, 7]), AIR);
        let (tx, _rx) = mpsc::channel(64);
        restored
            .register_profile(2, [1; 16], "Renamed".into(), tx)
            .unwrap();
        assert_eq!(
            restored.players[&2].inventory.slots[0]
                .as_ref()
                .unwrap()
                .quantity,
            1
        );
    }
    #[test]
    fn denied_edits_preserve_world_and_items() {
        let (mut w, _rx) = world();
        let valid = command(&w, false);
        for c in [
            BlockEdit {
                yaw: f32::NAN,
                ..valid.clone()
            },
            BlockEdit {
                target: [10, 2, 6],
                ..valid.clone()
            },
            BlockEdit {
                inventory_revision: 3,
                ..valid.clone()
            },
            BlockEdit {
                expected: WOOD,
                ..valid.clone()
            },
        ] {
            assert!(w.apply_block_edit(1, c).is_err());
        }
        w.players.get_mut(&1).unwrap().body.position[2] = 15.0;
        assert!(w.apply_block_edit(1, valid.clone()).is_err());
        w.players.get_mut(&1).unwrap().body.position[2] = 10.5;
        w.players
            .get_mut(&1)
            .unwrap()
            .inventory
            .slots
            .fill(Some(InventoryEntry {
                name: "石块".into(),
                quantity: 64,
            }));
        let before = w.players[&1].inventory.clone();
        assert!(w.apply_block_edit(1, valid).is_err());
        assert_eq!(w.terrain.get([10, 2, 7]), STONE);
        assert_eq!(w.players[&1].inventory, before);
        // Placement in the builder or another actor must not consume items.
        w.terrain.set([10, 2, 9], STONE);
        assert!(w.apply_block_edit(1, command(&w, true)).is_err());
        w.terrain.set([10, 2, 9], AIR);
        let (tx, _other) = mpsc::channel(64);
        w.register_player(2, tx).unwrap();
        w.players.get_mut(&2).unwrap().body.position = [10.5, 1.0, 8.5];
        assert!(w.apply_block_edit(1, command(&w, true)).is_err());
        assert_eq!(w.terrain.get([10, 2, 8]), AIR);
        assert_eq!(w.players[&1].inventory, before);
    }
    #[test]
    fn chests_must_be_empty_and_replacement_has_fresh_container() {
        let (mut w, _rx) = world();
        let pos = [10, 2, 7];
        w.terrain.set(pos, CHEST);
        w.containers.insert(
            pos,
            ContainerSnapshot {
                position: pos,
                revision: 4,
                items: vec![InventoryEntry {
                    name: "Supplies".into(),
                    quantity: 2,
                }],
            },
        );
        assert!(w.apply_block_edit(1, command(&w, false)).is_err());
        assert_eq!(w.containers[&pos].items[0].quantity, 2);
        w.containers.get_mut(&pos).unwrap().items.clear();
        let (tx, _other) = mpsc::channel(64);
        w.register_player(2, tx).unwrap();
        w.players.get_mut(&2).unwrap().open_container = Some(pos);
        w.apply_block_edit(1, command(&w, false)).unwrap();
        assert!(!w.containers.contains_key(&pos));
        assert!(w.players[&2].open_container.is_none());
        w.tick += 6;
        w.apply_block_edit(1, command(&w, true)).unwrap();
        assert_eq!(w.terrain.get(pos), CHEST);
        assert_eq!(w.containers[&pos].revision, 0);
        assert!(w.containers[&pos].items.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        assert!(VoxelWorld::load(&path).unwrap().containers[&pos]
            .items
            .is_empty());
    }
    #[test]
    fn floor_and_unknown_space_are_protected_and_slow_peers_do_not_block() {
        let (mut w, _rx) = world();
        let feet = [10.5, 1.0, 10.5];
        w.terrain.set([10, 0, 10], STONE);
        let c = BlockEdit {
            target: [10, 0, 10],
            expected: STONE,
            yaw: 0.0,
            pitch: -1.5,
            place: false,
            slot: 0,
            inventory_revision: 0,
        };
        assert_eq!(w.players[&1].body.position, feet);
        assert!(w.apply_block_edit(1, c).is_err());
        assert_eq!(w.terrain.get([10, 0, 10]), STONE);
        let (tx, _full) = mpsc::channel(32);
        for seq in 0..32 {
            tx.try_send(ServerMessage::Pong { seq }).unwrap();
        }
        w.register_player(2, tx).unwrap();
        w.players.get_mut(&2).unwrap().sent_columns.insert([0, 0]);
        w.apply_block_edit(1, command(&w, false)).unwrap();
        w.broadcast_terrain();
        assert!(w.players.contains_key(&1));
        assert!(!w.players.contains_key(&2));
    }
}

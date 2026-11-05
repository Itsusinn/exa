use super::*;
use exa_simulation::inventory::{insert, move_stack, quick_move};

pub(super) fn next_revision(revision: u64) -> Result<u64> {
    revision
        .checked_add(1)
        .filter(|r| *r <= i64::MAX as u64)
        .context("inventory revision exhausted")
}

impl VoxelWorld {
    pub fn inventory_command(&mut self, id: u32, command: InventoryCommand) {
        let result = self.apply_inventory_command(id, command);
        if let Some(p) = self.players.get_mut(&id) {
            p.container_error = result.err().map_or_else(String::new, |e| e.to_string());
        }
    }

    fn apply_inventory_command(&mut self, id: u32, command: InventoryCommand) -> Result<()> {
        let p = self.players.get_mut(&id).context("unknown player")?;
        anyhow::ensure!(p.hp > 0, "当前无法操作物品栏。");
        if let InventoryCommand::Select { slot } = command {
            anyhow::ensure!((slot as usize) < HOTBAR_SLOTS, "无效快捷栏槽位。");
            p.inventory.selected = slot;
            return Ok(());
        }
        let revision = match command {
            InventoryCommand::Move { revision, .. }
            | InventoryCommand::QuickMove { revision, .. } => revision,
            _ => unreachable!(),
        };
        anyhow::ensure!(revision == p.inventory.revision, "物品栏已变化，请重试。");
        let mut updated = p.inventory.clone();
        updated.revision = next_revision(revision)?;
        let result = match command {
            InventoryCommand::Move {
                from, to, quantity, ..
            } => move_stack(&mut updated, from as usize, to as usize, quantity),
            InventoryCommand::QuickMove { slot, .. } => quick_move(&mut updated, slot as usize),
            _ => unreachable!(),
        };
        result.map_err(anyhow::Error::msg)?;
        p.inventory = updated;
        Ok(())
    }

    pub(super) fn pickup_item(&mut self, id: u32, item_index: usize) {
        let p = self.players.get_mut(&id).unwrap();
        let Ok(revision) = next_revision(p.inventory.revision) else {
            return;
        };
        let item = &self.items[item_index];
        if insert(
            &mut p.inventory.slots,
            &InventoryEntry {
                name: item.name.clone(),
                quantity: 1,
            },
        ) == 0
        {
            p.container_error = "物品栏已满，物品留在地面。".into();
            return;
        }
        p.inventory.revision = revision;
        p.container_error.clear();
        let item = self.items.remove(item_index);
        self.note(format!("Player {id} picked up {}.", item.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn world() -> VoxelWorld {
        let mut w = VoxelWorld::fixture(1);
        let (tx, _rx) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        w
    }

    #[test]
    fn inventory_commands_validate_revisions_and_hotbar_bounds() {
        let mut w = world();
        w.players.get_mut(&1).unwrap().inventory.slots[0] = Some(InventoryEntry {
            name: "Supplies".into(),
            quantity: 16,
        });
        let command = InventoryCommand::Move {
            revision: 0,
            from: 0,
            to: 9,
            quantity: 8,
        };
        w.apply_inventory_command(1, command.clone()).unwrap();
        let before = w.players[&1].inventory.clone();
        assert!(w.apply_inventory_command(1, command).is_err());
        assert_eq!(w.players[&1].inventory, before);
        w.apply_inventory_command(1, InventoryCommand::Select { slot: 8 })
            .unwrap();
        assert_eq!(w.players[&1].inventory.selected, 8);
        assert!(w
            .apply_inventory_command(1, InventoryCommand::Select { slot: 9 })
            .is_err());
        w.apply_inventory_command(
            1,
            InventoryCommand::QuickMove {
                revision: 1,
                slot: 9,
            },
        )
        .unwrap();
        assert!(w.players[&1].inventory.slots[9].is_none());
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            16
        );
        assert_eq!(w.players[&1].inventory.selected, 8);
    }

    #[test]
    fn full_inventory_does_not_delete_ground_item() {
        let mut w = world();
        w.players
            .get_mut(&1)
            .unwrap()
            .inventory
            .slots
            .fill(Some(InventoryEntry {
                name: "Supplies".into(),
                quantity: 64,
            }));
        let items = w.items.clone();
        w.pickup_item(1, 0);
        assert_eq!(w.items, items);
        assert_eq!(w.players[&1].inventory.revision, 0);
        w.players.get_mut(&1).unwrap().inventory.slots[35] = None;
        w.pickup_item(1, 0);
        assert!(w.items.is_empty());
        assert_eq!(
            w.players[&1].inventory.slots[35].as_ref().unwrap().quantity,
            1
        );
        assert_eq!(w.players[&1].inventory.revision, 1);
    }
}

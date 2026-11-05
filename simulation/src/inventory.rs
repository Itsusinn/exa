//! Pure slot operations. Callers commit a validated copy atomically.
use exa_protocol::{
    InventoryEntry, PlayerInventory, HOTBAR_SLOTS, INVENTORY_SLOTS, ITEM_STACK_LIMIT,
};

pub fn insert(slots: &mut [Option<InventoryEntry>], item: &InventoryEntry) -> u32 {
    let mut left = item.quantity;
    for slot in slots.iter_mut().flatten().filter(|s| s.name == item.name) {
        let moved = left.min(ITEM_STACK_LIMIT.saturating_sub(slot.quantity));
        slot.quantity += moved;
        left -= moved;
    }
    for slot in slots.iter_mut().filter(|s| s.is_none()) {
        let moved = left.min(ITEM_STACK_LIMIT);
        if moved == 0 {
            break;
        }
        *slot = Some(InventoryEntry {
            name: item.name.clone(),
            quantity: moved,
        });
        left -= moved;
    }
    item.quantity - left
}

pub fn move_stack(
    inventory: &mut PlayerInventory,
    from: usize,
    to: usize,
    quantity: u32,
) -> Result<(), &'static str> {
    if from >= INVENTORY_SLOTS || to >= INVENTORY_SLOTS || from == to || quantity == 0 {
        return Err("无效槽位或数量。");
    }
    let source = inventory.slots[from].clone().ok_or("源槽位为空。")?;
    if quantity > source.quantity {
        return Err("物品数量不足。");
    }
    match &mut inventory.slots[to] {
        Some(target) if target.name == source.name => {
            let moved = quantity.min(ITEM_STACK_LIMIT - target.quantity);
            if moved == 0 {
                return Err("目标堆叠已满。");
            }
            target.quantity += moved;
            inventory.slots[from].as_mut().unwrap().quantity -= moved;
        }
        Some(_) => {
            if quantity != source.quantity {
                return Err("拆分的物品不能交换不同物品。");
            }
            inventory.slots.swap(from, to);
            return Ok(());
        }
        None => {
            inventory.slots[to] = Some(InventoryEntry {
                name: source.name,
                quantity,
            });
            inventory.slots[from].as_mut().unwrap().quantity -= quantity;
        }
    }
    if inventory.slots[from]
        .as_ref()
        .is_some_and(|s| s.quantity == 0)
    {
        inventory.slots[from] = None;
    }
    Ok(())
}

pub fn quick_move(inventory: &mut PlayerInventory, from: usize) -> Result<(), &'static str> {
    if from >= INVENTORY_SLOTS {
        return Err("无效槽位。");
    }
    let source = inventory.slots[from].clone().ok_or("源槽位为空。")?;
    let range = if from < HOTBAR_SLOTS {
        HOTBAR_SLOTS..INVENTORY_SLOTS
    } else {
        0..HOTBAR_SLOTS
    };
    let moved = insert(&mut inventory.slots[range], &source);
    if moved == 0 {
        return Err("目标物品栏已满。");
    }
    if moved == source.quantity {
        inventory.slots[from] = None;
    } else {
        inventory.slots[from].as_mut().unwrap().quantity -= moved;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(name: &str, quantity: u32) -> InventoryEntry {
        InventoryEntry {
            name: name.into(),
            quantity,
        }
    }
    fn count(bag: &PlayerInventory) -> u32 {
        bag.iter().map(|s| s.quantity).sum()
    }

    #[test]
    fn split_merge_swap_and_quick_move_preserve_items() {
        let mut bag = PlayerInventory::default();
        assert_eq!(insert(&mut bag.slots, &item("Supplies", 80)), 80);
        assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 64);
        assert_eq!(bag.slots[1].as_ref().unwrap().quantity, 16);
        move_stack(&mut bag, 0, 9, 32).unwrap();
        assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 32);
        assert_eq!(bag.slots[9].as_ref().unwrap().quantity, 32);
        move_stack(&mut bag, 9, 1, 32).unwrap();
        assert!(bag.slots[9].is_none());
        assert_eq!(bag.slots[1].as_ref().unwrap().quantity, 48);
        move_stack(&mut bag, 0, 1, 32).unwrap(); // Only available room is merged.
        assert_eq!(bag.slots[1].as_ref().unwrap().quantity, 64);
        assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 16);
        bag.slots[10] = Some(item("Other", 4));
        move_stack(&mut bag, 0, 10, 16).unwrap();
        assert_eq!(bag.slots[0].as_ref().unwrap().name, "Other");
        quick_move(&mut bag, 1).unwrap();
        assert!(bag.slots[1].is_none());
        assert_eq!(bag.slots[10].as_ref().unwrap().quantity, 64);
        assert_eq!(count(&bag), 84);
    }

    #[test]
    fn invalid_moves_are_noops_and_full_inventory_accepts_nothing() {
        let mut bag = PlayerInventory::default();
        bag.slots[0] = Some(item("Supplies", 12));
        bag.slots[1] = Some(item("Other", 4));
        for (from, to, quantity) in [
            (0, 36, 1),
            (36, 0, 1),
            (0, 0, 1),
            (0, 2, 0),
            (0, 2, 13),
            (0, 1, 6),
            (2, 1, 1),
        ] {
            let before = bag.clone();
            assert!(move_stack(&mut bag, from, to, quantity).is_err());
            assert_eq!(bag, before);
        }
        bag.slots.fill(Some(item("Supplies", 64)));
        let before = bag.clone();
        assert_eq!(insert(&mut bag.slots, &item("Supplies", 1)), 0);
        assert!(quick_move(&mut bag, 0).is_err());
        assert_eq!(bag, before);
    }
}

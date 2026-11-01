//! Authoritative equipment transactions and atomic publication of background fluid results.
use super::*;
use game_simulation::{
    equipment::{collision_boxes, occupied_cells, placement_origin, target},
    inventory::insert,
    HEIGHT, RADIUS,
};

impl VoxelWorld {
    pub(super) fn industry_kit(&mut self, id: u32) -> Result<()> {
        let p = self.players.get_mut(&id).context("玩家尚未连接。")?;
        let mut bag = p.inventory.clone();
        for (name, quantity) in [
            (TANK_ITEM, 2),
            (PIPE_ITEM, 16),
            (PUMP_ITEM, 2),
            (VALVE_ITEM, 2),
            (protocol::VESSEL_ITEM, 4),
            (protocol::HEAT_EXCHANGER_ITEM, 1),
            (protocol::PIPE_RACK_ITEM, 8),
        ] {
            anyhow::ensure!(
                insert(
                    &mut bag.slots,
                    &InventoryEntry {
                        name: name.into(),
                        quantity
                    }
                ) == quantity,
                "物品栏空间不足，未领取任何工业补给。"
            );
        }
        bag.revision = inventory::next_revision(bag.revision)?;
        p.inventory = bag;
        Ok(())
    }

    pub fn equipment_command(&mut self, player: u32, command: EquipmentCommand) {
        if matches!(
            command,
            EquipmentCommand::BeginPipe { .. }
                | EquipmentCommand::LayPipe { .. }
                | EquipmentCommand::CancelPipe { .. }
        ) {
            self.pipe_command(player, command);
            return;
        }
        let result = self.apply_equipment_command(player, command);
        if let Some(p) = self.players.get_mut(&player) {
            p.container_error = result.err().map_or_else(String::new, |e| e.to_string());
        }
    }

    fn apply_equipment_command(&mut self, player: u32, command: EquipmentCommand) -> Result<()> {
        if let EquipmentCommand::ConfigureThermal {
            id,
            revision,
            yaw,
            pitch,
            ref settings,
        } = command
        {
            return self.configure_thermal(player, id, revision, yaw, pitch, settings.clone());
        }
        if let EquipmentCommand::Configure {
            id,
            revision,
            yaw,
            pitch,
            enabled,
            opening,
            pump_reversed,
        } = command
        {
            return self.configure_equipment(
                player,
                id,
                revision,
                yaw,
                pitch,
                enabled,
                opening,
                pump_reversed,
            );
        }
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法操作设备。");
        anyhow::ensure!(self.tick >= p.edit_after, "操作过快，请稍后重试。");
        let mut bag = p.inventory.clone();
        let revision = inventory::next_revision(self.industry_revision)?;
        let mut next_id = self.next_equipment_id;
        let (add, remove) = match command {
            EquipmentCommand::Place {
                target: expected,
                yaw,
                pitch,
                kind,
                axis,
                inventory_revision,
                slot,
            } => {
                anyhow::ensure!(inventory_revision == bag.revision, "物品栏已变化，请重试。");
                anyhow::ensure!(
                    self.terrain.equipment.len() < MAX_EQUIPMENT,
                    "世界设备数量已达到 {MAX_EQUIPMENT} 台上限。"
                );
                anyhow::ensure!(
                    slot < HOTBAR_SLOTS as u8 && slot == bag.selected,
                    "请选择当前快捷栏设备。"
                );
                let item = bag.slots[slot as usize]
                    .as_mut()
                    .context("当前快捷栏为空。")?;
                anyhow::ensure!(
                    item.name == kind.item_name() && item.quantity > 0,
                    "当前物品与设备不符。"
                );
                let hit = target(&self.terrain, p.body.position, yaw, pitch)
                    .context("目标太远或被遮挡。")?;
                anyhow::ensure!(
                    hit.position == expected && hit.normal != [0; 3],
                    "目标已变化，请重新瞄准。"
                );
                let origin = placement_origin(hit, kind);
                let equipment = EquipmentSnapshot {
                    id: next_id,
                    revision,
                    kind,
                    origin,
                    axis,
                    contents: vec![],
                    internal_energy_j: 0.0,
                    enabled: false,
                    opening: 1.0,
                    pump_reversed: false,
                    pipe_ends: None,
                    thermal: (kind == EquipmentKind::HeatExchanger)
                        .then(protocol::ThermalSettings::default),
                    reading: None,
                };
                anyhow::ensure!(equipment.valid(), "设备方向或位置无效。");
                next_id = next_id
                    .checked_add(1)
                    .filter(|id| *id <= i64::MAX as u64)
                    .context("设备标识已耗尽。")?;
                let cells = occupied_cells(&equipment);
                anyhow::ensure!(
                    cells.iter().all(|&cell| self.terrain.get(cell) == AIR)
                        && !self.equipment_conflicts(&equipment),
                    "设备占用范围与地形、其他设备或未加载区块冲突。"
                );
                anyhow::ensure!(
                    !self
                        .items
                        .iter()
                        .any(|item| cells.contains(&item.position.map(|v| v.floor() as i32))),
                    "请先拾取设备占用位置的物品。"
                );
                let boxes = collision_boxes(&equipment);
                let collides = |feet: [f32; 3]| {
                    boxes.iter().any(|b| {
                        let low = [feet[0] - RADIUS, feet[1], feet[2] - RADIUS];
                        let high = [feet[0] + RADIUS, feet[1] + HEIGHT, feet[2] + RADIUS];
                        (0..3).all(|i| low[i] < b.max[i] && high[i] > b.min[i])
                    })
                };
                anyhow::ensure!(
                    !self.players.values().any(|p| collides(p.body.position))
                        && !self
                            .npcs
                            .query::<&Npc>()
                            .iter()
                            .any(|n| collides(n.body.position)),
                    "不能在角色身体内建造设备。"
                );
                item.quantity -= 1;
                if item.quantity == 0 {
                    bag.slots[slot as usize] = None;
                }
                (Some(equipment), None)
            }
            EquipmentCommand::Remove {
                id,
                revision,
                yaw,
                pitch,
                inventory_revision,
            } => {
                anyhow::ensure!(inventory_revision == bag.revision, "物品栏已变化，请重试。");
                let hit = target(&self.terrain, p.body.position, yaw, pitch)
                    .context("设备太远或被遮挡。")?;
                anyhow::ensure!(hit.equipment == Some(id), "设备目标已变化或被遮挡。");
                let equipment = self.terrain.equipment.get(&id).context("设备已被拆除。")?;
                anyhow::ensure!(equipment.revision == revision, "设备已变化，请重试。");
                anyhow::ensure!(equipment.is_empty(), "设备中仍有物料，不能拆除。");
                anyhow::ensure!(
                    equipment.kind != EquipmentKind::PipeRack
                        || !self.terrain.equipment.values().any(|above| {
                            above.origin == equipment.origin && above.kind == EquipmentKind::Pipe
                        }),
                    "请先拆除管架承托的管道。"
                );
                anyhow::ensure!(
                    insert(
                        &mut bag.slots,
                        &InventoryEntry {
                            name: equipment.kind.item_name().into(),
                            quantity: 1
                        }
                    ) == 1,
                    "物品栏已满，无法收取设备。"
                );
                (None, Some(id))
            }
            EquipmentCommand::Configure { .. } | EquipmentCommand::ConfigureThermal { .. } => {
                unreachable!()
            }
            EquipmentCommand::BeginPipe { .. }
            | EquipmentCommand::LayPipe { .. }
            | EquipmentCommand::CancelPipe { .. } => unreachable!(),
        };
        bag.revision = inventory::next_revision(bag.revision)?;
        // One authority transaction: a failure above changes neither inventory nor world facts.
        if let Some(equipment) = add {
            self.terrain.equipment.insert(equipment.id, equipment);
        }
        if let Some(id) = remove {
            self.terrain.equipment.remove(&id);
        }
        self.industry_revision = revision;
        self.next_equipment_id = next_id;
        let p = self.players.get_mut(&player).unwrap();
        p.inventory = bag;
        p.edit_after = self.tick + 6;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn configure_equipment(
        &mut self,
        player: u32,
        id: u64,
        expected: u64,
        yaw: f32,
        pitch: f32,
        enabled: bool,
        opening: f64,
        pump_reversed: bool,
    ) -> Result<()> {
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        anyhow::ensure!(
            p.hp > 0 && p.open_container.is_none() && self.tick >= p.edit_after,
            "当前无法操作设备。"
        );
        anyhow::ensure!(
            target(&self.terrain, p.body.position, yaw, pitch)
                .is_some_and(|hit| hit.equipment == Some(id)),
            "设备太远或被遮挡。"
        );
        let old = self.terrain.equipment.get(&id).context("设备不存在。")?;
        anyhow::ensure!(old.revision == expected, "设备已变化，请重试。");
        anyhow::ensure!(
            matches!(old.kind, EquipmentKind::Pump | EquipmentKind::Valve),
            "此设备没有可调控制。"
        );
        let mut changed = old.clone();
        changed.enabled = enabled;
        changed.opening = opening;
        changed.pump_reversed = pump_reversed;
        changed.reading = None;
        anyhow::ensure!(changed.valid(), "无效设备设置。");
        let revision = inventory::next_revision(self.industry_revision)?;
        changed.revision = revision;
        self.terrain.equipment.insert(id, changed);
        self.industry_revision = revision;
        self.players.get_mut(&player).unwrap().edit_after = self.tick + 6;
        Ok(())
    }

    pub fn industry_ledger(&self) -> &crate::industry_data::ExchangeLedger {
        &self.industry_ledger
    }

    #[allow(clippy::too_many_arguments)]
    fn configure_thermal(
        &mut self,
        player: u32,
        id: u64,
        expected: u64,
        yaw: f32,
        pitch: f32,
        settings: protocol::ThermalSettings,
    ) -> Result<()> {
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        anyhow::ensure!(
            p.hp > 0 && p.open_container.is_none() && self.tick >= p.edit_after,
            "当前无法操作设备。"
        );
        anyhow::ensure!(
            target(&self.terrain, p.body.position, yaw, pitch)
                .is_some_and(|hit| hit.equipment == Some(id)),
            "设备太远或被遮挡。"
        );
        let old = self.terrain.equipment.get(&id).context("设备不存在。")?;
        anyhow::ensure!(old.revision == expected, "设备已变化，请重试。");
        anyhow::ensure!(
            old.kind == EquipmentKind::HeatExchanger && settings.valid(),
            "无效换热设置。"
        );
        let revision = inventory::next_revision(self.industry_revision)?;
        let mut changed = old.clone();
        changed.thermal = Some(settings);
        changed.reading = None;
        changed.revision = revision;
        self.terrain.equipment.insert(id, changed);
        self.industry_revision = revision;
        self.players.get_mut(&player).unwrap().edit_after = self.tick + 6;
        Ok(())
    }

    pub(super) fn exchange_thermal(
        &mut self,
        player: u32,
        id: u64,
        charge: Option<([f64; 2], f64)>,
        heat: Option<f64>,
    ) -> Result<()> {
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法操作设备。");
        anyhow::ensure!(
            target(&self.terrain, p.body.position, p.yaw, p.pitch)
                .is_some_and(|hit| hit.equipment == Some(id)),
            "请在近处瞄准密闭容器。"
        );
        let mut changed = self
            .terrain
            .equipment
            .get(&id)
            .context("设备不存在。")?
            .clone();
        anyhow::ensure!(
            changed.kind == EquipmentKind::Vessel,
            "此指令需要密闭容器。"
        );
        let mut ledger = self.industry_ledger.clone();
        if let Some((amounts, t)) = charge {
            let energy =
                crate::process::charge(&mut changed, amounts, t).map_err(anyhow::Error::msg)?;
            for (total, n) in ledger.hydro_injected_moles.iter_mut().zip(amounts) {
                *total += n;
            }
            ledger.hydro_injected_energy_j += energy;
        } else if let Some(j) = heat {
            crate::process::heat(&mut changed, j).map_err(anyhow::Error::msg)?;
            if j >= 0.0 {
                ledger.heat_added_j += j;
            } else {
                ledger.heat_removed_j -= j;
            }
        } else {
            let amounts = crate::thermo::amounts_from_materials(&changed.contents)
                .map_err(anyhow::Error::msg)?;
            for (total, n) in ledger.hydro_drained_moles.iter_mut().zip(amounts) {
                *total += n;
            }
            ledger.hydro_drained_energy_j += crate::process::stored_energy_j(&changed);
            changed.contents.clear();
            changed.internal_energy_j = 0.0;
        }
        anyhow::ensure!(
            ledger.valid() && changed.valid(),
            "物料或能量交换超出范围。"
        );
        let revision = inventory::next_revision(self.industry_revision)?;
        changed.revision = revision;
        changed.reading = None;
        self.terrain.equipment.insert(id, changed);
        self.industry_revision = revision;
        self.industry_ledger = ledger;
        Ok(())
    }

    pub(super) fn exchange_water(
        &mut self,
        player: u32,
        id: u64,
        fill: Option<(f64, f64)>,
    ) -> Result<()> {
        if fill.is_none()
            && self
                .terrain
                .equipment
                .get(&id)
                .is_some_and(|e| e.kind == EquipmentKind::Vessel)
        {
            return self.exchange_thermal(player, id, None, None);
        }
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法操作设备。");
        anyhow::ensure!(
            target(&self.terrain, p.body.position, p.yaw, p.pitch)
                .is_some_and(|hit| hit.equipment == Some(id)),
            "请在近处瞄准要注排的储罐。"
        );
        let mut changed = self
            .terrain
            .equipment
            .get(&id)
            .context("设备不存在。")?
            .clone();
        let mut ledger = self.industry_ledger.clone();
        if let Some((liters, temperature)) = fill {
            let (moles, energy) = crate::process::fill_water(&mut changed, liters, temperature)
                .map_err(anyhow::Error::msg)?;
            ledger.injected_moles += moles;
            ledger.injected_energy_j += energy;
        } else {
            let (moles, energy) =
                crate::process::drain_water(&mut changed).map_err(anyhow::Error::msg)?;
            ledger.drained_moles += moles;
            ledger.drained_energy_j += energy;
        }
        anyhow::ensure!(ledger.valid() && changed.valid(), "注排结果超出有效范围。");
        let revision = inventory::next_revision(self.industry_revision)?;
        changed.revision = revision;
        changed.reading = None;
        self.terrain.equipment.insert(id, changed);
        self.industry_revision = revision;
        self.industry_ledger = ledger;
        Ok(())
    }

    /// No task owns a mutable world reference. The caller runs this immutable batch off-thread.
    pub fn industry_input(&self) -> Option<(u64, Vec<EquipmentSnapshot>)> {
        (!self.terrain.equipment.is_empty()).then(|| {
            (
                self.industry_revision,
                self.terrain.equipment.values().cloned().collect(),
            )
        })
    }

    pub fn apply_industry_result(
        &mut self,
        expected: u64,
        result: Result<crate::process::FlowStep, String>,
    ) -> bool {
        if expected != self.industry_revision {
            return false;
        }
        let next = match inventory::next_revision(self.industry_revision) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let outcome = result.and_then(|step| {
            if step.devices.len() != self.terrain.equipment.len()
                || !step.pump_work_j.is_finite()
                || step.pump_work_j < 0.0
            {
                return Err("无效求解结果，物料已保留。".into());
            }
            let mut seen = BTreeSet::new();
            for device in &step.devices {
                let old = self
                    .terrain
                    .equipment
                    .get(&device.id)
                    .ok_or("求解设备身份已变化。")?;
                if !seen.insert(device.id)
                    || !device.valid()
                    || device.revision != old.revision
                    || device.kind != old.kind
                    || device.origin != old.origin
                    || device.axis != old.axis
                    || device.pipe_ends != old.pipe_ends
                    || device.enabled != old.enabled
                    || device.opening != old.opening
                    || device.pump_reversed != old.pump_reversed
                    || device.thermal != old.thermal
                {
                    return Err("求解结果改变设备身份或设置。".into());
                }
            }
            let totals = |devices: &[EquipmentSnapshot]| {
                let mut moles = BTreeMap::<String, f64>::new();
                for e in devices {
                    for c in &e.contents {
                        *moles.entry(c.material.clone()).or_default() += c.moles;
                    }
                }
                (
                    moles,
                    devices
                        .iter()
                        .map(crate::process::stored_energy_j)
                        .sum::<f64>(),
                )
            };
            let (before, energy_before) =
                totals(&self.terrain.equipment.values().cloned().collect::<Vec<_>>());
            let (after, energy_after) = totals(&step.devices);
            let close = |a: f64, b: f64| {
                a.is_finite()
                    && b.is_finite()
                    && (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
            };
            if before.keys().chain(after.keys()).any(|key| {
                !close(
                    *before.get(key).unwrap_or(&0.0),
                    *after.get(key).unwrap_or(&0.0),
                )
            }) || !close(energy_after, energy_before + step.pump_work_j)
            {
                return Err("求解守恒校验失败，物料已保留。".into());
            }
            let mut ledger = self.industry_ledger.clone();
            ledger.pump_work_j += step.pump_work_j;
            if !ledger.valid() {
                return Err("工业账目超出范围。".into());
            }
            Ok((step, ledger))
        });
        match outcome {
            Ok((mut step, ledger)) => {
                for d in &mut step.devices {
                    if let Some(t) = d.reading.as_mut().and_then(|r| r.thermal.as_mut()) {
                        // Identical observations retain their timestamp, avoiding idle broadcasts.
                        if let Some(old) = self
                            .terrain
                            .equipment
                            .get(&d.id)
                            .and_then(|e| e.reading.as_ref())
                            .and_then(|r| r.thermal.as_ref())
                        {
                            t.sampled_at_tick = old.sampled_at_tick;
                        }
                        if self
                            .terrain
                            .equipment
                            .get(&d.id)
                            .and_then(|e| e.reading.as_ref())
                            .and_then(|r| r.thermal.as_ref())
                            != Some(t)
                        {
                            t.sampled_at_tick = self.tick;
                        }
                    }
                }
                if step.pump_work_j == 0.0
                    && step
                        .devices
                        .iter()
                        .all(|e| self.terrain.equipment.get(&e.id) == Some(e))
                {
                    return true;
                }
                self.terrain.equipment = step.devices.into_iter().map(|e| (e.id, e)).collect();
                self.industry_ledger = ledger;
            }
            Err(error) => {
                let reading = EquipmentReading {
                    temperature_k: None,
                    pressure_pa: None,
                    volume_m3: 0.0,
                    capacity_m3: 0.0,
                    flow_m3_s: 0.0,
                    status: bounded_status(&error),
                    thermal: None,
                };
                if self
                    .terrain
                    .equipment
                    .values()
                    .all(|e| e.reading.as_ref() == Some(&reading))
                {
                    return true;
                }
                for e in self.terrain.equipment.values_mut() {
                    e.reading = Some(reading.clone());
                }
            }
        }
        self.industry_revision = next;
        true
    }

    pub(super) fn equipment_occupies(&self, cell: [i32; 3]) -> bool {
        self.terrain
            .equipment
            .values()
            .any(|e| occupied_cells(e).contains(&cell))
    }
    pub(super) fn equipment_conflicts(&self, proposed: &EquipmentSnapshot) -> bool {
        let cells = occupied_cells(proposed);
        self.terrain.equipment.values().any(|old| {
            !game_simulation::equipment::rack_compatible(old, proposed)
                && occupied_cells(old).iter().any(|cell| cells.contains(cell))
        })
    }

    pub(super) fn broadcast_equipment(&mut self) {
        self.expire_pipe_starts();
        let mut slow = Vec::new();
        for (&id, p) in &mut self.players {
            if p.industry_revision == Some(self.industry_revision)
                && p.industry_columns == p.sent_columns
            {
                continue;
            }
            let devices = self
                .terrain
                .equipment
                .values()
                .filter(|equipment| {
                    occupied_cells(equipment).iter().any(|cell| {
                        p.sent_columns
                            .contains(&[cell[0].div_euclid(16), cell[2].div_euclid(16)])
                    })
                })
                .cloned()
                .collect();
            if p.tx
                .try_send(ServerMessage::EquipmentState {
                    revision: self.industry_revision,
                    devices,
                })
                .is_err()
            {
                slow.push(id);
            } else {
                p.industry_revision = Some(self.industry_revision);
                p.industry_columns.clone_from(&p.sent_columns);
            }
        }
        for id in slow {
            self.unregister_player(id);
        }
    }

    pub(super) fn write_industry_checkpoint(&self, path: &Path, generation: &str) -> Result<()> {
        let archive = crate::industry_data::IndustryArchive::capture(
            &self.terrain,
            self.world_seed,
            self.tick,
            generation.into(),
            self.industry_revision,
            self.next_equipment_id,
            self.industry_ledger.clone(),
        )?;
        let folder = path.with_extension("industry");
        std::fs::create_dir_all(&folder)?;
        crate::industry_data::write_new(&folder.join(format!("{generation}.rkyv")), &archive)
    }

    pub(super) fn load_industry_checkpoint(
        &mut self,
        path: &Path,
        generation: &str,
        version: u32,
    ) -> Result<()> {
        let archive_path = path
            .with_extension("industry")
            .join(format!("{generation}.rkyv"));
        let archive = if version == 8 {
            crate::industry_data::read_legacy(&archive_path)?
        } else if version == 9 {
            crate::industry_data::read_v2(&archive_path)?
        } else if version == 10 {
            crate::industry_data::read_v3(&archive_path)?
        } else {
            crate::industry_data::read(&archive_path)?
        };
        anyhow::ensure!(
            archive.generation == generation
                && archive.seed == self.world_seed
                && archive.last_update == self.tick,
            "industry checkpoint identity mismatch"
        );
        let equipment = archive.restore(&self.terrain)?;
        self.terrain.equipment = equipment;
        self.industry_revision = archive.revision;
        self.next_equipment_id = archive.next_id;
        self.industry_ledger = archive.ledger;
        Ok(())
    }
}

fn bounded_status(error: &str) -> String {
    let mut status = String::from("暂停：");
    for c in error.chars() {
        if status.len() + c.len_utf8() > 96 {
            break;
        }
        status.push(c);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (VoxelWorld, mpsc::Receiver<ServerMessage>) {
        let mut grid = Grid::unknown();
        for x in -1..=1 {
            for z in -1..=1 {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    grid.apply_chunk([x * 16, y, z * 16], &vec![AIR; 4096]);
                }
                for dx in 0..16 {
                    for dz in 0..16 {
                        grid.set([x * 16 + dx, 80, z * 16 + dz], STONE);
                    }
                }
            }
        }
        let mut world = VoxelWorld::with_map(7, grid);
        world.npcs.clear();
        world.items.clear();
        let (tx, rx) = mpsc::channel(256);
        world
            .register_profile(1, [1; 16], "builder".into(), tx)
            .unwrap();
        world.players.get_mut(&1).unwrap().body.position = [-0.5, 81.0, 18.5];
        world.industry_kit(1).unwrap();
        (world, rx)
    }

    fn aim(w: &VoxelWorld, point: [f32; 3]) -> (f32, f32) {
        let p = w.players[&1].body.position;
        let d = [
            point[0] - p[0],
            point[1] - p[1] - game_simulation::EYE_HEIGHT,
            point[2] - p[2],
        ];
        (
            d[0].atan2(-d[2]),
            d[1].atan2((d[0] * d[0] + d[2] * d[2]).sqrt()),
        )
    }
    fn place_tank(w: &mut VoxelWorld) {
        let (yaw, pitch) = aim(w, [-0.5, 81.0, 15.5]);
        w.apply_equipment_command(
            1,
            EquipmentCommand::Place {
                target: [-1, 80, 15],
                yaw,
                pitch,
                kind: EquipmentKind::Tank,
                axis: 0,
                inventory_revision: w.players[&1].inventory.revision,
                slot: 0,
            },
        )
        .unwrap();
    }
    fn remove(w: &VoxelWorld, id: u64) -> EquipmentCommand {
        let e = &w.terrain.equipment[&id];
        let point = [
            e.origin[0] as f32 + 1.0,
            e.origin[1] as f32 + 1.4,
            e.origin[2] as f32 + 1.0,
        ];
        let (yaw, pitch) = aim(w, point);
        EquipmentCommand::Remove {
            id,
            revision: e.revision,
            yaw,
            pitch,
            inventory_revision: w.players[&1].inventory.revision,
        }
    }
    fn directory() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/industry-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    #[test]
    fn old_nonwater_inventory_survives_migration_pause_and_save() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let tank = w.terrain.equipment.get_mut(&1).unwrap();
        tank.contents.push(MaterialAmount {
            material: "legacy:oil".into(),
            moles: 42.0,
        });
        tank.internal_energy_j = 12345.0;
        let original = tank.clone();
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let mut manifest: Save =
            rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        manifest.version = 8;
        let ip = path.with_extension("industry").join(format!(
            "{}.rkyv",
            manifest.industry_archive.as_ref().unwrap()
        ));
        let archive = crate::industry_data::read(&ip).unwrap();
        std::fs::remove_file(&ip).unwrap();
        crate::industry_data::write_legacy_fixture(&ip, &archive).unwrap();
        let mut old = b"DNG3D001".to_vec();
        old.extend(rmp_serde::to_vec(&manifest).unwrap());
        std::fs::write(&path, &old).unwrap();
        let mut restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.equipment[&1], original);
        assert_eq!(std::fs::read(path.with_extension("v8.bak")).unwrap(), old);
        let backup = dir
            .path()
            .join("world3d.v8.bak.industry")
            .join(ip.file_name().unwrap());
        assert_eq!(std::fs::read(backup).unwrap(), std::fs::read(&ip).unwrap());
        let (revision, input) = restored.industry_input().unwrap();
        restored.apply_industry_result(revision, crate::process::solve(&input, 0.2));
        let tank = &restored.terrain.equipment[&1];
        assert_eq!(tank.contents, original.contents);
        assert_eq!(tank.internal_energy_j, original.internal_energy_j);
        assert!(tank.reading.as_ref().unwrap().status.contains("暂停"));
        restored.save(&path).unwrap();
        assert_eq!(
            VoxelWorld::load(&path).unwrap().terrain.equipment[&1],
            original
        );
    }

    #[test]
    fn version_nine_migrates_exact_water_facts_and_ledger_with_complete_backup() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        crate::process::fill_water(w.terrain.equipment.get_mut(&1).unwrap(), 2345.0, 303.15)
            .unwrap();
        w.industry_ledger.injected_moles = 456.0;
        w.industry_ledger.pump_work_j = 123.0;
        let expected = w.terrain.equipment.clone();
        let ledger = w.industry_ledger.clone();
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let mut manifest: Save =
            rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        manifest.version = 9;
        let ip = path.with_extension("industry").join(format!(
            "{}.rkyv",
            manifest.industry_archive.as_ref().unwrap()
        ));
        let archive = crate::industry_data::read(&ip).unwrap();
        std::fs::remove_file(&ip).unwrap();
        crate::industry_data::write_v2_fixture(&ip, &archive).unwrap();
        let old_archive = std::fs::read(&ip).unwrap();
        let mut old = b"DNG3D001".to_vec();
        old.extend(rmp_serde::to_vec(&manifest).unwrap());
        std::fs::write(&path, &old).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.equipment, expected);
        assert_eq!(restored.industry_ledger, ledger);
        assert_eq!(std::fs::read(path.with_extension("v9.bak")).unwrap(), old);
        assert_eq!(
            std::fs::read(
                dir.path()
                    .join("world3d.v9.bak.industry")
                    .join(ip.file_name().unwrap())
            )
            .unwrap(),
            old_archive
        );
        restored.save(&path).unwrap();
        let current: Save = rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        assert_eq!(current.version, 11);
        assert_eq!(VoxelWorld::load(&path).unwrap().terrain.equipment, expected);
    }

    #[test]
    fn sealed_exchange_rejects_stale_worker_and_persists_signed_energy_ledger() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        w.terrain.equipment.get_mut(&1).unwrap().kind = EquipmentKind::Vessel;
        let (yaw, pitch) = aim(&w, [0.0, 82.4, 16.0]);
        let p = w.players.get_mut(&1).unwrap();
        p.yaw = yaw;
        p.pitch = pitch;
        w.exchange_thermal(1, 1, Some(([1000.0, 1000.0], 320.0)), None)
            .unwrap();
        assert_eq!(w.industry_ledger.hydro_injected_moles, [1000.0, 1000.0]);
        let (revision, input) = w.industry_input().unwrap();
        let before = w.terrain.equipment[&1].clone();
        let ledger = w.industry_ledger.clone();
        assert!(w
            .exchange_thermal(1, 1, Some(([1.0, 1.0], 320.0)), None)
            .is_err());
        assert!(w.exchange_thermal(1, 1, None, Some(1e15)).is_err());
        assert_eq!(w.terrain.equipment[&1], before);
        assert_eq!(w.industry_ledger, ledger);
        w.exchange_thermal(1, 1, None, Some(1000.0)).unwrap();
        assert!(!w.apply_industry_result(revision, crate::process::solve(&input, 0.2)));
        assert_eq!(w.industry_ledger.heat_added_j, 1000.0);
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.industry_ledger, w.industry_ledger);
        assert_eq!(restored.terrain.equipment, w.terrain.equipment);
        w.exchange_water(1, 1, None).unwrap();
        assert!(w.terrain.equipment[&1].is_empty());
        let ledger = w.industry_ledger();
        assert_eq!(ledger.hydro_injected_moles, ledger.hydro_drained_moles);
        assert!(
            (ledger.hydro_injected_energy_j + ledger.heat_added_j - ledger.hydro_drained_energy_j)
                .abs()
                < 1e-7
        );
    }

    #[test]
    fn version_ten_migration_preserves_thermal_facts_and_all_archive_backups() {
        let (mut w, _) = world();
        place_tank(&mut w);
        crate::process::fill_water(w.terrain.equipment.get_mut(&1).unwrap(), 2000.0, 300.0)
            .unwrap();
        let mut vessel = w.terrain.equipment[&1].clone();
        vessel.id = 2;
        vessel.origin = [4, 81, 15];
        vessel.kind = EquipmentKind::Vessel;
        vessel.contents.clear();
        vessel.internal_energy_j = 0.0;
        let energy = crate::process::charge(&mut vessel, [1000.0, 1000.0], 320.0).unwrap();
        w.terrain.equipment.insert(2, vessel);
        w.next_equipment_id = 3;
        w.industry_ledger.hydro_injected_moles = [1000.0, 1000.0];
        w.industry_ledger.hydro_injected_energy_j = energy;
        w.industry_ledger.heat_added_j = 123.0;
        let expected = w.terrain.equipment.clone();
        let ledger = w.industry_ledger.clone();
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let mut saved: Save = rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        saved.version = 10;
        let ip = path
            .with_extension("industry")
            .join(format!("{}.rkyv", saved.industry_archive.as_ref().unwrap()));
        let archive = crate::industry_data::read(&ip).unwrap();
        std::fs::remove_file(&ip).unwrap();
        crate::industry_data::write_v3_fixture(&ip, &archive).unwrap();
        let mut bytes = b"DNG3D001".to_vec();
        bytes.extend(rmp_serde::to_vec(&saved).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.equipment, expected);
        assert_eq!(restored.industry_ledger, ledger);
        assert_eq!(
            std::fs::read(path.with_extension("v10.bak")).unwrap(),
            bytes
        );
        for extension in ["chunks", "players", "entities", "industry"] {
            let original = path.with_extension(extension);
            let backup = dir.path().join(format!("world3d.v10.bak.{extension}"));
            for entry in std::fs::read_dir(original).unwrap() {
                let entry = entry.unwrap();
                assert_eq!(
                    std::fs::read(entry.path()).unwrap(),
                    std::fs::read(backup.join(entry.file_name())).unwrap()
                );
            }
        }
        restored.save(&path).unwrap();
        let final_save: Save = rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        assert_eq!(final_save.version, 11);
        assert_eq!(VoxelWorld::load(&path).unwrap().industry_ledger, ledger);
    }

    #[test]
    fn unsupported_property_revision_refuses_load_without_changing_saved_facts() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let manifest_bytes = std::fs::read(&path).unwrap();
        let manifest: Save = rmp_serde::from_slice(&manifest_bytes[8..]).unwrap();
        let ip = path
            .with_extension("industry")
            .join(format!("{}.rkyv", manifest.industry_archive.unwrap()));
        let mut archive = crate::industry_data::read(&ip).unwrap();
        archive.thermo_revision = "future-incompatible-model".into();
        std::fs::remove_file(&ip).unwrap();
        crate::industry_data::write_new(&ip, &archive).unwrap();
        let archived_bytes = std::fs::read(&ip).unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), manifest_bytes);
        assert_eq!(std::fs::read(&ip).unwrap(), archived_bytes);
    }

    #[test]
    fn water_exchange_ledger_and_stale_batches_are_atomic() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let (yaw, pitch) = aim(&w, [0.0, 82.4, 16.0]);
        let p = w.players.get_mut(&1).unwrap();
        p.yaw = yaw;
        p.pitch = pitch;
        w.exchange_water(1, 1, Some((100.0, 293.15))).unwrap();
        let contents = w.terrain.equipment[&1].contents.clone();
        let ledger = w.industry_ledger.clone();
        let (revision, input) = w.industry_input().unwrap();
        let step = crate::process::solve(&input, 0.2).unwrap();
        w.exchange_water(1, 1, Some((100.0, 293.15))).unwrap();
        assert!(!w.apply_industry_result(revision, Ok(step)));
        assert!((w.terrain.equipment[&1].contents[0].moles - 2.0 * contents[0].moles).abs() < 1e-9);
        let before = w.terrain.equipment[&1].clone();
        let before_ledger = w.industry_ledger.clone();
        for invalid in [(f64::NAN, 293.15), (1e12, 293.15), (10.0, -1.0)] {
            assert!(w.exchange_water(1, 1, Some(invalid)).is_err());
            assert_eq!(w.terrain.equipment[&1], before);
            assert_eq!(w.industry_ledger, before_ledger);
        }
        w.exchange_water(1, 1, None).unwrap();
        assert!(w.terrain.equipment[&1].contents.is_empty());
        assert!((w.industry_ledger.injected_moles - w.industry_ledger.drained_moles).abs() < 1e-9);
        assert!(
            (w.industry_ledger.injected_energy_j - w.industry_ledger.drained_energy_j).abs() < 1e-5
        );
        assert_eq!(
            w.industry_ledger.injected_moles,
            ledger.injected_moles * 2.0
        );
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        assert_eq!(
            VoxelWorld::load(&path).unwrap().industry_ledger,
            w.industry_ledger
        );
        assert!(w.apply_industry_result(w.industry_revision, Err("错误".repeat(100))));
        assert!(w.terrain.equipment[&1].valid());
    }

    #[test]
    fn configure_revision_and_untrusted_results_preserve_facts() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let e = w.terrain.equipment.get_mut(&1).unwrap();
        e.kind = EquipmentKind::Pump;
        let (yaw, pitch) = aim(&w, [-0.5, 81.5, 15.5]);
        w.tick = 6;
        let old = w.terrain.equipment[&1].revision;
        w.configure_equipment(1, 1, old, yaw, pitch, true, 1.0, false)
            .unwrap();
        w.tick = 12;
        assert!(w
            .configure_equipment(1, 1, old, yaw, pitch, false, 1.0, false)
            .is_err());
        let expected = w.terrain.equipment[&1].clone();
        let (revision, input) = w.industry_input().unwrap();
        let step = crate::process::solve(&input, 0.2).unwrap();
        assert!(w.apply_industry_result(revision, Ok(step)));
        assert_eq!(w.terrain.equipment[&1].revision, expected.revision);
        let unchanged = w.industry_revision;
        let (_, input) = w.industry_input().unwrap();
        assert!(w.apply_industry_result(unchanged, crate::process::solve(&input, 0.2)));
        assert_eq!(
            w.industry_revision, unchanged,
            "steady-state facts must not rebroadcast"
        );
        let (revision, mut input) = w.industry_input().unwrap();
        input[0].contents.push(MaterialAmount {
            material: "game:water".into(),
            moles: 1.0,
        });
        w.apply_industry_result(
            revision,
            Ok(crate::process::FlowStep {
                devices: input,
                pump_work_j: 0.0,
            }),
        );
        assert_eq!(w.terrain.equipment[&1].contents, expected.contents);
        assert!(w.terrain.equipment[&1]
            .reading
            .as_ref()
            .unwrap()
            .status
            .contains("暂停"));
    }

    #[test]
    fn cross_chunk_build_remove_and_nonempty_rejection_are_atomic() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let e = &w.terrain.equipment[&1];
        assert_eq!(e.origin, [-1, 81, 15]);
        assert_eq!(
            occupied_cells(e)
                .iter()
                .map(|p| [p[0].div_euclid(16), p[2].div_euclid(16)])
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            1
        );
        w.tick = 6;
        let remove = remove(&w, 1);
        w.terrain
            .equipment
            .get_mut(&1)
            .unwrap()
            .contents
            .push(MaterialAmount {
                material: "game:water".into(),
                moles: 55.5,
            });
        let bag = w.players[&1].inventory.clone();
        assert!(w
            .apply_equipment_command(1, remove.clone())
            .unwrap_err()
            .to_string()
            .contains("物料"));
        assert_eq!(w.players[&1].inventory, bag);
        w.terrain.equipment.get_mut(&1).unwrap().contents.clear();
        w.apply_equipment_command(1, remove.clone()).unwrap();
        let bag = w.players[&1].inventory.clone();
        w.tick = 12;
        assert!(w.apply_equipment_command(1, remove).is_err());
        assert_eq!(w.players[&1].inventory, bag);
        assert!(w.terrain.equipment.is_empty());
        assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 2);
        assert_eq!(w.next_equipment_id, 2);
        place_tank(&mut w);
        assert!(w.terrain.equipment.contains_key(&2));
        assert!(!w.terrain.equipment.contains_key(&1));
    }
    #[test]
    fn worker_cannot_change_explicit_pipe_geometry_during_a_process_commit() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        let d = w.terrain.equipment.get_mut(&1).unwrap();
        d.kind = EquipmentKind::Pipe;
        let ledger = w.industry_ledger.clone();
        let (revision, mut ds) = w.industry_input().unwrap();
        ds[0].pipe_ends = Some([0, 4]);
        assert!(ds[0].valid());
        assert!(w.apply_industry_result(
            revision,
            Ok(crate::process::FlowStep {
                devices: ds,
                pump_work_j: 0.0
            })
        ));
        assert_eq!(w.terrain.equipment[&1].pipe_ends, None);
        assert_eq!(w.industry_ledger, ledger);
        assert!(w.terrain.equipment[&1]
            .reading
            .as_ref()
            .unwrap()
            .status
            .contains("暂停"));
    }

    #[test]
    fn pipes_keep_all_three_axes_and_reject_invalid_build_inputs() {
        for axis in 0..3 {
            let (mut w, _rx) = world();
            w.players.get_mut(&1).unwrap().inventory.selected = 1;
            let bag = w.players[&1].inventory.clone();
            let (yaw, pitch) = aim(&w, [-0.5, 81.0, 15.5]);
            let command = |axis, yaw, revision| EquipmentCommand::Place {
                target: [-1, 80, 15],
                yaw,
                pitch,
                kind: EquipmentKind::Pipe,
                axis,
                inventory_revision: revision,
                slot: 1,
            };
            for invalid in [
                command(3, yaw, bag.revision),
                command(axis, f32::NAN, bag.revision),
                command(axis, yaw, bag.revision + 1),
            ] {
                assert!(w.apply_equipment_command(1, invalid).is_err());
                assert_eq!(w.players[&1].inventory, bag);
                assert!(w.terrain.equipment.is_empty());
            }
            w.apply_equipment_command(1, command(axis, yaw, bag.revision))
                .unwrap();
            assert_eq!(w.terrain.equipment[&1].axis, axis);
            assert_eq!(
                w.players[&1].inventory.slots[1].as_ref().unwrap().quantity,
                15
            );
            let dir = directory();
            let path = dir.path().join("world3d.bin");
            w.save(&path).unwrap();
            assert_eq!(
                VoxelWorld::load(&path).unwrap().terrain.equipment,
                w.terrain.equipment
            );
        }
    }

    #[test]
    fn kit_capacity_and_obstructed_placement_never_partially_commit() {
        let (mut w, _rx) = world();
        let p = w.players.get_mut(&1).unwrap();
        p.inventory.slots.fill(Some(InventoryEntry {
            name: "Full".into(),
            quantity: 64,
        }));
        p.inventory.slots[0] = Some(InventoryEntry {
            name: TANK_ITEM.into(),
            quantity: 62,
        });
        let bag = p.inventory.clone();
        assert!(w.industry_kit(1).is_err());
        assert_eq!(w.players[&1].inventory, bag);
        w.terrain.set([0, 82, 16], STONE);
        let (yaw, pitch) = aim(&w, [-0.5, 81.0, 15.5]);
        assert!(w
            .apply_equipment_command(
                1,
                EquipmentCommand::Place {
                    target: [-1, 80, 15],
                    yaw,
                    pitch,
                    kind: EquipmentKind::Tank,
                    axis: 0,
                    inventory_revision: bag.revision,
                    slot: 0
                }
            )
            .is_err());
        assert_eq!(w.players[&1].inventory, bag);
        assert_eq!(w.next_equipment_id, 1);
        assert!(w.terrain.equipment.is_empty());
    }

    #[test]
    fn stale_revision_teleport_and_block_overlap_are_rejected() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        w.tick = 6;
        let mut stale = remove(&w, 1);
        if let EquipmentCommand::Remove { revision, .. } = &mut stale {
            *revision += 1;
        }
        let bag = w.players[&1].inventory.clone();
        assert!(w
            .apply_equipment_command(1, stale)
            .unwrap_err()
            .to_string()
            .contains("已变化"));
        assert_eq!(w.players[&1].inventory, bag);
        let old_position = w.players[&1].body.position;
        w.chat(1, "/tp 0 81 16".into()).unwrap();
        assert_eq!(w.players[&1].body.position, old_position);
        assert!(w.players[&1]
            .command_log
            .last()
            .unwrap()
            .text
            .contains("占用"));
        let p = w.players.get_mut(&1).unwrap();
        p.inventory.selected = 2;
        p.inventory.slots[2] = Some(InventoryEntry {
            name: "石块".into(),
            quantity: 3,
        });
        let bag = p.inventory.clone();
        let (yaw, pitch) = aim(&w, [-0.99, 81.0, 15.01]);
        w.edit_block(
            1,
            BlockEdit {
                target: [-1, 80, 15],
                expected: STONE,
                yaw,
                pitch,
                place: true,
                slot: 2,
                inventory_revision: bag.revision,
            },
        );
        assert!(!w.players[&1].container_error.is_empty());
        assert_eq!(w.players[&1].inventory, bag);
        assert_eq!(w.terrain.get([-1, 81, 15]), AIR);
    }

    #[test]
    fn visible_cross_chunk_device_is_sent_once_and_unloaded_as_a_whole() {
        let (mut w, mut rx) = world();
        place_tank(&mut w);
        w.players.get_mut(&1).unwrap().sent_columns =
            BTreeSet::from([[-1, 0], [0, 0], [-1, 1], [0, 1]]);
        w.broadcast_equipment();
        assert!(
            matches!(rx.try_recv().unwrap(), ServerMessage::EquipmentState { devices, .. } if devices.len() == 1)
        );
        w.broadcast_equipment();
        assert!(rx.try_recv().is_err());
        w.players.get_mut(&1).unwrap().sent_columns = BTreeSet::from([[0, 1]]);
        w.broadcast_equipment();
        assert!(
            matches!(rx.try_recv().unwrap(), ServerMessage::EquipmentState { devices, .. } if devices.len() == 1)
        );
        w.players.get_mut(&1).unwrap().sent_columns = BTreeSet::from([[1, 1]]);
        w.broadcast_equipment();
        assert!(
            matches!(rx.try_recv().unwrap(), ServerMessage::EquipmentState { devices, .. } if devices.is_empty())
        );
        assert_eq!(w.terrain.equipment.len(), 1);
    }

    #[test]
    fn industry_facts_roundtrip_and_missing_corrupt_archive_preserve_world() {
        let (mut w, _rx) = world();
        place_tank(&mut w);
        w.terrain
            .equipment
            .get_mut(&1)
            .unwrap()
            .contents
            .push(MaterialAmount {
                material: "game:water".into(),
                moles: 100.0,
            });
        w.terrain.equipment.get_mut(&1).unwrap().internal_energy_j = 1234.5;
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.equipment, w.terrain.equipment);
        assert_eq!(restored.next_equipment_id, 2);
        let inventory = restored.saved_players[&[1; 16]].restore().unwrap().1;
        assert_eq!(inventory.slots, w.players[&1].inventory.slots);
        assert_eq!(inventory.selected, w.players[&1].inventory.selected);
        assert_eq!(inventory.revision, 0);
        let bytes = std::fs::read(&path).unwrap();
        let saved: Save = rmp_serde::from_slice(&bytes[8..]).unwrap();
        let ip = path
            .with_extension("industry")
            .join(format!("{}.rkyv", saved.industry_archive.unwrap()));
        let archive = std::fs::read(&ip).unwrap();
        std::fs::remove_file(&ip).unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&ip, &archive[..archive.len() - 1]).unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn v7_world_is_backed_up_and_migrated_without_industrial_backfill() {
        let (w, _rx) = world();
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let mut saved: Save = rmp_serde::from_slice(&std::fs::read(&path).unwrap()[8..]).unwrap();
        saved.version = 7;
        saved.industry_archive = None;
        let mut bytes = b"DNG3D001".to_vec();
        bytes.extend(rmp_serde::to_vec(&saved).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        let migrated = VoxelWorld::load(&path).unwrap();
        assert!(migrated.terrain.equipment.is_empty());
        assert_eq!(migrated.terrain.blocks, w.terrain.blocks);
        assert_eq!(std::fs::read(path.with_extension("v7.bak")).unwrap(), bytes);
        for extension in ["chunks", "players", "entities", "industry"] {
            let original = path.with_extension(extension);
            let backup = dir.path().join(format!("world3d.v7.bak.{extension}"));
            for file in std::fs::read_dir(original).unwrap() {
                let file = file.unwrap();
                assert_eq!(
                    std::fs::read(file.path()).unwrap(),
                    std::fs::read(backup.join(file.file_name())).unwrap()
                );
            }
        }
        migrated.save(&path).unwrap();
        assert_eq!(
            VoxelWorld::load(&path).unwrap().terrain.blocks,
            w.terrain.blocks
        );
    }

    #[test]
    fn invalid_facts_and_interrupted_commit_cannot_replace_previous_world() {
        let (mut w, _rx) = world();
        let dir = directory();
        let path = dir.path().join("world3d.bin");
        w.save(&path).unwrap();
        let old_bytes = std::fs::read(&path).unwrap();
        place_tank(&mut w);
        std::fs::write(path.with_extension("tmp"), b"interrupted transaction").unwrap();
        assert!(w.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), old_bytes);
        let old = VoxelWorld::load(&path).unwrap();
        assert!(old.terrain.equipment.is_empty());
        assert_eq!(
            old.saved_players[&[1; 16]].restore().unwrap().1.slots[0]
                .as_ref()
                .unwrap()
                .quantity,
            2
        );
        std::fs::remove_file(path.with_extension("tmp")).unwrap();
        w.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let saved: Save = rmp_serde::from_slice(&bytes[8..]).unwrap();
        let ip = path
            .with_extension("industry")
            .join(format!("{}.rkyv", saved.industry_archive.unwrap()));
        let valid = crate::industry_data::read(&ip).unwrap();
        for case in 0..4 {
            let mut invalid = valid.clone();
            match case {
                0 => invalid.equipment.push(invalid.equipment[0].clone()),
                1 => invalid.equipment[0].kind = "unknown:device".into(),
                2 => invalid.equipment[0]
                    .contents
                    .push(crate::industry_data::Component {
                        material: "game:water".into(),
                        moles: f64::NAN,
                    }),
                _ => invalid.equipment[0].origin[1] = 80,
            }
            std::fs::remove_file(&ip).unwrap();
            crate::industry_data::write_new(&ip, &invalid).unwrap();
            assert!(VoxelWorld::load(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}

//! Per-session inspection replies. Reports are derived and never saved.
use super::*;

impl VoxelWorld {
    pub(super) fn check_assembly(
        &mut self,
        player: u32,
        request_id: u64,
        id: u64,
        revision: u64,
        yaw: f32,
        pitch: f32,
    ) {
        if request_id == 0 || request_id > i64::MAX as u64 || id == 0 || id > i64::MAX as u64 {
            return;
        }
        self.broadcast_assembly_reports();
        let Some(p) = self.players.get(&player) else {
            return;
        };
        if let Some(last) = &p.assembly_report {
            if request_id == last.request_id && id == last.master_id {
                if p.tx
                    .try_send(ServerMessage::AssemblyReport(last.clone()))
                    .is_err()
                {
                    self.unregister_player(player);
                }
                return;
            }
            if request_id <= last.request_id {
                return;
            }
        }
        let result = (|| -> Result<()> {
            anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法检查设备。");
            anyhow::ensure!(self.tick >= p.assembly_after, "检查过快，请稍后重试。");
            let hit =
                game_simulation::equipment::target(&self.terrain, p.body.position, yaw, pitch)
                    .context("换热器太远或被遮挡。");
            anyhow::ensure!(hit?.equipment == Some(id), "请瞄准要检查的换热器。");
            let master = self.terrain.equipment.get(&id).context("设备已被拆除。");
            let master = master?;
            anyhow::ensure!(
                master.kind == EquipmentKind::HeatExchanger && master.revision == revision,
                "设备类型或版本已变化。"
            );
            Ok(())
        })();
        let report = match result {
            Ok(()) => crate::assembly::inspect(&self.terrain.equipment, id, request_id),
            Err(error) => AssemblyReport {
                request_id,
                master_id: id,
                status: AssemblyStatus::Rejected,
                lines: vec![],
                message: error.to_string(),
            },
        };
        self.assembly_structure = crate::assembly::structure(&self.terrain.equipment);
        let p = self.players.get_mut(&player).unwrap();
        p.assembly_after = self.tick.saturating_add(15);
        p.assembly_report = Some(report.clone());
        if p.tx
            .try_send(ServerMessage::AssemblyReport(report))
            .is_err()
        {
            self.unregister_player(player);
        }
    }

    /// No repeated matching or process solving. A single bounded structural comparison
    /// invalidates inspected reports on construction/direction changes, including new pipes.
    pub(super) fn broadcast_assembly_reports(&mut self) {
        if !self.players.values().any(|p| {
            p.assembly_report.as_ref().is_some_and(|r| {
                matches!(
                    r.status,
                    AssemblyStatus::Complete | AssemblyStatus::Incomplete
                )
            })
        }) {
            return;
        }
        let current = crate::assembly::structure(&self.terrain.equipment);
        if self.assembly_structure == current {
            return;
        }
        self.assembly_structure = current;
        let mut slow = vec![];
        for (&id, p) in &mut self.players {
            let Some(report) = &mut p.assembly_report else {
                continue;
            };
            if !matches!(
                report.status,
                AssemblyStatus::Complete | AssemblyStatus::Incomplete
            ) {
                continue;
            }
            report.status = AssemblyStatus::Stale;
            report.lines.clear();
            report.message = "工业结构或泵方向已变化，请重新检查部件。".into();
            if p.tx
                .try_send(ServerMessage::AssemblyReport(report.clone()))
                .is_err()
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
        let mut w = VoxelWorld::fixture(7);
        w.terrain.equipment = crate::assembly::tests::unit();
        w.npcs.clear();
        w.items.clear();
        let (tx, rx) = mpsc::channel(256);
        w.register_player(1, tx).unwrap();
        // Inspection fixtures are at y=1; replace terrain locally with known air.
        for x in -1..=0 {
            for z in -1..=0 {
                w.terrain.apply_chunk([x * 16, 0, z * 16], &vec![AIR; 4096]);
            }
        }
        w.players.get_mut(&1).unwrap().body.position = [1.0, 1.0, -2.0];
        (w, rx)
    }
    fn check(w: &mut VoxelWorld, req: u64) {
        let delta = [0.0f32, 1.5 - (1.0 + game_simulation::EYE_HEIGHT), 2.5];
        w.check_assembly(1, req, 1, 0, std::f32::consts::PI, delta[1].atan2(delta[2]));
    }
    #[test]
    fn authority_caches_replies_and_invalidates_only_structure_not_running_facts() {
        let (mut w, _rx) = world();
        check(&mut w, 1);
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Complete
        );
        w.terrain.equipment.get_mut(&10).unwrap().internal_energy_j = 42.0;
        w.terrain.equipment.get_mut(&12).unwrap().opening = 0.0;
        w.broadcast_assembly_reports();
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Complete
        );
        w.terrain.equipment.remove(&13);
        w.broadcast_assembly_reports();
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Stale
        );
        check(&mut w, 1); // Retry never reruns a formerly complete inspection.
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Stale
        );
        w.tick += 15;
        check(&mut w, 2);
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Incomplete
        );
        w.players.get_mut(&1).unwrap().body.position = [100.0; 3];
        w.tick += 15;
        check(&mut w, 3);
        assert_eq!(
            w.players[&1].assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Rejected
        );
    }
}

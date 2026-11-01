use super::*;

const HELP: &str =
    "指令：/spawn；/tp x y z；/fly [on|off]；/industry kit；/industry fill ID 升 [温度K]；/industry charge ID 丁烷mol 戊烷mol 温度K；/industry heat ID 焦耳；/industry drain ID；/help";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Spawn,
    Teleport([f32; 3]),
    Fly(Option<bool>),
    IndustryKit,
    IndustryFill(u64, f64, f64),
    IndustryDrain(u64),
    IndustryCharge(u64, [f64; 2], f64),
    IndustryHeat(u64, f64),
}

fn parse(text: &str, origin: [f32; 3]) -> Result<Command> {
    let words: Vec<_> = text.split_whitespace().collect();
    match words.as_slice() {
        ["/help"] => Ok(Command::Help),
        ["/industry", "kit"] => Ok(Command::IndustryKit),
        ["/industry", "fill", id, liters] => parse_fill(id, liters, "293.15"),
        ["/industry", "fill", id, liters, temperature] => parse_fill(id, liters, temperature),
        ["/industry", "charge", id, butane, pentane, temperature] => {
            let id=id.parse::<u64>().context("设备ID必须为正整数。")?;
            let amounts=[butane.parse::<f64>().context("丁烷量必须为摩尔数。")?,pentane.parse::<f64>().context("戊烷量必须为摩尔数。")?];
            let t=temperature.parse::<f64>().context("温度必须为开尔文数值。")?;
            anyhow::ensure!(id>0&&amounts.iter().all(|n|n.is_finite()&&*n>=0.0&&*n<=1e12)&&amounts.iter().sum::<f64>()>0.0&&t.is_finite(),"无效装料参数。");
            Ok(Command::IndustryCharge(id,amounts,t))
        },
        ["/industry", "heat", id, joules] => {
            let id=id.parse::<u64>().context("设备ID必须为正整数。")?;let j=joules.parse::<f64>().context("能量必须为焦耳数值。")?;
            anyhow::ensure!(id>0&&j.is_finite()&&j.abs()<=1e15,"无效加热参数。");Ok(Command::IndustryHeat(id,j))
        },
        ["/industry", "drain", id] => Ok(Command::IndustryDrain(
            id.parse().context("设备ID必须为正整数。")?,
        )),
        ["/industry", ..] => anyhow::bail!(
            "用法：/industry kit；fill ID 升 [温度K]；charge ID 丁烷mol 戊烷mol 温度K；heat ID 焦耳；drain ID（全量排出）"
        ),
        ["/spawn"] => Ok(Command::Spawn),
        ["/spawn", ..] => anyhow::bail!("用法：/spawn"),
        ["/fly"] => Ok(Command::Fly(None)),
        ["/fly", "on"] => Ok(Command::Fly(Some(true))),
        ["/fly", "off"] => Ok(Command::Fly(Some(false))),
        ["/tp", x, y, z] => {
            let mut position = [0.0; 3];
            for (i, word) in [x, y, z].into_iter().enumerate() {
                position[i] = if let Some(offset) = word.strip_prefix('~') {
                    origin[i]
                        + if offset.is_empty() {
                            0.0
                        } else {
                            offset.parse::<f32>().context("相对坐标必须是数字。")?
                        }
                } else {
                    word.parse::<f32>().context("坐标必须是数字。")?
                };
            }
            anyhow::ensure!(valid_position(position), "坐标无效或超出世界边界。");
            Ok(Command::Teleport(position))
        }
        ["/tp", ..] => anyhow::bail!("用法：/tp x y z，例如 /tp 28.5 12 32.5 或 /tp ~ ~5 ~"),
        ["/fly", ..] => anyhow::bail!("用法：/fly [on|off]"),
        _ => anyhow::bail!("未知指令。输入 /help 查看帮助。"),
    }
}

fn parse_fill(id: &str, liters: &str, temperature: &str) -> Result<Command> {
    let id = id.parse::<u64>().context("设备ID必须为正整数。")?;
    let liters = liters.parse::<f64>().context("注入量必须为升数。")?;
    let temperature = temperature
        .parse::<f64>()
        .context("温度必须为开尔文数值。")?;
    anyhow::ensure!(
        id > 0 && liters.is_finite() && liters > 0.0 && temperature.is_finite(),
        "无效注入参数。"
    );
    Ok(Command::IndustryFill(id, liters, temperature))
}

impl VoxelWorld {
    pub(super) fn player_command(&mut self, id: u32, text: &str) {
        let result = self.execute_command(id, text);
        if let Some(p) = self.players.get_mut(&id) {
            p.command_log.push(LogEntry {
                text: result.unwrap_or_else(|e| e.to_string()),
                color: 0xBEC9B2,
                turn: self.tick,
            });
            if p.command_log.len() > 4 {
                p.command_log.remove(0);
            }
        }
    }

    pub(super) fn player_log(&self, p: &Player) -> Vec<LogEntry> {
        let mut log: Vec<_> = self.log.iter().chain(&p.command_log).cloned().collect();
        log.sort_by_key(|entry| entry.turn);
        log.drain(..log.len().saturating_sub(12));
        log
    }

    fn execute_command(&mut self, id: u32, text: &str) -> Result<String> {
        let p = self.players.get(&id).context("玩家尚未连接。")?;
        let command = parse(text, p.body.position)?;
        if command == Command::Help {
            return Ok(HELP.into());
        }
        anyhow::ensure!(p.hp > 0, "请等待复活后再使用指令。");
        anyhow::ensure!(self.tick >= p.command_after, "指令操作过快，请稍后重试。");
        // Rate-limit even rejected destinations before potentially generating terrain.
        self.players.get_mut(&id).unwrap().command_after = self.tick + 15;
        match command {
            Command::Help => unreachable!(),
            Command::IndustryKit => {
                self.industry_kit(id)?;
                Ok("已领取工业测试补给：2 个水罐、4 个密闭容器、1 台换热器、16 段管道、2 台泵、2 个阀门。右键建造，R 旋转，E 检查和控制。".into())
            }
            Command::IndustryFill(equipment, liters, temperature) => {
                self.exchange_water(id, equipment, Some((liters, temperature)))?;
                Ok(format!("已向设备 {equipment} 注入 {liters} L 测试水（{temperature} K），外部输入已记账。"))
            }
            Command::IndustryDrain(equipment) => {
                self.exchange_water(id, equipment, None)?;
                Ok(format!("设备 {equipment} 已排空，外部排出已记账。"))
            }
            Command::IndustryCharge(equipment, amounts, t) => {
                self.exchange_thermal(id, equipment, Some((amounts, t)), None)?;
                Ok(format!(
                    "已向密闭容器 {equipment} 装入测试烃，物料与能量输入已记账。"
                ))
            }
            Command::IndustryHeat(equipment, j) => {
                self.exchange_thermal(id, equipment, None, Some(j))?;
                Ok(format!("设备 {equipment} 能量调整 {j} J，热交换已记账。"))
            }
            Command::Fly(value) => {
                let p = self.players.get_mut(&id).unwrap();
                p.flying = value.unwrap_or(!p.flying);
                p.body.velocity = [0.0; 3];
                p.body.grounded = false;
                Ok(if p.flying {
                    "飞行已开启：Space 上升，Shift 下降，Ctrl 加速；松键悬停。"
                } else {
                    "飞行已关闭。"
                }
                .into())
            }
            Command::Spawn | Command::Teleport(_) => {
                let position = match command {
                    Command::Spawn => self
                        .spawn_position(id)
                        .context("没有可用的出生点，请稍后重试。")?,
                    Command::Teleport(position) => position,
                    _ => unreachable!(),
                };
                use game_simulation::{HEIGHT, RADIUS};
                anyhow::ensure!(
                    position[0] >= -WORLD_LIMIT as f32 + RADIUS
                        && position[0] < WORLD_LIMIT as f32 - RADIUS
                        && position[2] >= -WORLD_LIMIT as f32 + RADIUS
                        && position[2] < WORLD_LIMIT as f32 - RADIUS
                        && position[1] >= 0.0
                        && position[1] + HEIGHT < WORLD_HEIGHT as f32,
                    "角色身体超出世界边界。"
                );
                let center = column_at(position);
                for z in -1..=1 {
                    for x in -1..=1 {
                        let column = [center[0] + x, center[1] + z];
                        if valid_column(column) {
                            terrain::ensure_column(&mut self.terrain, self.world_seed, column)?;
                        }
                    }
                }
                anyhow::ensure!(
                    !game_simulation::overlaps_grid(position, &self.terrain),
                    "传送落点被方块占用。"
                );
                let collides = |other: [f32; 3]| {
                    (other[0] - position[0]).abs() < RADIUS * 2.0
                        && (other[2] - position[2]).abs() < RADIUS * 2.0
                        && (other[1] - position[1]).abs() < HEIGHT
                };
                anyhow::ensure!(
                    !self
                        .players
                        .iter()
                        .any(|(&other_id, p)| other_id != id && collides(p.body.position))
                        && !self
                            .npcs
                            .query::<&Npc>()
                            .iter()
                            .any(|n| collides(n.body.position)),
                    "传送落点被角色占用。"
                );
                let p = self.players.get_mut(&id).unwrap();
                p.body = Body {
                    position,
                    ..Default::default()
                };
                p.open_container = None;
                p.container_error.clear();
                // Keep queued input sequence numbers: seq only acknowledges executed input.
                let destination = if command == Command::Spawn {
                    "已返回出生点"
                } else {
                    "已传送至"
                };
                Ok(format!(
                    "{destination} {:.2} {:.2} {:.2}。",
                    position[0], position[1], position[2]
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (VoxelWorld, mpsc::Receiver<ServerMessage>) {
        let mut w = VoxelWorld::fixture(42);
        w.npcs.clear();
        let (tx, rx) = mpsc::channel(256);
        w.register_player(1, tx).unwrap();
        (w, rx)
    }

    #[test]
    fn teleport_generates_destination_preserves_sequences_and_closes_container() {
        let (mut w, _rx) = world();
        w.input(
            1,
            1,
            PlayerInput {
                strafe: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        w.players.get_mut(&1).unwrap().open_container = Some([26, 7, 30]);
        w.execute_command(1, "/tp -200.5 110 300.5").unwrap();
        assert!(w.terrain.nearby_known([-200.5, 110.0, 300.5]));
        let p = &w.players[&1];
        assert_eq!(p.body.position, [-200.5, 110.0, 300.5]);
        assert_eq!(p.body.velocity, [0.0; 3]);
        assert_eq!(p.last_seq, 0);
        assert_eq!(p.inbox.len(), 1);
        assert!(p.open_container.is_none());
        w.tick();
        assert_eq!(w.players[&1].last_seq, 1);
        assert!(w.players[&1].body.position[0] > -200.5);
    }

    #[test]
    fn invalid_destinations_and_rate_limits_leave_player_unchanged() {
        let (mut w, _rx) = world();
        let original = w.players[&1].body;
        let (tx, _other_rx) = mpsc::channel(64);
        w.register_player(2, tx).unwrap();
        let other = w.players[&2].body.position;
        for text in [
            "/tp 0 0 0".into(),
            "/tp 1 127 1".into(),
            "/tp NaN 5 5".into(),
            format!("/tp {} {} {}", other[0], other[1], other[2]),
        ] {
            w.tick += 15;
            assert!(w.execute_command(1, &text).is_err());
            assert_eq!(w.players[&1].body, original);
        }
        w.tick += 15;
        assert!(w.execute_command(1, "/fly on").is_ok());
        assert!(w.execute_command(1, "/fly off").is_err());
        assert!(w.players[&1].flying);
        assert!(w.execute_command(1, "/help").is_ok());
    }

    #[test]
    fn spawn_returns_to_free_slot_preserving_player_state_and_queued_input() {
        let (mut w, _rx) = world();
        let spawn = w.players[&1].body.position;
        w.execute_command(1, "/tp -200.5 110 300.5").unwrap();
        let (tx, _other_rx) = mpsc::channel(64);
        w.register_player(2, tx).unwrap();
        assert_eq!(w.players[&2].body.position, spawn);
        w.input(1, 1, PlayerInput::default()).unwrap();
        let p = w.players.get_mut(&1).unwrap();
        p.hp = 42;
        p.flying = true;
        p.body.velocity = [1.0, -2.0, 3.0];
        p.open_container = Some([26, 7, 30]);
        p.container_error = "old error".into();
        let inventory = p.inventory.clone();
        let before = p.body;
        assert!(w.execute_command(1, "/spawn").is_err());
        assert_eq!(w.players[&1].body, before);
        w.tick += 15;
        let expected = w.spawn_position(1).unwrap();
        assert_ne!(expected, spawn, "occupied spawn slot must be skipped");
        w.chat(1, "/spawn".into()).unwrap();
        let p = &w.players[&1];
        assert_eq!(p.body.position, expected);
        assert_eq!(p.body.velocity, [0.0; 3]);
        assert_eq!(p.hp, 42);
        assert!(p.flying);
        assert_eq!(p.inventory, inventory);
        assert!(p.open_container.is_none());
        assert!(p.container_error.is_empty());
        assert_eq!(p.last_seq, 0);
        assert_eq!(p.inbox.len(), 1);
        assert!(p.command_log.last().unwrap().text.contains("已返回出生点"));
        assert!(w.player_log(&w.players[&2]).is_empty());
        w.tick();
        assert_eq!(w.players[&1].last_seq, 1);
    }

    #[test]
    fn spawn_rejects_blocked_spawn_area_and_dead_players() {
        let (mut w, _rx) = world();
        w.execute_command(1, "/tp -200.5 110 300.5").unwrap();
        let original = w.players[&1].body;
        w.tick += 15;
        w.players.get_mut(&1).unwrap().hp = 0;
        assert!(w
            .execute_command(1, "/spawn")
            .unwrap_err()
            .to_string()
            .contains("复活"));
        assert_eq!(w.players[&1].body, original);
        w.players.get_mut(&1).unwrap().hp = 100;
        for p in w.spawn_sites.clone() {
            w.terrain.set(
                [p[0].floor() as i32, WORLD_HEIGHT - 1, p[2].floor() as i32],
                STONE,
            );
        }
        assert!(w
            .execute_command(1, "/spawn")
            .unwrap_err()
            .to_string()
            .contains("没有可用的出生点"));
        assert_eq!(w.players[&1].body, original);
        assert!(w
            .execute_command(1, "/spawn")
            .unwrap_err()
            .to_string()
            .contains("过快"));
    }

    #[test]
    fn command_feedback_is_private_and_flight_resets_after_respawn_and_reconnect() {
        let (mut w, _rx) = world();
        let (tx, _other_rx) = mpsc::channel(64);
        w.register_player(2, tx).unwrap();
        w.chat(1, "/unknown".into()).unwrap();
        assert!(w
            .player_log(&w.players[&1])
            .iter()
            .any(|e| e.text.contains("未知指令")));
        assert!(w.player_log(&w.players[&2]).is_empty());
        w.chat(1, "hello".into()).unwrap();
        assert!(w
            .player_log(&w.players[&2])
            .iter()
            .any(|e| e.text.contains("hello")));
        w.chat(1, "/fly".into()).unwrap();
        assert!(w.players[&1].flying);
        w.players.get_mut(&1).unwrap().hp = 0;
        w.tick();
        assert!(!w.players[&1].flying);
        w.tick += 15;
        w.chat(1, "/fly".into()).unwrap();
        w.unregister_player(1);
        let (tx, _reconnected) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        assert!(!w.players[&1].flying);
    }

    #[test]
    fn parses_absolute_relative_and_rejects_invalid_commands() {
        assert_eq!(parse(" /spawn ", [0.0; 3]).unwrap(), Command::Spawn);
        assert!(HELP.contains("/spawn"));
        assert_eq!(
            parse("/tp -20 15 30", [0.0; 3]).unwrap(),
            Command::Teleport([-20.0, 15.0, 30.0])
        );
        assert_eq!(
            parse(" /tp ~ ~2.5 ~-4 ", [2.0, 7.0, 3.0]).unwrap(),
            Command::Teleport([2.0, 9.5, -1.0])
        );
        assert_eq!(parse("/fly", [0.0; 3]).unwrap(), Command::Fly(None));
        assert_eq!(
            parse("/fly off", [0.0; 3]).unwrap(),
            Command::Fly(Some(false))
        );
        for invalid in [
            "/tp NaN 10 0",
            "/tp inf 10 0",
            "/tp 32768 10 0",
            "/tp 0 128 0",
            "/tp 1 2",
            "/tp 1 2 3 4",
            "/fly maybe",
            "/spawn extra",
            "/help extra",
            "/unknown",
        ] {
            assert!(parse(invalid, [0.0; 3]).is_err(), "{invalid}");
        }
    }
}

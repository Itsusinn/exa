extends "res://tests/industry_design.gd"
## Real construction and nine process connections; no material conversion is enabled.

func finish() -> void:
	live_session = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("COLUMN_LOOP_TEST_SAVE=",path)
	print("GODOT_COLUMN_LOOP_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func select_item(name: String) -> bool:
	var found := -1
	for i in range(scene.state.inventory_slots.size()):
		if scene.state.inventory_slots[i].get("name","") == name:
			found = i
			break
	if found < 0:
		check(false,"test kit contains "+name)
		await finish()
		return false
	if found >= 9:
		var quantity: int = scene.state.inventory_slots[found].quantity
		scene.bridge.move_inventory(scene.state.inventory_revision,found,8,quantity)
		if not await expect(func(): return scene.state.inventory_slots[8].get("name","") == name,"move "+name+" from backpack to hotbar"): return false
		found = 8
	scene._select_hotbar(found)
	return await expect(func(): return scene.state.selected_slot == found,"select "+name)

func panel_bounds() -> void:
	check(scene.get_viewport().get_visible_rect().encloses(scene.industry.panel.get_global_rect()),"inspection panel stays inside viewport")
	check(scene.get_viewport().get_visible_rect().encloses(scene.industry.panel_close.get_global_rect()),"inspection close button stays visible")

func inspect_equipment(current: Dictionary) -> bool:
	if current.is_empty():
		check(false,"equipment exists for inspection")
		return false
	if scene.industry.opened: key(KEY_ESCAPE)
	scene._focus_gained()
	aim(Vector3(current.origin)+Vector3(0.5,0.5,0.5))
	if not await expect(func(): return scene.bridge.equipment_target(scene.yaw,scene.pitch).get("id",0) == current.id,"target construction equipment"): return false
	key(KEY_E)
	scene._process(0.0)
	return await expect(func(): return scene.industry.opened and scene.industry.selected_id == current.id,"inspect construction equipment")

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/column-loop-fixture-path.txt")
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-column-loop-%d/world3d.bin" % Time.get_ticks_usec())
	if source.is_empty() or not copy_directory(source.get_base_dir(),path.get_base_dir()):
		quit(1)
		return
	OS.set_environment("EXA_SAVE_PATH",path)
	OS.set_environment("EXA_IDENTITY_PATH",path.get_base_dir().path_join("loop-client.uuid"))
	OS.set_environment("EXA_COLUMN_DESIGN_PATH",path.get_base_dir().path_join("design.json"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	scene.set_process(false)
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"column loop world ready"): return
	live_session = true
	scene._focus_gained()
	if not await teleport(Vector3(1.5,81,13.5)):
		await finish()
		return
	if not await expect(func(): return scene.terrain_pending.is_empty() and not device(8).is_empty(),"loop terrain and devices arrive"): return
	var master := at(Vector3i(0,81,16))
	check(master.kind == 8 and master.ports.size() == 5,"reflux column has five role ports")
	if not await inspect_equipment(master): return
	check(scene.industry.details.text.contains("再沸蒸气返回") and scene.industry.details.text.contains("未运行"),"inspection explains return ports and nonoperating state")
	await screenshot("loop-column-ports.png")
	scene.industry.assembly_button.pressed.emit()
	if not await expect(func(): return scene.bridge.assembly_report().get("status","") == "complete","real authority accepts full process loop"): return
	var report: Dictionary = scene.bridge.assembly_report()
	check(report.lines.size() == 9 and report.lines.all(func(line): return line.valid),"all nine process connections are complete")
	for i in range(3): await process_frame
	panel_bounds()
	await screenshot("loop-connections-top.png")
	scene.industry.scroll_area.scroll_vertical = int(scene.industry.scroll_area.get_v_scroll_bar().max_value)
	for i in range(3): await process_frame
	panel_bounds()
	check(scene.industry.assembly_facts.get_global_rect().end.y <= scene.industry.scroll_area.get_global_rect().end.y+2,"last process report line is visible after scrolling")
	check(scene.industry.assembly_facts.text.contains("塔底产品") and scene.industry.assembly_facts.text.contains("回流返回"),"report includes product and reflux paths")
	await screenshot("loop-connections-bottom.png")
	key(KEY_ESCAPE)
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/fly on")
	if not await expect(func(): return scene.state.flying,"flight enables whole assembly view"): return
	if not await teleport(Vector3(24,91,5)):
		await finish()
		return
	aim(Vector3(8,85,16))
	await screenshot("loop-overview.png")
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/industry kit")
	if not await expect(func(): return scene.state.inventory_slots.any(func(item): return item.get("name","") == "回流蒸馏塔"),"test kit supplies process construction equipment"): return
	# Screenshots/background windows may have released OS focus. This scripted
	# input scenario simulates focus again, as at the start and in inspection.
	scene._focus_gained()
	key(KEY_J)
	scene._process(0.0)
	if not await expect(func(): return scene.distillation.opened,"J opens reusable column design"): return
	scene.distillation._save_design()
	check(scene.distillation.read_saved_design().has("spec"),"existing schema design saved for reflux column")
	key(KEY_ESCAPE)
	if not await select_item("回流蒸馏塔"): return
	key(KEY_B)
	scene._process(0.0)
	scene.industry.build_import.pressed.emit()
	check(scene.bridge.column_build_design().has("spec") and scene.industry.build_note.text.contains("五口"),"B imports existing design into five-port tower construction")
	await screenshot("loop-build-design.png")
	scene.industry.build_apply.pressed.emit()
	var built: Array[Dictionary] = []
	for entry in [["回流蒸馏塔",8,32],["冷凝器",9,38],["再沸器",10,44],["回流三通",11,50]]:
		if not await teleport(Vector3(40.5,81,entry[2]-2.5)):
			await finish()
			return
		if not await select_item(entry[0]): return
		aim(Vector3(40.5,80.99,entry[2]+0.5))
		if not await expect(func(): return scene.industry.placement.get("valid",false),"valid preview for "+entry[0]): return
		if entry[1] == 11:
			for orientation in range(12):
				check(scene.industry.placement.axis == orientation,"three-way preview orientation %d" % orientation)
				check(scene.industry.placement.ports.size() == 3,"three-way preview has three ports")
				if orientation < 11:
					key(KEY_R)
					scene._process(0.0)
			await screenshot("loop-tee-preview.png")
		var preview_mesh: Mesh = scene.industry.preview.mesh
		click(MOUSE_BUTTON_RIGHT)
		var origin := Vector3i(40,81,entry[2])
		if not await expect(func(): return not at(origin).is_empty(),"authority builds "+entry[0]): return
		var current := at(origin)
		check(current.kind == entry[1] and scene.industry.nodes[current.id].mesh == preview_mesh,"placed device preserves preview geometry")
		built.append(current)
		if entry[1] == 11:
			if not await inspect_equipment(current): return
			check(current.axis == 11 and scene.industry.details.text.contains("12 / 12"),"last three-way orientation can be inspected")
			await screenshot("loop-tee-inspection.png")
			key(KEY_ESCAPE)
	# The fifth reboiler nozzle must be selectable by the real pipe tool.
	if not await teleport(Vector3(44.5,81,46.5)):
		await finish()
		return
	if not await select_item("管道"): return
	key(KEY_P)
	aim(Vector3(43,81.5,46.5))
	click(MOUSE_BUTTON_RIGHT)
	if not await expect(func(): return scene.industry.route_state.get("start",{}).get("port",-1) == 4,"pipe tool accepts reboiler fifth port"): return
	check(scene.industry.route_state.start.id == built[2].id,"fifth-port route retains correct device identity")
	await screenshot("loop-fifth-port.png")
	key(KEY_ESCAPE)
	scene._process(0.0)
	key(KEY_E)
	scene._process(0.0)
	check(scene.industry.opened and scene.industry.selected_id == built[2].id,"E opens reboiler through utility nozzle")
	check(scene.industry.details.text.contains("热媒出口") and scene.industry.details.text.contains("公用工程"),"reboiler inspection distinguishes utility nozzles and limited scope")
	await screenshot("loop-reboiler-ports.png")
	key(KEY_ESCAPE)
	if not await teleport(Vector3(46,84,35)):
		await finish()
		return
	aim(Vector3(41.5,82,39.5))
	await screenshot("loop-condenser.png")
	live_session = false
	scene._leave_game(false)
	if not await expect(func(): return scene.pending_action.is_empty(),"loop world saved"): return
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"loop world reopened"): return
	live_session = true
	if not await teleport(Vector3(40.5,81,43.5)):
		await finish()
		return
	if not await expect(func(): return not at(Vector3i(40,81,50)).is_empty(),"new process devices restored"): return
	for original in built:
		var restored := at(original.origin)
		check(not restored.is_empty() and restored.id == original.id and restored.kind == original.kind and restored.axis == original.axis and restored.column_design == original.column_design,"construction identity and design survive restart")
	check(not scene.bridge.assembly_report().has("master_id"),"derived process report is not persisted")
	if not await teleport(Vector3(40.5,81,47.5)):
		await finish()
		return
	scene._focus_gained()
	aim(Vector3(40.5,81.5,50.5))
	click(MOUSE_BUTTON_LEFT)
	if not await expect(func(): return at(Vector3i(40,81,50)).is_empty(),"empty three-way fitting can be dismantled"): return
	await finish()

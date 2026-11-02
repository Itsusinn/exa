extends "res://tests/industry_thermal.gd"
## Real authoritative construction, design drafts, and transient structural reports.

func check(ok: bool, message: String) -> void:
	super.check(ok,message)
	if not ok and is_instance_valid(scene):
		print("DESIGN_DIAGNOSTIC ",message," assembly_report=",scene.bridge.assembly_report(),
			" assembly_panel=",scene.industry.assembly_facts.text,
			" pump=",at(Vector3i(-3,81,16)))

func expect(until: Callable, message: String) -> bool:
	var ok := await settle(until)
	check(ok,message)
	if not ok: await finish()
	return ok

func slot(name: String) -> int:
	for i in range(9):
		if scene.state.inventory_slots[i].get("name","") == name: return i
	return -1

func click(button: MouseButton) -> void:
	var event := InputEventMouseButton.new()
	event.button_index = button
	event.pressed = true
	scene._unhandled_input(event)

func change_pump_direction(pump: Dictionary, reversed: bool) -> void:
	# Change a nearby component while the report is open, as an independent
	# player's edit would. The real authority still validates the supplied ray.
	var point := Vector3(pump.origin)+Vector3(0.5,0.5,0.5)
	var offset: Vector3 = point-scene.state.body_position-Vector3(0,1.58,0)
	var yaw := fposmod(atan2(offset.x,-offset.z),TAU)
	var pitch := atan2(offset.y,Vector2(offset.x,offset.z).length())
	scene.bridge.configure_equipment(pump.id,pump.revision,pump.enabled,pump.opening,reversed,yaw,pitch)

func finish() -> void:
	live_session = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("INDUSTRY_DESIGN_TEST_SAVE=",path)
	print("GODOT_INDUSTRY_DESIGN_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/industry-design-fixture-path.txt")
	if not FileAccess.file_exists(pointer):
		push_error("Run industrial_parameterized_tanks_and_assembly_emit_graphics_fixture first")
		quit(1)
		return
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-industry-design-%d/world3d.bin" % Time.get_ticks_usec())
	if not copy_directory(source.get_base_dir(),path.get_base_dir()):
		push_error("Cannot copy design fixture")
		quit(1)
		return
	OS.set_environment("CDDA_SAVE_PATH",path)
	OS.set_environment("CDDA_IDENTITY_PATH",path.get_base_dir().path_join("design-client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"design singleplayer ready")
		await finish()
		return
	live_session = true
	scene._focus_gained()
	if not await teleport(Vector3(8.5,81,24.5)):
		await finish()
		return
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"design terrain and meshes loaded")
	check(not at(Vector3i(8,81,16)).is_empty(),"saved large tank is visible")
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/industry kit")
	check(await settle(func(): return slot("储罐") >= 0),"test kit provides tank item")
	if failed:
		await finish()
		return
	scene._select_hotbar(slot("储罐"))
	if not await expect(func(): return scene.state.selected_slot == slot("储罐"),"selected tank for sized construction"): return
	aim(Vector3(8.5,80.99,27.5))
	key(KEY_B)
	scene._process(0.0)
	check(scene.industry.build_opened and not scene.captured,"B opens construction dimensions and releases mouse")
	check(not scene.hud.reticle.visible and not scene.hud.hotbar.visible,"design panel hides world HUD")
	var count_before: int = scene.state.equipment_count
	click(MOUSE_BUTTON_RIGHT)
	scene._process(0.0)
	check(scene.state.equipment_count == count_before,"panel input does not construct equipment")
	scene.industry.build_diameter.value = 6
	key(KEY_ESCAPE)
	check(not scene.industry.build_opened and not scene.hud.menu.visible,"Escape cancels draft before pause menu")
	check(scene.bridge.tank_build_spec().diameter == 2,"cancel keeps applied default dimensions")
	key(KEY_B)
	scene.industry.build_diameter.value = 4
	scene.industry.build_height.value = 6
	check(scene.industry.build_capacity.text.contains("65778"),"draft shows shared effective capacity")
	await screenshot("tank-specification.png")
	scene.industry.build_apply.pressed.emit()
	aim(Vector3(8.5,80.99,27.5))
	if not await expect(func(): return scene.industry.placement.get("valid",false),"large tank preview fits known empty footprint"): return
	var preview: Dictionary = scene.industry.placement
	check(preview.diameter == 4 and preview.height == 6 and preview.origin == Vector3i(8,81,27),"preview uses selected dimensions and authoritative anchor rule")
	var preview_mesh: Mesh = scene.industry.preview.mesh
	var quantity: int = scene.state.inventory_slots[slot("储罐")].quantity
	await screenshot("tank-preview.png")
	if failed:
		await finish()
		return
	click(MOUSE_BUTTON_RIGHT)
	if not await expect(func(): return not at(Vector3i(8,81,27)).is_empty(),"right click builds sized tank through server"): return
	var built := at(Vector3i(8,81,27))
	check(built.diameter == 4 and built.height == 6,"built dimensions match preview")
	check(scene.state.inventory_slots[slot("储罐")].quantity == quantity-1,"one sized tank consumes one test item")
	check(scene.industry.nodes[built.id].mesh == preview_mesh,"authoritative instance reuses same-sized preview mesh")
	check(scene.industry.nodes[device(0).id].mesh != preview_mesh,"legacy default tank mesh stays distinct")
	if failed:
		await finish()
		return
	key(KEY_B)
	scene._focus_lost()
	check(not scene.industry.build_opened,"focus loss closes construction draft")
	scene._focus_gained()
	if not await teleport(Vector3(5.5,81,24.5)):
		await finish()
		return
	aim(Vector3(10,84,29))
	await screenshot("large-tank.png")
	if not await teleport(Vector3(-0.5,81,14.5)):
		await finish()
		return
	if not await open_checked(at(Vector3i(0,81,16))):
		await finish()
		return
	scene.industry.assembly_button.pressed.emit()
	if not await expect(func(): return scene.bridge.assembly_report().get("status","") == "complete","real authority checks complete four-tank heat-exchange unit"): return
	check(scene.industry.assembly_facts.text.contains("结构完整"),"report renders structural result")
	if failed:
		await finish()
		return
	await screenshot("assembly-complete.png")
	change_pump_direction(at(Vector3i(-3,81,16)),true)
	if not await expect(func(): return scene.bridge.assembly_report().get("status","") == "stale","component edit invalidates existing structural report"): return
	# The authority limits structural inspection to one request per 15 ticks.
	# Headless rendering skips screenshots, so the edit can arrive before that
	# 0.5-second interval has elapsed. Wait as a user would before rechecking.
	await create_timer(0.6).timeout
	scene.industry.assembly_button.pressed.emit()
	if not await expect(func(): return scene.bridge.assembly_report().get("status","") == "incomplete","recheck reports reversed inlet pump"): return
	await screenshot("assembly-incomplete.png")
	change_pump_direction(at(Vector3i(-3,81,16)),false)
	if not await expect(func(): return not at(Vector3i(-3,81,16)).pump_reversed,"pump direction restored"): return
	key(KEY_ESCAPE)
	check(not scene.bridge.assembly_report().has("master_id"),"closing panel clears transient report")
	live_session = false
	scene._leave_game(false)
	if not await expect(func(): return scene.pending_action.is_empty(),"design world saved and stopped"): return
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"design world reopens"): return
	live_session = true
	if not await teleport(Vector3(8.5,81,24.5)):
		await finish()
		return
	if not await expect(func(): return not at(Vector3i(8,81,27)).is_empty(),"sized tank restored"): return
	check(at(Vector3i(8,81,27)).id == built.id and at(Vector3i(8,81,27)).diameter == 4 and at(Vector3i(8,81,27)).height == 6,"stable identity and construction dimensions survive restart")
	check(not scene.bridge.assembly_report().has("master_id"),"structural report is not restored from save")
	await finish()

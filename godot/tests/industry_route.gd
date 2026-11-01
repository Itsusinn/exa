extends "res://tests/industry_thermal.gd"
## Bounded construction route previews over a real, saved rising water circuit.

func route() -> Dictionary:
	return scene.industry.route_state

func click_route() -> void:
	var event := InputEventMouseButton.new()
	event.button_index = MOUSE_BUTTON_RIGHT
	event.pressed = true
	scene._unhandled_input(event)

func select_pipe() -> bool:
	var found := -1
	for i in range(9):
		if scene.state.inventory_slots[i].get("name","") == "管道": found = i
	if found < 0:
		check(false,"pipe items available for route tool")
		return false
	scene._select_hotbar(found)
	return await settle(func(): return scene.state.selected_slot == found)

func begin_preview() -> bool:
	if not await teleport(Vector3(-23.5,81,16.5)): return false
	scene._focus_gained()
	if not await select_pipe(): return false
	if not route().get("active",false): key(KEY_P)
	aim(Vector3(-21,81.5,16.5))
	click_route()
	var accepted := await settle(func(): return not route().get("start",{}).is_empty() and not route().get("pending",false))
	check(accepted,"first click receives authoritative free start port")
	return accepted

func finish() -> void:
	live_session = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("INDUSTRY_ROUTE_TEST_SAVE=",path)
	print("GODOT_INDUSTRY_ROUTE_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/industry-route-fixture-path.txt")
	if not FileAccess.file_exists(pointer):
		push_error("Run industrial_pipe_routes_are_atomic_and_emit_graphics_fixture first")
		quit(1)
		return
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-industry-route-%d/world3d.bin" % Time.get_ticks_usec())
	if not copy_directory(source.get_base_dir(),path.get_base_dir()):
		push_error("Cannot copy route fixture")
		quit(1)
		return
	OS.set_environment("CDDA_SAVE_PATH",path)
	OS.set_environment("CDDA_IDENTITY_PATH",path.get_base_dir().path_join("route-client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	check(scene.environment.sdfgi_enabled,"pipe route scene uses SDFGI")
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"route singleplayer ready")
		await finish()
		return
	live_session = true
	scene._focus_gained()
	if not await teleport(Vector3(-22,81,13.5)):
		await finish()
		return
	check(await settle(func(): return not device(6).is_empty() and scene.bridge.equipment_state().filter(func(d): return d.kind == 1 and d.pipe_ends.size() == 2).size() > 0),"saved explicit pipe joins and rack arrive")
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"route terrain ready for graphics")
	if failed:
		await finish()
		return
	var initial_count: int = scene.state.equipment_count
	for i in range(2):
		await create_timer(0.6).timeout
		scene.bridge.send_chat("/industry kit")
		await settle(func(): return scene.state.inventory_slots.any(func(item): return item.get("name","") == "管道" and item.get("quantity",0) >= (i+1)*16))
	if not await begin_preview():
		await finish()
		return
	if not await teleport(Vector3(-6.5,83,19.5)):
		await finish()
		return
	aim(Vector3(-8,83.5,16.5))
	check(await settle(func(): return route().get("valid",false) and route().get("length",0) > 0),"A-star previews an obstacle-avoiding route across height and chunks")
	check(route().get("length",100) <= 32 and route().get("elbows",0) > 0,"preview is bounded and contains explicit elbows")
	check(route().get("materials",0) == route().get("length",-1),"preview material count is one pipe per new cell")
	check(scene.industry.route_start.visible and scene.industry.route_target.visible,"free target and retained start are marked")
	if failed:
		await finish()
		return
	var revision: int = route().revision
	var instances: Array = scene.industry.route_nodes.map(func(node): return node.get_instance_id())
	for i in range(12): scene._process(0.0)
	check(route().revision == revision and scene.industry.route_nodes.map(func(node): return node.get_instance_id()) == instances,"unchanged aim reuses cached route and preview instances")
	await screenshot("route-preview.png")
	key(KEY_ESCAPE)
	scene._process(0.0)
	check(not route().get("active",false) and not scene.hud.menu.visible,"Escape cancels route before opening pause menu")
	check(scene.industry.route_nodes.all(func(node): return not node.visible),"cancelled route ghost is hidden")
	check(scene.state.equipment_count == initial_count,"cancelled preview did not place or consume equipment")
	if not await begin_preview():
		await finish()
		return
	scene._focus_lost()
	scene._process(0.0)
	check(not route().get("active",false) and route().get("start",{}).is_empty(),"focus loss clears route draft")
	scene._focus_gained()
	if not await begin_preview():
		await finish()
		return
	if not await teleport(Vector3(-6.5,83,19.5)):
		await finish()
		return
	aim(Vector3(-8,83.5,16.5))
	if not await settle(func(): return route().get("valid",false) and route().get("length",0) > 0):
		check(false,"second route preview is valid before commit")
		await finish()
		return
	var to_place: int = route().length
	click_route()
	check(await settle(func(): return scene.state.equipment_count == initial_count+to_place and route().get("start",{}).is_empty() and not route().get("pending",false)),"second click commits the entire route through the authority")
	initial_count += to_place
	key(KEY_P)
	scene._process(0.0)
	check(not route().get("active",false),"P leaves route mode after a successful lay")
	if not await teleport(Vector3(-18.5,81,13.5)):
		await finish()
		return
	if not await open_checked(device(2)):
		await finish()
		return
	check(scene.industry.details.text.contains("输送中"),"saved pump runs through the rising bent pipe route")
	key(KEY_ESCAPE)
	if not await teleport(Vector3(-18.5,81,12.5)):
		await finish()
		return
	aim(Vector3(-15,82,16.5))
	await screenshot("route-circuit.png")
	var saved_ends: Array = scene.bridge.equipment_state().filter(func(d): return d.kind == 1).map(func(d): return [d.id,d.origin,d.pipe_ends])
	live_session = false
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"route world saves and shuts down")
	check(scene.industry.route_nodes.is_empty(),"preview instances released on disconnect")
	live_session = false
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"route world reopens")
	live_session = true
	if not await teleport(Vector3(-22,81,13.5)):
		await finish()
		return
	check(await settle(func(): return scene.state.equipment_count == initial_count),"saved route identities restored")
	var restored: Array = scene.bridge.equipment_state().filter(func(d): return d.kind == 1).map(func(d): return [d.id,d.origin,d.pipe_ends])
	check(restored == saved_ends and not device(6).is_empty(),"explicit joints and same-cell rack survive restart")
	await finish()

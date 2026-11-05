extends SceneTree
var failed := false
var scene: Node3D

func _initialize() -> void: call_deferred("run")
func check(ok: bool, message: String) -> void:
	if ok: print("PASS: ",message)
	else:
		failed = true
		push_error(message)
func settle(until: Callable) -> bool:
	for i in range(600):
		scene._process(0.0)
		if until.call(): return true
		await create_timer(0.02).timeout
	return false
func aim(point: Vector3) -> void:
	var offset: Vector3 = point-scene.state.body_position-Vector3(0,1.58,0)
	scene.yaw = fposmod(atan2(offset.x,-offset.z),TAU)
	scene.pitch = clampf(atan2(offset.y,Vector2(offset.x,offset.z).length()),-1.45,1.45)
	scene._process(0.0)
func click(button: MouseButton) -> void:
	var event := InputEventMouseButton.new()
	event.button_index = button
	event.pressed = true
	scene._unhandled_input(event)
func key(code: Key) -> void:
	var event := InputEventKey.new()
	event.keycode = code
	event.physical_keycode = code
	event.pressed = true
	scene._input(event)
func slot(name: String) -> int:
	for i in range(9):
		if scene.state.inventory_slots[i].get("name","") == name: return i
	return -1
func screenshot(path: String, name: String) -> void:
	if DisplayServer.get_name() == "headless": return
	for i in range(12): await process_frame
	await RenderingServer.frame_post_draw
	check(root.get_texture().get_image().save_png(path.get_base_dir().path_join(name)) == OK,"graphical screenshot saved: "+name)
func finish() -> void:
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("GODOT_INDUSTRY_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var path := ProjectSettings.globalize_path("res://../target/godot-industry-%d/world3d.bin" % Time.get_ticks_usec())
	OS.set_environment("EXA_SAVE_PATH",path)
	OS.set_environment("EXA_IDENTITY_PATH",path.get_base_dir().path_join("client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	check(scene.environment.sdfgi_enabled,"industrial scene keeps SDFGI enabled without saving user settings")
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"singleplayer ready")
		await finish()
		return
	scene._focus_gained()
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"terrain and mesh queue settled")
	scene.bridge.send_chat("/industry kit")
	check(await settle(func(): return slot("储罐") >= 0 and slot("管道") >= 0),"test kit received through authoritative chat command")
	if slot("储罐") < 0:
		await finish()
		return
	key(KEY_1+slot("储罐"))
	check(await settle(func(): return scene.state.selected_slot == slot("储罐")),"hotbar key selects storage tank")
	var feet: Vector3 = scene.state.body_position
	var placement: Dictionary = {}
	# The same local preview the player sees finds an unobstructed nearby footprint.
	for dz in range(-4,5):
		for dx in range(-4,5):
			aim(Vector3(floorf(feet.x)+dx+0.5,feet.y-0.5,floorf(feet.z)+dz+0.5))
			var candidate: Dictionary = scene.bridge.equipment_preview(scene.yaw,scene.pitch)
			if candidate.get("valid",false):
				placement = candidate
				break
		if not placement.is_empty(): break
	check(not placement.is_empty(),"reachable tank placement preview found")
	if placement.is_empty():
		await finish()
		return
	check(scene.industry.preview.visible,"placement preview appears before construction")
	await screenshot(path,"industry-preview.png")
	var origin: Vector3 = Vector3(placement.origin)
	click(MOUSE_BUTTON_RIGHT)
	check(await settle(func(): return scene.state.equipment_count == 1),"right click constructs one tank")
	var devices: Array = scene.bridge.equipment_state()
	if devices.is_empty():
		await finish()
		return
	var tank_id: int = devices[0].id
	check(scene.industry.nodes.has(tank_id),"tank has a cached render instance")
	var tank_node: Node = scene.industry.nodes[tank_id]
	for i in range(4): scene._process(0.0)
	check(scene.industry.nodes[tank_id] == tank_node,"unchanged equipment reuses its render instance")
	aim(origin+Vector3(1,1.4,1))
	key(KEY_E)
	check(scene.industry.opened and not scene.captured,"E opens equipment information and releases mouse")
	var before: int = scene.state.equipment_count
	click(MOUSE_BUTTON_LEFT)
	await create_timer(0.25).timeout
	scene._process(0.0)
	check(scene.state.equipment_count == before,"equipment panel blocks world demolition")
	await screenshot(path,"industry-panel.png")
	key(KEY_ESCAPE)
	check(not scene.industry.opened and scene.captured,"Esc closes information and restores world controls")
	key(KEY_1+slot("管道"))
	check(await settle(func(): return scene.state.selected_slot == slot("管道")),"hotbar selects pipe")
	key(KEY_R)
	scene._process(0.0)
	check(scene.state.equipment_axis == 1,"R rotates pipe to vertical Y axis")
	# Place on nearby terrain; the pipe's axis differs from the surface normal deliberately.
	var pipe_placement: Dictionary = {}
	for dz in range(-1,3):
		for dx in range(-1,3):
			aim(origin+Vector3(dx+0.5,-0.5,dz+0.5))
			var candidate: Dictionary = scene.bridge.equipment_preview(scene.yaw,scene.pitch)
			if candidate.get("valid",false):
				pipe_placement = candidate
				break
		if not pipe_placement.is_empty(): break
	check(not pipe_placement.is_empty(),"pipe preview finds a valid cell")
	if pipe_placement.is_empty():
		await finish()
		return
	click(MOUSE_BUTTON_RIGHT)
	check(await settle(func(): return scene.state.equipment_count == 2),"right click constructs rotated pipe")
	var pipe_id := 0
	for device in scene.bridge.equipment_state():
		if device.kind == 1:
			pipe_id = device.id
			check(device.axis == 1,"authoritative pipe orientation is vertical")
	scene._focus_lost()
	key(KEY_R)
	scene._process(0.0)
	check(scene.state.equipment_axis == 1,"focus loss prevents pipe rotation")
	scene._focus_gained()
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"equipment world saves cleanly")
	check(scene.industry.nodes.is_empty(),"session exit clears equipment instances")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready and scene.state.equipment_count == 2),"equipment reloads with stable identities")
	scene._focus_gained()
	check(scene.industry.nodes.has(tank_id) and scene.industry.nodes.has(pipe_id),"tank and pipe identity survive restart")
	aim(origin+Vector3(1,1.4,1))
	if DisplayServer.get_name() != "headless":
		await settle(func(): return scene.terrain_pending.is_empty())
		# Hide the ghost for a clear persisted-equipment screenshot.
		scene.industry.preview.hide()
		var original_fov: float = scene.camera.fov
		scene.camera.fov = 100.0
		await screenshot(path,"industry.png")
		scene.camera.fov = original_fov
	click(MOUSE_BUTTON_LEFT)
	check(await settle(func(): return scene.state.equipment_count == 1),"left click dismantles empty tank")
	check(not scene.industry.nodes.has(tank_id),"dismantled tank render instance removed")
	print("INDUSTRY_TEST_SAVE=",path)
	await finish()

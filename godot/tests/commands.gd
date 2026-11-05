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
	for i in range(400):
		scene._process(0.0)
		if until.call(): return true
		await create_timer(0.02).timeout
	return false
func key(code: Key) -> void:
	var event := InputEventKey.new()
	event.keycode = code
	event.pressed = true
	scene._input(event)
func submit(text: String) -> void:
	scene.hud.chat.text = text
	scene.hud.chat.text_submitted.emit(text)
func move(actions: Array, frames: int) -> float:
	var before: float = scene.state.body_position.y
	for action in actions: Input.action_press(action)
	for i in range(frames):
		scene._process(1.0/30.0)
		await create_timer(0.035).timeout
	for action in actions: Input.action_release(action)
	await settle(func(): return scene.state.pending == 0)
	return scene.state.body_position.y - before
func run() -> void:
	var path := ProjectSettings.globalize_path("res://../target/godot-commands-%d/world3d.bin" % Time.get_ticks_usec())
	OS.set_environment("EXA_SAVE_PATH",path)
	OS.set_environment("EXA_IDENTITY_PATH",path.get_base_dir().path_join("client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"singleplayer ready")
		quit(1)
		return
	scene._focus_gained()
	var initial_spawn: Vector3 = scene.state.body_position
	key(KEY_SLASH)
	check(scene.hud.inventory_open and scene.hud.chat.has_focus() and scene.hud.chat.text == "/", "slash opens focused command entry")
	check(not scene.captured, "command entry releases mouse")
	submit("/fly on")
	check(await settle(func(): return scene.state.flying), "chat command enables authoritative flight")
	check(scene.hud.health.text.contains("飞行"), "HUD shows flight state")
	var held: Vector3 = scene.state.body_position
	await move(["jump","sprint"],8)
	check(scene.state.body_position.distance_to(held) < 0.01, "chat focus blocks flight controls")
	scene._close_inventory()
	check(scene.captured, "closing command entry restores mouse capture")
	var normal := await move(["jump"],8)
	var fast := await move(["jump","sprint"],8)
	check(normal > 0.8 and fast > normal * 2.8, "Space ascends and Ctrl accelerates threefold")
	held = scene.state.body_position
	await create_timer(0.6).timeout
	scene._process(0.0)
	check(scene.state.body_position.distance_to(held) < 0.01, "released controls hover")
	check(await move(["descend"],8) < -0.8, "Shift descends")
	Input.action_press("descend")
	Input.action_press("sprint")
	scene._focus_lost()
	check(not Input.is_action_pressed("descend") and not Input.is_action_pressed("sprint"), "focus loss clears descent and acceleration")
	scene._focus_gained()
	key(KEY_SLASH)
	submit("/tp -200.5 110 300.5")
	check(await settle(func(): return scene.state.body_position.distance_to(Vector3(-200.5,110,300.5)) < 0.01), "command teleports to negative coordinates")
	check(await settle(func(): return scene.terrain_nodes.has(Vector3i(-208,0,288)) and scene.state.loaded_sections == 392), "teleport streams destination and unloads old columns")
	check(scene.state.position.distance_to(scene.state.body_position) < 0.01, "camera resets interpolation after teleport")
	submit("/unknown")
	check(await settle(func(): return scene.hud.journal.text.contains("未知指令")), "unknown command reports an error without disconnecting")
	await create_timer(0.6).timeout
	submit("/tp 0 0 0")
	check(await settle(func(): return scene.hud.journal.text.contains("落点被方块占用")), "solid destination rejected")
	check(scene.state.body_position.distance_to(Vector3(-200.5,110,300.5)) < 0.01, "failed teleport preserves position")
	if DisplayServer.get_name() != "headless":
		for i in range(45):
			scene._process(0.0)
			await process_frame
		await RenderingServer.frame_post_draw
		check(root.get_texture().get_image().save_png(path.get_base_dir().path_join("commands.png")) == OK,"graphical command screenshot saved")
	await create_timer(0.6).timeout
	submit("/fly off")
	check(await settle(func(): return not scene.state.flying and scene.state.body_position.y < 110), "flight off restores gravity")
	await create_timer(0.6).timeout
	submit("/fly on")
	check(await settle(func(): return scene.state.flying), "flight can be reenabled")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()), "command world saves on exit")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready), "saved command world reopens")
	check(not scene.state.flying and absf(scene.state.body_position.x + 200.5) < 0.01, "teleport position persists while flight resets")
	key(KEY_SLASH)
	submit("/spawn")
	check(await settle(func(): return scene.hud.journal.text.contains("已返回出生点")), "spawn command returns through chat")
	check(await settle(func(): return scene.state.ready and scene.state.body_position.distance_to(initial_spawn) < 0.1), "spawn restores authoritative position and nearby chunks")
	check(scene.state.position.distance_to(scene.state.body_position) < 0.01, "spawn resets camera interpolation")
	check(await settle(func(): return scene.terrain_nodes.has(Vector3i(16,0,32)) and not scene.terrain_nodes.has(Vector3i(-208,0,288))), "spawn replaces distant terrain meshes")
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("GODOT_COMMANDS_RESULT=", "FAIL" if failed else "PASS")
	print("COMMANDS_TEST_SAVE=",path)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

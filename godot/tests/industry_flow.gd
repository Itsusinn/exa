extends SceneTree
var failed := false
var scene: Node3D
var path := ""

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
func key(code: Key) -> void:
	var event := InputEventKey.new()
	event.keycode = code
	event.physical_keycode = code
	event.pressed = true
	scene._input(event)
func device(kind: int) -> Dictionary:
	for value in scene.bridge.equipment_state():
		if value.kind == kind: return value
	return {}
func pump_flow() -> float:
	return device(2).get("reading",{}).get("flow_m3_s",NAN)
func copy_directory(source: String, destination: String) -> bool:
	if DirAccess.make_dir_recursive_absolute(destination) != OK: return false
	for name in DirAccess.get_files_at(source):
		if DirAccess.copy_absolute(source.path_join(name),destination.path_join(name)) != OK: return false
	for name in DirAccess.get_directories_at(source):
		if not copy_directory(source.path_join(name),destination.path_join(name)): return false
	return true
func screenshot(name: String) -> void:
	if DisplayServer.get_name() == "headless": return
	for i in range(12): await process_frame
	await RenderingServer.frame_post_draw
	check(root.get_texture().get_image().save_png(path.get_base_dir().path_join(name)) == OK,"graphical screenshot saved: "+name)
func open_device(kind: int) -> void:
	if scene.industry.opened: key(KEY_ESCAPE)
	var current := device(kind)
	aim(Vector3(current.origin)+Vector3(0.5,0.5,0.5))
	key(KEY_E)
	check(scene.industry.opened and scene.industry.selected_id == current.id,"E targets equipment kind %d" % kind)
func direction_screenshot(name: String) -> void:
	var pump := device(2)
	aim(Vector3(pump.origin)+Vector3(0.5,0.5,0.5))
	var old_fov: float = scene.camera.fov
	scene.camera.fov = 48
	await screenshot(name)
	scene.camera.fov = old_fov
func finish() -> void:
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("INDUSTRY_FLOW_TEST_SAVE=",path)
	print("GODOT_INDUSTRY_FLOW_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/industry-flow-fixture-path.txt")
	if not FileAccess.file_exists(pointer):
		push_error("Run the industrial_flow_controls_conserve_and_emit_graphics_fixture Rust test first")
		quit(1)
		return
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-industry-flow-%d/world3d.bin" % Time.get_ticks_usec())
	if not copy_directory(source.get_base_dir(),path.get_base_dir()):
		push_error("Could not copy isolated industrial fixture")
		quit(1)
		return
	OS.set_environment("CDDA_SAVE_PATH",path)
	OS.set_environment("CDDA_IDENTITY_PATH",path.get_base_dir().path_join("graphics-client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	check(scene.environment.sdfgi_enabled,"flow scene enables SDFGI without changing user settings")
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"singleplayer ready")
		await finish()
		return
	scene._focus_gained()
	scene.bridge.send_chat("/tp 0.5 81 19")
	check(await settle(func(): return scene.state.body_position.distance_to(Vector3(0.5,81,19)) < 0.2 and scene.state.equipment_count == 6),"loaded the complete two-tank pump valve circuit")
	check(await settle(func(): return not device(2).get("reading",{}).is_empty()),"authoritative runtime readings arrive")
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"all nearby terrain is ready before visual review")
	if device(2).is_empty() or device(3).is_empty():
		await finish()
		return
	var pump_id: int = device(2).id
	var valve_id: int = device(3).id
	var pump_node: Node = scene.industry.nodes[pump_id]
	var pump_mesh: Mesh = pump_node.mesh
	var arrow: MeshInstance3D = scene.industry.arrows[pump_id]
	var arrow_mesh: Mesh = arrow.mesh
	check(arrow.global_transform.basis.x.is_equal_approx(Vector3.RIGHT),"stopped pump arrow points toward configured +X discharge")
	for axis in range(3):
		for reversed in [false,true]:
			scene.industry._set_arrow(scene.industry.preview_arrow,axis,reversed)
			var expected: Vector3 = [Vector3.RIGHT,Vector3.UP,Vector3.BACK][axis] * (-1 if reversed else 1)
			check(scene.industry.preview_arrow.basis.x.is_equal_approx(expected),"direction marker supports axis %d reversed %s" % [axis,reversed])
	check(scene.industry.preview_arrow.mesh == arrow_mesh,"preview and placed pump share marker geometry")
	await direction_screenshot("pump-forward.png")
	open_device(2)
	check(not scene.captured and scene.industry.pump_controls.visible,"pump controls release mouse and isolate world input")
	check(not device(2).enabled,"fixture pump starts isolated")
	scene.industry.pump_switch.pressed.emit()
	check(await settle(func(): return device(2).enabled and scene.industry.pending_revision < 0),"pump starts with authoritative settings echo")
	check(await settle(func(): return absf(pump_flow()) > 0.000001),"pump drives water against the fixture level difference")
	check(scene.industry.details.text.contains("测试动力"),"pump panel declares test power")
	await screenshot("pump-running.png")
	open_device(3)
	check(scene.industry.valve_controls.visible,"valve panel exposes opening controls")
	scene.industry.valve_buttons[0].pressed.emit()
	check(await settle(func(): return device(3).opening == 0.0 and scene.industry.pending_revision < 0),"valve closes through authoritative command")
	check(await settle(func(): return absf(pump_flow()) < 0.0000001),"closed valve stops the running pump circuit")
	await create_timer(0.3).timeout
	scene.industry.valve_buttons[1].pressed.emit()
	check(await settle(func(): return device(3).opening == 0.25 and scene.industry.pending_revision < 0),"valve accepts partial opening")
	check(await settle(func(): return absf(pump_flow()) > 0.000001),"partial opening resumes flow")
	await screenshot("valve-control.png")
	check(scene.industry.nodes[pump_id] == pump_node and pump_node.mesh == pump_mesh,"runtime readings reuse equipment node and mesh")
	open_device(2)
	scene.industry.pump_switch.pressed.emit()
	check(await settle(func(): return not device(2).enabled and scene.industry.pending_revision < 0),"pump shuts down through panel")
	check(await settle(func(): return absf(pump_flow()) < 0.0000001),"stopped pump isolates transport")
	await create_timer(0.3).timeout
	scene.industry.pump_direction.pressed.emit()
	check(await settle(func(): return device(2).pump_reversed and scene.industry.pending_revision < 0),"pump direction reverses while stopped")
	key(KEY_ESCAPE)
	check(scene.captured and not scene.industry.opened,"closing process panel restores world controls")
	check(scene.industry.arrows[pump_id] == arrow and arrow.mesh == arrow_mesh,"reversing pump reuses direction marker and mesh")
	check(arrow.global_transform.basis.x.is_equal_approx(Vector3.LEFT),"authoritative reversal flips the visible arrow toward -X")
	await direction_screenshot("pump-reversed.png")
	check(await settle(func(): return scene.terrain_pending.is_empty()),"terrain mesh queue drains")
	if DisplayServer.get_name() != "headless":
		scene.bridge.send_chat("/tp 0.5 81 21")
		check(await settle(func(): return scene.state.body_position.distance_to(Vector3(0.5,81,21)) < 0.2),"move back for full tank silhouette review")
		var old_fov: float = scene.camera.fov
		scene.camera.fov = 100
		aim(Vector3(0,82,16.5))
		await screenshot("circuit.png")
		scene.camera.fov = old_fov
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"runtime world saves cleanly")
	check(scene.industry.nodes.is_empty() and scene.industry.records.is_empty(),"disconnect clears all industrial state")
	check(scene.industry.arrows.is_empty(),"disconnect clears direction marker instances")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready and scene.state.equipment_count == 6),"industrial world reloads")
	scene._focus_gained()
	check(device(2).id == pump_id and device(3).id == valve_id,"device identities survive restart")
	check(not device(2).enabled and device(2).pump_reversed and device(3).opening == 0.25,"pump and valve control facts survive restart")
	check(scene.industry.arrows[pump_id].global_transform.basis.x.is_equal_approx(Vector3.LEFT),"saved reversed direction restores its arrow")
	check(await settle(func(): return not device(0).get("reading",{}).is_empty()),"readings reconstruct after restart")
	var reading: Dictionary = device(0).reading
	check(reading.volume_m3 > 0 and is_finite(reading.pressure_pa) and reading.temperature_k != null and is_finite(reading.temperature_k),"tank shows finite preserved inventory and temperature")
	await finish()

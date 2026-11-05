extends SceneTree
# Uses the isolated runway save produced by the Rust QUIC exploration test.
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
func run() -> void:
	var path := preload("res://src/app_config.gd").environment("SAVE_PATH")
	if not path.contains("target") or not FileAccess.file_exists(path):
		push_error("Set EXA_SAVE_PATH to an isolated target/streaming-quic fixture")
		quit(1)
		return
	var identity := path.get_basename() + ".streaming.uuid"
	var file := FileAccess.open(identity,FileAccess.WRITE)
	file.store_string("5b5b5b5b-5b5b-5b5b-5b5b-5b5b5b5b5b5b")
	file.close()
	OS.set_environment("EXA_IDENTITY_PATH",identity)
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready and scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()):
		check(false,"streaming fixture ready")
		quit(1)
		return
	check(scene.state.body_position.x < -32,"saved negative position restored")
	check(scene.terrain_nodes.has(Vector3i(-48,0,32)),"negative terrain mesh exists")
	var start_x: float = scene.state.body_position.x
	var old_center := floori(start_x/16.0)
	var obsolete := Vector3i((old_center+3)*16,0,32)
	var incoming := Vector3i((old_center-4)*16,0,32)
	check(scene.terrain_nodes.has(obsolete),"old edge mesh present")
	check(not scene.terrain_nodes.has(incoming),"new edge is initially absent")
	var destination := float(old_center*16)-2.0
	scene.yaw = 0.0
	scene.pitch = -0.1
	scene._focus_gained()
	for i in range(480):
		scene._focus_gained()
		Input.action_press("left")
		scene._process(1.0/30.0)
		if scene.state.loaded_sections > 392: check(false,"bounded client cache")
		if scene.state.body_position.x < destination: break
		await create_timer(1.0/30.0).timeout
	Input.action_release("left")
	check(scene.state.body_position.x < destination,"walk crosses negative chunk boundary")
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_nodes.has(incoming)),"new edge generated and rendered")
	check(not scene.terrain_nodes.has(obsolete),"obsolete mesh node removed")
	if DisplayServer.get_name() != "headless":
		for i in range(40):
			scene._process(0.0)
			await process_frame
		await RenderingServer.frame_post_draw
		var output := path.get_basename() + ".streaming.png"
		root.get_texture().get_image().save_png(output)
		print("Streaming screenshot: ",output)
	scene._leave_game(false)
	check(await settle(func(): return not scene.state.local_running and not scene.state.shutdown_pending),"streamed world saved and stopped")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

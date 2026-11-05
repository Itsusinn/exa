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
func aim(point: Vector3) -> void:
	var offset: Vector3 = point - scene.state.body_position - Vector3(0,1.58,0)
	scene.yaw = atan2(offset.x,-offset.z)
	scene.pitch = atan2(offset.y,Vector2(offset.x,offset.z).length())
func click(button: MouseButton) -> void:
	var event := InputEventMouseButton.new()
	event.button_index = button
	event.pressed = true
	scene._unhandled_input(event)
func run() -> void:
	var path := ProjectSettings.globalize_path("res://../target/godot-edit-%d/world3d.bin" % Time.get_ticks_usec())
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
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"initial streaming and mesh queue settle before mesh reuse check")
	# Mine a neighboring ground cell so placement cannot overlap the player.
	var feet: Vector3 = scene.state.body_position
	var target := Vector3(floorf(feet.x)+1.5, roundf(feet.y)-0.5, floorf(feet.z)+0.5)
	aim(target)
	var original_target: Dictionary = scene.bridge.block_target(scene.yaw,scene.pitch)
	check(not original_target.is_empty(),"crosshair finds neighboring terrain")
	var far_node = scene.terrain_nodes.get(Vector3i(48,0,48))
	var far_mesh = far_node.mesh if is_instance_valid(far_node) else null
	var before: int = scene.state.inventory_revision
	click(MOUSE_BUTTON_LEFT)
	check(await settle(func(): return scene.state.inventory_revision > before),"left click mines a block through authority")
	check(not scene.state.inventory_slots[0].is_empty(),"mined block enters hotbar")
	check(scene.state.inventory_slots[0].get("quantity",0) == 1,"exactly one block collected")
	check(scene.terrain_nodes.get(Vector3i(48,0,48)) == far_node and far_node.mesh == far_mesh,"unrelated chunk mesh is retained")
	await create_timer(0.25).timeout
	# Look vertically into the opened cell, hitting its supporting block.
	aim(target-Vector3(0,1,0))
	before = scene.state.inventory_revision
	click(MOUSE_BUTTON_RIGHT)
	check(await settle(func(): return scene.state.inventory_revision > before),"right click places selected hotbar block")
	check(scene.state.inventory_slots[0].is_empty(),"placement consumes one item")
	aim(target)
	check(scene.bridge.block_target(scene.yaw,scene.pitch).get("position") == original_target.get("position"),"placed terrain returns to the target cell")
	await create_timer(0.25).timeout
	before = scene.state.inventory_revision
	click(MOUSE_BUTTON_LEFT)
	check(await settle(func(): return scene.state.inventory_revision > before),"replacement block can be mined again")
	# UI must consume mouse clicks without editing the world.
	scene.hud.inventory_open = true
	scene._process(0.0)
	before = scene.state.inventory_revision
	click(MOUSE_BUTTON_RIGHT)
	await create_timer(0.25).timeout
	scene._process(0.0)
	check(scene.state.inventory_revision == before,"inventory UI blocks world placement")
	scene._close_inventory()
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"edited world saves")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"edited world reopens")
	aim(target)
	check(scene.bridge.block_target(scene.yaw,scene.pitch).get("position") != original_target.get("position"),"removed block remains absent after restart")
	check(scene.state.inventory_slots[0].get("quantity",0) == 1,"collected block persists after restart")
	if DisplayServer.get_name() != "headless":
		for i in range(45):
			scene._process(0.0)
			await process_frame
		await RenderingServer.frame_post_draw
		check(root.get_texture().get_image().save_png(path.get_base_dir().path_join("editing.png")) == OK,"graphical editing screenshot saved")
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("GODOT_EDITING_RESULT=", "FAIL" if failed else "PASS")
	print("EDITING_TEST_SAVE=",path)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

extends SceneTree
var failed := false
var scene: Node3D
var folder: String

func _initialize() -> void: call_deferred("run")
func check(ok: bool, message: String) -> void:
	if ok: print("PASS: ",message)
	else:
		failed = true
		push_error(message)
func settle(until: Callable) -> bool:
	var deadline := Time.get_ticks_msec()+30000
	while Time.get_ticks_msec()<deadline:
		scene._process(0.0)
		await process_frame
		if until.call(): return true
	return false
func capture(name: String) -> void:
	if DisplayServer.get_name() == "headless": return
	for i in range(90):
		scene._process(0.0)
		await process_frame
	await RenderingServer.frame_post_draw
	check(root.get_texture().get_image().save_png(folder.path_join(name+".png"))==OK,"save "+name+" screenshot")
func run() -> void:
	folder=ProjectSettings.globalize_path("res://../target/natural-spawn-%d" % Time.get_ticks_usec())
	DirAccess.make_dir_recursive_absolute(folder)
	OS.set_environment("EXA_SAVE_PATH",folder.path_join("world3d.bin"))
	OS.set_environment("EXA_IDENTITY_PATH",folder.path_join("client.uuid"))
	scene=load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene=scene
	scene.set_process(false)
	scene.set_process_input(false)
	scene.set_process_unhandled_input(false)
	scene.hud.menu.loading_settings=true
	scene.environment.sdfgi_enabled=true
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready and scene.state.loaded_sections==392 and scene.terrain_pending.is_empty()):
		check(false,"natural world starts and streams")
		quit(1)
		return
	var spawn: Vector3=scene.state.body_position
	check(spawn.y!=7.0,"spawn follows natural surface height")
	scene.hud.hide()
	scene.yaw=2.4
	scene.pitch=-0.2
	await capture("ground")
	var peak:=spawn.y
	for i in range(12):
		scene.bridge.advance(1.0/30.0,{"forward":1.0,"jump":i==0,"pickup":true})
		await create_timer(1.0/30.0).timeout
		scene._process(0.0)
		peak=maxf(peak,scene.state.body_position.y)
	check(await settle(func(): return scene.state.pending==0),"walking inputs acknowledged")
	check(peak>spawn.y+0.1 and scene.state.body_position.distance_to(spawn)>0.2,"walk and jump on generated terrain")
	check(not scene.state.inventory.is_empty(),"supplies placed at the generated spawn")
	scene.bridge.send_chat("/fly on")
	check(await settle(func(): return scene.state.flying),"enable inspection flight")
	await create_timer(0.6).timeout
	var view:=spawn+Vector3(12,25,24)
	scene.bridge.send_chat("/tp %s %s %s" % [view.x,view.y,view.z])
	check(await settle(func(): return scene.state.ready and scene.state.body_position.distance_to(view)<0.1 and scene.terrain_pending.is_empty()),"inspect full generated spawn area")
	scene.yaw=-0.46
	scene.pitch=-0.7
	await capture("overview")
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/spawn")
	check(await settle(func(): return scene.state.ready and scene.state.body_position.distance_to(spawn)<0.1),"spawn command returns to safe generated ground")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"natural world saves")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"natural world reopens")
	check(scene.state.body_position.distance_to(spawn)<0.1,"saved spawn position remains safe")
	check(not scene.state.inventory.is_empty() and scene.state.items.is_empty(),"collected supplies remain collected")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"reopened world closes")
	print("NATURAL_SPAWN_RESULT=", "FAIL" if failed else "PASS")
	print("NATURAL_SPAWN_EVIDENCE=",folder)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

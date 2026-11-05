extends SceneTree
# Native GPU regression: the Rust fixture supplies real seed-12345 biome sites.
var failed := false
var scene: Node3D
var folder: String
var terrain_peak_ms := 0.0
var max_sections := 0
var sites: Array

func _initialize() -> void: call_deferred("run")
func check(ok: bool, message: String) -> void:
	if ok: print("PASS: ",message)
	else:
		failed = true
		push_error(message)
func frame() -> void:
	var start := Time.get_ticks_usec()
	scene._process(0.0)
	terrain_peak_ms = maxf(terrain_peak_ms,float(Time.get_ticks_usec()-start)/1000.0)
	max_sections = maxi(max_sections,int(scene.state.get("loaded_sections",0)))
	if scene.terrain_updates_last_frame > 2 or max_sections > 392: failed = true
	await process_frame
func settle(until: Callable) -> bool:
	var deadline := Time.get_ticks_msec()+30000
	while Time.get_ticks_msec()<deadline:
		await frame()
		if until.call(): return true
	return false
func run() -> void:
	folder = ProjectSettings.globalize_path("res://../target/ecology-graphics-%d" % Time.get_ticks_usec())
	DirAccess.make_dir_recursive_absolute(folder)
	var data = JSON.parse_string(FileAccess.get_file_as_string("res://../target/ecology-sites.json"))
	if not data is Array:
		push_error("Run the Rust ecology_sites test before this regression")
		quit(1)
		return
	sites = data
	OS.set_environment("EXA_SAVE_PATH",folder.path_join("world3d.bin"))
	OS.set_environment("EXA_IDENTITY_PATH",folder.path_join("client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.set_process(false)
	scene.set_process_input(false)
	scene.set_process_unhandled_input(false)
	scene.hud.menu.loading_settings = true
	scene.environment.sdfgi_enabled = true
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"isolated singleplayer starts")
		quit(1)
		return
	scene.bridge.send_chat("/fly on")
	check(await settle(func(): return scene.state.flying),"authority enables flight")
	scene.hud.hide()
	await create_timer(0.6).timeout
	for site in sites:
		var destination := Vector3(site.x,site.y,site.z)
		scene.bridge.send_chat("/tp %s %s %s" % [site.x,site.y,site.z])
		check(await settle(func(): return scene.state.body_position.distance_to(destination)<0.1 and scene.state.loaded_sections==392 and scene.terrain_pending.is_empty()),"biome streams completely: "+str(site.biome))
		scene.yaw = 0.4
		scene.pitch = -0.15 if site.biome == "Cave" else -0.6
		for i in range(75): await frame()
		var all_resident := true
		for origin in scene.terrain_nodes:
			if not scene.bridge.has_chunk(origin): all_resident = false
		check(all_resident,"visible sections belong to loaded world")
		if DisplayServer.get_name() != "headless":
			await RenderingServer.frame_post_draw
			check(root.get_texture().get_image().save_png(folder.path_join(str(site.biome)+".png"))==OK,"save biome screenshot")
	check(max_sections==392,"128-high cache remains bounded at 49 columns")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"world and new materials save on exit")
	check(scene.terrain_nodes.is_empty() and scene.terrain_pending.is_empty(),"all old sections and pending meshes released")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"128-high world reopens")
	var last: Dictionary = sites.back()
	check(absf(scene.state.body_position.x-float(last.x))<0.1,"explorer position restores")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"reopened world closes")
	var file := FileAccess.open(folder.path_join("metrics.json"),FileAccess.WRITE)
	file.store_string(JSON.stringify({"passed":not failed,"process_peak_ms":terrain_peak_ms,"max_sections":max_sections,"sites":sites},"\t"))
	file.close()
	print("ECOLOGY_RESULT=", "FAIL" if failed else "PASS")
	print("ECOLOGY_EVIDENCE=",folder)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

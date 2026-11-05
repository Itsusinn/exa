extends SceneTree
# Exercise actual authority generation, QUIC streaming, GPU meshes and save/reopen.
var failed := false
var scene: Node3D
var folder: String
var peak_ms := 0.0
var max_sections := 0

func _initialize() -> void: call_deferred("run")
func check(ok: bool, message: String) -> void:
	if ok: print("PASS: ",message)
	else:
		failed = true
		push_error(message)
func frame() -> void:
	var start := Time.get_ticks_usec()
	scene._process(0.0)
	peak_ms = maxf(peak_ms,float(Time.get_ticks_usec()-start)/1000.0)
	max_sections = maxi(max_sections,int(scene.state.get("loaded_sections",0)))
	if scene.terrain_updates_last_frame > 2 or max_sections > 392: failed = true
	await process_frame
func settle(until: Callable) -> bool:
	var deadline := Time.get_ticks_msec()+30000
	while Time.get_ticks_msec()<deadline:
		await frame()
		if until.call(): return true
	return false
func aim(point: Vector3) -> void:
	var offset: Vector3 = point-scene.state.body_position-Vector3(0,1.58,0)
	scene.yaw = atan2(offset.x,-offset.z)
	scene.pitch = atan2(offset.y,Vector2(offset.x,offset.z).length())
func run() -> void:
	folder = ProjectSettings.globalize_path("res://../target/structures-graphics-%d" % Time.get_ticks_usec())
	DirAccess.make_dir_recursive_absolute(folder)
	var sites = JSON.parse_string(FileAccess.get_file_as_string("res://../target/structure-sites.json"))
	if not sites is Array or sites.size()!=4:
		push_error("Run structure_sites_emit_fixture before this regression")
		quit(1)
		return
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
		check(await settle(func(): return scene.state.body_position.distance_to(destination)<0.1 and scene.state.loaded_sections==392 and scene.terrain_pending.is_empty()),"structure streams completely: "+str(site.name))
		scene.yaw = site.yaw
		scene.pitch = site.pitch
		for i in range(90): await frame()
		var center := Vector3i(int(site.center[0]/16)*16,int(site.ground/16)*16,int(site.center[1]/16)*16)
		# Use floor for negative coordinates, matching Rust's Euclidean division.
		center.x = int(floorf(float(site.center[0])/16.0))*16
		center.z = int(floorf(float(site.center[1])/16.0))*16
		check(scene.terrain_nodes.has(center),"structure center has a rendered mesh")
		var all_resident := true
		for origin in scene.terrain_nodes:
			if not scene.bridge.has_chunk(origin): all_resident = false
		check(all_resident,"unloaded structure meshes are removed")
		if DisplayServer.get_name() != "headless":
			await RenderingServer.frame_post_draw
			check(root.get_texture().get_image().save_png(folder.path_join(str(site.name)+".png"))==OK,"save structure screenshot")
	check(max_sections==392,"cache remains bounded at 49 columns")
	var last: Dictionary = sites.back()
	var stand := Vector3(last.edit_stand[0]+0.5,last.edit_stand[1],last.edit_stand[2]+0.5)
	var target := Vector3(last.edit_target[0]+0.5,last.edit_target[1]+0.5,last.edit_target[2]+0.5)
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/tp %s %s %s" % [stand.x,stand.y,stand.z])
	check(await settle(func(): return scene.state.body_position.distance_to(stand)<0.1 and scene.state.ready),"enter generated village at ground level")
	aim(target)
	var selected: Dictionary = scene.bridge.block_target(scene.yaw,scene.pitch)
	check(selected.get("position") == Vector3i(last.edit_target[0],last.edit_target[1],last.edit_target[2]),"village wall is targetable")
	var before: int = scene.state.inventory_revision
	scene.bridge.edit_block(scene.yaw,scene.pitch,false)
	check(await settle(func(): return scene.state.inventory_revision>before),"generated wall can be mined through authority")
	check(scene.state.inventory_slots[0].get("quantity",0)==1,"wall yields exactly one block")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"structures save on exit")
	check(scene.terrain_nodes.is_empty() and scene.terrain_pending.is_empty(),"session releases meshes and pending work")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready and scene.terrain_pending.is_empty()),"structure world reopens")
	check(scene.state.body_position.distance_to(stand)<0.1,"explorer position restores")
	aim(target)
	check(scene.bridge.block_target(scene.yaw,scene.pitch).get("position")!=selected.get("position"),"mined village wall remains absent after restart")
	check(scene.state.inventory_slots[0].get("quantity",0)==1,"collected building block persists")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"reopened world closes")
	var file := FileAccess.open(folder.path_join("metrics.json"),FileAccess.WRITE)
	file.store_string(JSON.stringify({"passed":not failed,"process_peak_ms":peak_ms,"max_sections":max_sections,"sites":sites},"\t"))
	file.close()
	print("STRUCTURES_RESULT=", "FAIL" if failed else "PASS")
	print("STRUCTURES_EVIDENCE=",folder)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

extends SceneTree
# Same route and rendering settings for before/after measurements; isolated saves only.
class MeasuredWorld extends "res://src/main.gd":
	var terrain_usec := 0
	func _rebuild_terrain() -> void:
		var before := Time.get_ticks_usec()
		super._rebuild_terrain()
		terrain_usec += Time.get_ticks_usec() - before

var scene: MeasuredWorld
var cpu: Array[float] = []
var terrain: Array[float] = []
var frames: Array[float] = []
var failed := false
var max_updates := 0
var max_pending := 0

func _initialize() -> void: call_deferred("run")
func frame(measure := false, elapsed := 0.0) -> void:
	var before := Time.get_ticks_usec()
	scene.terrain_usec = 0
	scene._process(elapsed)
	max_updates = maxi(max_updates,scene.terrain_updates_last_frame)
	max_pending = maxi(max_pending,scene.terrain_pending.size())
	if scene.terrain_updates_last_frame > 2 or scene.terrain_pending.size() > 392: failed = true
	if measure:
		cpu.append(float(Time.get_ticks_usec()-before)/1000.0)
		terrain.append(float(scene.terrain_usec)/1000.0)
	await process_frame
	if measure: frames.append(float(Time.get_ticks_usec()-before)/1000.0)
func stats(values: Array[float]) -> Dictionary:
	values.sort()
	var over := 0
	for v in values:
		if v > 33.3: over += 1
	return {"p50_ms":values[values.size()/2], "p95_ms":values[int(values.size()*0.95)], "max_ms":values.back(), "over_33ms":over}
func run() -> void:
	var label := preload("res://src/app_config.gd").environment("PERF_LABEL")
	if label.is_empty(): label = "current"
	var folder := ProjectSettings.globalize_path("res://../target/streaming-perf-%s-%d" % [label,Time.get_ticks_usec()])
	DirAccess.make_dir_recursive_absolute(folder)
	OS.set_environment("EXA_SAVE_PATH",folder.path_join("world3d.bin"))
	OS.set_environment("EXA_IDENTITY_PATH",folder.path_join("client.uuid"))
	scene = MeasuredWorld.new()
	root.add_child(scene)
	current_scene = scene
	scene.set_process(false)
	scene.hud.menu.loading_settings = true
	if preload("res://src/app_config.gd").environment("PERF_GI") == "on": scene.environment.sdfgi_enabled = true
	elif preload("res://src/app_config.gd").environment("PERF_GI") == "off": scene.environment.sdfgi_enabled = false
	scene._singleplayer()
	var deadline := Time.get_ticks_msec() + 20000
	while not scene.state.get("ready",false) or scene.state.get("loaded_sections",0) != 392 or not scene.terrain_pending.is_empty():
		await frame()
		if Time.get_ticks_msec() > deadline:
			push_error("initial world timed out")
			quit(1)
			return
	scene.bridge.send_chat("/fly on")
	for i in range(150): await frame()
	scene.pitch = -0.55
	# Repeated nearby teleports reproduce new strips of columns, then a full replacement.
	for destination in [Vector3(64.5,110,32.5),Vector3(80.5,110,32.5),Vector3(96.5,110,32.5),Vector3(-200.5,110,300.5)]:
		scene.bridge.send_chat("/tp %s %s %s" % [destination.x,destination.y,destination.z])
		var started := Time.get_ticks_msec()
		while Time.get_ticks_msec()-started < 2500 or not scene.terrain_pending.is_empty():
			await frame(true)
			if Time.get_ticks_msec()-started > 15000: break
		if scene.state.body_position.distance_to(destination) > 0.1:
			failed = true
		if not scene.terrain_pending.is_empty():
			push_error("terrain queue did not drain without further revisions")
			failed = true
		for origin in scene.terrain_nodes:
			if not scene.bridge.has_chunk(origin):
				push_error("unloaded terrain retained a node")
				failed = true
	var report := {"label":label,"frames":frames.size(),"cpu":stats(cpu),"terrain":stats(terrain),"frame":stats(frames),"max_updates":max_updates,"max_pending":max_pending,"sdfgi":scene.environment.sdfgi_enabled,"viewport":str(root.size),"passed":not failed}
	cpu.clear()
	terrain.clear()
	frames.clear()
	var start_x: float = scene.state.body_position.x
	scene._focus_gained()
	scene.yaw = 0.0
	Input.action_press("right")
	Input.action_press("sprint")
	var previous := Time.get_ticks_usec()
	deadline = Time.get_ticks_msec() + 7000
	while Time.get_ticks_msec() < deadline:
		var now := Time.get_ticks_usec()
		await frame(true,float(now-previous)/1000000.0)
		previous = now
	Input.action_release("right")
	Input.action_release("sprint")
	report.flight = {"distance":scene.state.body_position.x-start_x,"frames":frames.size(),"cpu":stats(cpu),"terrain":stats(terrain),"frame":stats(frames)}
	if scene.state.body_position.x-start_x < 60 or not scene.state.connected:
		push_error("accelerated flight failed to cross several streaming boundaries")
		failed = true
	for i in range(150): await frame()
	if DisplayServer.get_name() != "headless":
		await RenderingServer.frame_post_draw
		root.get_texture().get_image().save_png(folder.path_join("world.png"))
	scene._leave_game(false)
	deadline = Time.get_ticks_msec()+15000
	while not scene.pending_action.is_empty() and Time.get_ticks_msec()<deadline: await frame()
	if not scene.pending_action.is_empty() or not scene.terrain_pending.is_empty() or not scene.terrain_nodes.is_empty():
		push_error("shutdown retained pending terrain or nodes")
		failed = true
	report.passed = not failed
	var file := FileAccess.open(folder.path_join("metrics.json"),FileAccess.WRITE)
	file.store_string(JSON.stringify(report,"\t"))
	file.close()
	print("STREAMING_PERFORMANCE=",JSON.stringify(report))
	print("STREAMING_PERFORMANCE_PATH=",folder)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

extends SceneTree
## Real Forward+ comparison using the game scene, demo terrain and authored
## industrial meshes. Never starts an authority or writes player settings.
const Lighting = preload("res://src/world_lighting.gd")
const Models = preload("res://src/industry_models.gd")
const WARMUP_FRAMES := 96
const SAMPLE_FRAMES := 64
var scene: Node3D
var output := ""
var failed := false
var report := {}
var viewport_rid: RID
var fixtures: Node3D

func _initialize() -> void:
	call_deferred("run")

func check(ok: bool, message: String) -> void:
	if not ok:
		failed = true
		push_error(message)

func run() -> void:
	if DisplayServer.get_name() == "headless":
		push_error("Lighting review requires a real graphical Forward+ viewport")
		quit(1)
		return
	create_timer(85.0).timeout.connect(func():
		push_error("Lighting review exceeded 85 seconds")
		quit(1))
	output = preload("res://src/app_config.gd").environment("VISUAL_PATH")
	if output.is_empty(): output = ProjectSettings.globalize_path("res://../target/lighting-review")
	if DirAccess.make_dir_recursive_absolute(output) != OK:
		push_error("Cannot create lighting evidence directory: "+output)
		quit(1)
		return
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	# Prevent all test-only changes from reaching settings.cfg.
	scene.hud.menu.loading_settings = true
	scene.hud.menu.set_antialiasing("msaa4")
	scene.hud.gi_toggle.button_pressed = true
	scene.hud.fov_slider.value = 72.0
	scene._demo()
	scene._focus_lost()
	root.size = Vector2i(1280,800)
	var settled := false
	for frame in range(360):
		await process_frame
		if frame > 90 and scene.state.get("ready",false) and scene.terrain_pending.is_empty():
			settled = true
			break
	check(settled,"offline terrain finishes loading before lighting review")
	check(scene.terrain_nodes.size() > 0,"actual game terrain exists")
	check(scene.material != null,"actual voxel shader exists")
	scene.set_process(false)
	scene.hud.visible = false
	_add_equipment()
	viewport_rid = root.get_viewport_rid()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,true)
	report = {
		"engine":Engine.get_version_info().string,
		"adapter":RenderingServer.get_video_adapter_name(),
		"renderer":RenderingServer.get_current_rendering_method(),
		"resolution":[root.size.x,root.size.y],
		"warmup_frames":WARMUP_FRAMES,"samples_per_view":SAMPLE_FRAMES,
		"timing_note":"Viewport GPU and render CPU timestamps exclude script/game simulation; CPU setup is separate. Wall intervals include vsync/scheduling. GPU clock state can affect comparisons.",
		"reflection_note":"SSR only has visible depth/color. Screen-edge/offscreen fallback uses approximate sky/SDFGI, not exact hidden geometry.",
		"views":{},"differences":{}
	}
	var views := [
		{"name":"outdoor","eye":Vector3(28.5,8.58,32.5),"at":Vector3(28.5,8.5,21.0)},
		{"name":"shadow","eye":Vector3(38.8,9.8,29.0),"at":Vector3(36.5,8.2,26.0)},
		{"name":"interior","eye":Vector3(28.5,8.8,23.5),"at":Vector3(26.5,8.8,18.5)},
		{"name":"metal","eye":Vector3(34.0,10.0,46.0),"at":Vector3(32.0,8.4,40.0)}
	]
	for view in views:
		for enhanced in [false,true]:
			_apply_profile(enhanced)
			scene.camera.position = view.eye
			scene.camera.look_at(view.at)
			await _warmup()
			var name: String = view.name+("-after" if enhanced else "-before")
			report.views[name] = await _measure()
			await _capture(name)
		var difference := _difference(view.name+"-before",view.name+"-after")
		report.differences[view.name] = difference
		check(difference.get("changed_samples",0) >= 16,"profile changes rendered geometry/lighting: "+view.name)
	# Isolate SSR from sun, sky, GI, camera and exposure changes.
	_apply_profile(true)
	scene.environment.ssr_enabled = false
	await _warmup()
	report.views["metal-ssr-off"] = await _measure()
	await _capture("metal-ssr-off")
	scene.environment.ssr_enabled = true
	await _warmup()
	report.views["metal-ssr-on"] = await _measure()
	await _capture("metal-ssr-on")
	report.differences["ssr"] = _difference("metal-ssr-off","metal-ssr-on")
	check(report.differences.ssr.get("changed_samples",0) >= 4,"SSR produces a visible change on authored equipment")
	await _edge_motion()
	# The existing player GI switch remains effective through either profile.
	scene.hud.gi_toggle.button_pressed = false
	for enhanced in [false,true]:
		Lighting.apply(scene.environment,scene.sun,enhanced)
		check(not scene.environment.sdfgi_enabled,"profile preserves disabled player GI setting")
	await _warmup()
	report.views["metal-no-gi"] = await _measure()
	await _capture("metal-no-gi")
	scene.hud.gi_toggle.button_pressed = true
	check(scene.environment.sdfgi_enabled,"player GI switch restores indirect light")
	RenderingServer.viewport_set_measure_render_time(viewport_rid,false)
	report["passed"] = not failed
	var file := FileAccess.open(output.path_join("metrics.json"),FileAccess.WRITE)
	check(file != null,"lighting metrics can be written")
	if file != null: file.store_string(JSON.stringify(report,"  "))
	print("LIGHTING_REVIEW_PATH=",output)
	print("GODOT_LIGHTING_RESULT=","FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func _apply_profile(enhanced: bool) -> void:
	Lighting.apply(scene.environment,scene.sun,enhanced)
	check(scene.environment.sdfgi_enabled,"profile preserves enabled GI")
	check(scene.environment.tonemap_mode == Environment.TONE_MAPPER_ACES,"profile preserves ACES")
	check(is_equal_approx(scene.environment.tonemap_exposure,1.0),"comparison uses fixed exposure")
	check(scene.environment.sky != null,"sky fallback resource exists")
	check(scene.environment.ssr_enabled == enhanced,"SSR follows selected profile")
	if enhanced:
		check(scene.environment.reflected_light_source == Environment.REFLECTION_SOURCE_SKY,"sky reflection fallback remains enabled")
		check(scene.environment.ssr_max_steps > 0 and scene.environment.ssr_max_steps <= 64,"SSR work is bounded")

func _warmup() -> void:
	# Let the radiance cubemap and SDFGI history settle after every profile/camera change.
	for frame in range(WARMUP_FRAMES): await process_frame

func _measure() -> Dictionary:
	var gpu: Array[float] = []
	var cpu: Array[float] = []
	var setup: Array[float] = []
	var wall: Array[float] = []
	var previous := Time.get_ticks_usec()
	for frame in range(SAMPLE_FRAMES):
		await process_frame
		await RenderingServer.frame_post_draw
		var now := Time.get_ticks_usec()
		wall.append(float(now-previous)/1000.0)
		previous = now
		gpu.append(RenderingServer.viewport_get_measured_render_time_gpu(viewport_rid))
		cpu.append(RenderingServer.viewport_get_measured_render_time_cpu(viewport_rid))
		setup.append(RenderingServer.get_frame_setup_time_cpu())
	return {"gpu_ms":_summary(gpu),"render_cpu_ms":_summary(cpu),"render_setup_cpu_ms":_summary(setup),"wall_frame_ms":_summary(wall)}

func _summary(samples: Array[float]) -> Dictionary:
	var positive: Array[float] = []
	for value in samples:
		if is_finite(value) and value > 0.0: positive.append(value)
	if positive.is_empty():
		return {"available":false,"valid_samples":0,"total_samples":samples.size(),"reason":"Renderer returned no positive finite timing; unavailable is not zero cost."}
	positive.sort()
	return {"available":true,"valid_samples":positive.size(),"total_samples":samples.size(),"p50":positive[positive.size()/2],"p95":positive[mini(int(ceil(positive.size()*0.95))-1,positive.size()-1)],"max":positive.back()}

func _capture(name: String) -> void:
	await RenderingServer.frame_post_draw
	var image := root.get_texture().get_image()
	check(image != null and not image.is_empty(),"viewport image exists: "+name)
	if image == null or image.is_empty(): return
	check(image.save_png(output.path_join(name+".png")) == OK,"screenshot saved: "+name)

func _difference(before: String, after: String) -> Dictionary:
	var a := Image.load_from_file(output.path_join(before+".png"))
	var b := Image.load_from_file(output.path_join(after+".png"))
	if a == null or b == null or a.get_size() != b.get_size():
		check(false,"comparison images exist at equal dimensions")
		return {}
	var changed := 0
	var samples := 0
	var total := 0.0
	# Central/lower frame prioritizes geometry over the changed sky background.
	for y in range(int(b.get_height()*0.25),b.get_height(),4):
		for x in range(int(b.get_width()*0.1),int(b.get_width()*0.9),4):
			var ca := a.get_pixel(x,y)
			var cb := b.get_pixel(x,y)
			var delta := absf(ca.r-cb.r)+absf(ca.g-cb.g)+absf(ca.b-cb.b)
			total += delta
			samples += 1
			if delta > 0.015: changed += 1
	return {"changed_samples":changed,"samples":samples,"mean_rgb_abs_sum":total/maxi(samples,1)}

func _add_equipment() -> void:
	# Add a few real game models before any GI warmup; these are visual targets,
	# not authoritative construction and not a replacement gameplay test.
	fixtures = Node3D.new()
	fixtures.name = "LightingEquipmentTargets"
	scene.add_child(fixtures)
	var library = Models.new()
	var kinds := [4,9,5]
	var origins := [Vector3(28,7,39),Vector3(31,7,39),Vector3(35,7,39)]
	var metals := 0
	for i in range(kinds.size()):
		var request := {"kind":kinds[i],"axis":0,"diameter":2,"height":3,"pipe_ends":[],"column_design":{}}
		var layout: Dictionary = scene.bridge.equipment_model_layout(request)
		check(not layout.is_empty(),"Rust supplies equipment layout")
		var mesh: ArrayMesh = library.build(layout)
		check(mesh.get_surface_count() > 0,"authored equipment mesh loads")
		for surface in range(mesh.get_surface_count()):
			var material := mesh.surface_get_material(surface) as BaseMaterial3D
			if material != null and material.metallic > 0.9: metals += 1
		var node := MeshInstance3D.new()
		node.mesh = mesh
		node.position = origins[i]
		node.gi_mode = GeometryInstance3D.GI_MODE_STATIC
		fixtures.add_child(node)
	check(metals > 0,"reflection targets include real imported metal surfaces")

func _edge_motion() -> void:
	var original: Transform3D = scene.camera.transform
	# Pan targets toward the viewport edge. Review the frames for popping; a sky
	# fallback cannot preserve the exact reflection of offscreen objects.
	for frame in range(72):
		scene.camera.rotation.y = original.basis.get_euler().y + float(frame)*0.006
		await process_frame
		if frame in [0,23,47,71]: await _capture("reflection-motion-%02d" % frame)
	scene.camera.transform = original
	await _warmup()
	await _capture("reflection-return")

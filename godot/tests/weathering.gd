extends "res://tests/lighting.gd"
## Isolated Forward+ review: shader weathering and atmospheric depth are tested
## independently. Static timestamps and sparse motion images are not gameplay
## benchmarks or proof that continuous motion is free from temporal artifacts.
var sample_root: Node3D
var sample_material: ShaderMaterial
var fixed_light_state: Array
var fog_baseline: Array

func run() -> void:
	if DisplayServer.get_name() == "headless" or RenderingServer.get_current_rendering_method() != "forward_plus":
		push_error("Weathering review requires a real Forward+ viewport")
		quit(1)
		return
	create_timer(240.0).timeout.connect(func():
		push_error("Weathering review exceeded 240 seconds")
		quit(1))
	output = preload("res://src/app_config.gd").environment("VISUAL_PATH")
	if output.is_empty(): output = ProjectSettings.globalize_path("res://../target/weathering-review")
	if DirAccess.make_dir_recursive_absolute(output) != OK:
		push_error("Cannot create weathering evidence directory")
		quit(1)
		return
	# Each invocation gets its own authority files, even when screenshot paths repeat.
	var save_dir := output.path_join("world-%d" % Time.get_ticks_usec())
	DirAccess.make_dir_recursive_absolute(save_dir)
	OS.set_environment("EXA_SAVE_PATH",save_dir.path_join("world3d.bin"))
	OS.set_environment("EXA_IDENTITY_PATH",save_dir.path_join("client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.set_process(false)
	scene.set_process_input(false)
	scene.set_process_unhandled_input(false)
	scene.hud.menu.loading_settings = true
	scene.hud.menu.set_antialiasing("msaa4")
	scene.hud.gi_toggle.button_pressed = true
	scene.hud.fov_slider.value = 72.0
	root.size = Vector2i(1280,800)
	var names: Array = scene.material.shader.get_shader_uniform_list().map(func(value: Dictionary): return value.name)
	check("surface_weathering_strength" in names,"weathering uniform is declared")
	scene._demo()
	scene._focus_lost()
	check(await _settle(func(): return scene.state.get("ready",false) and scene.terrain_pending.is_empty()),"demo terrain finishes loading")
	scene.hud.hide()
	Lighting.apply_atmosphere(scene.environment,false)
	fog_baseline = _fog_state()
	fixed_light_state = _light_state()
	viewport_rid = root.get_viewport_rid()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,true)
	report = {"engine":Engine.get_version_info().string,"adapter":RenderingServer.get_video_adapter_name(),"renderer":RenderingServer.get_current_rendering_method(),"resolution":[1280,800],"samples_per_view":SAMPLE_FRAMES,"warmup_frames":WARMUP_FRAMES,"views":{},"differences":{},"sites":[],"save_directory":save_dir,
		"evidence_note":"Differences establish effect presence only. Sparse motion snapshots do not establish flicker-free continuous motion. GPU timestamps exclude simulation/script work; unavailable timings are not zero cost.",
		"continuity_note":"The split sample spans negative and positive world coordinates with independently mapped local UVs and no exposed edge at the shared boundary. Different world positions need not have equal noise values; inspect the incremental on/off change for artificial seams.",
		"streaming_note":"Real seed-12345 terrain is limited to the authority's loaded 49 columns/392 sections. Fog does not prove hidden geometry exists, extend streaming, or supply a distant terrain mesh."}
	var views := [
		{"name":"stone-exposed","eye":Vector3(37.9,8.7,26.7),"at":Vector3(36.7,8.4,26)},
		{"name":"wood-coplanar","eye":Vector3(28,7.8,24),"at":Vector3(28.6,7,23.3)},
		{"name":"stone-recess","eye":Vector3(28.5,8.8,23.5),"at":Vector3(26.5,8.1,18.5)}]
	for view in views:
		_camera(view.eye,view.at)
		await _compare(view.name,false)
	await _motion("stone-recess",false)
	_build_samples()
	for pattern in [1,2,0,3,10,11,13]:
		_set_sample_pattern(pattern)
		_camera(Vector3(0,101,3),Vector3(0,101,0))
		var name := "negative-split-pattern-%d" % pattern
		await _compare(name,false)
		if pattern in [0,3,10,11,13]:
			check(report.differences[name].get("mean_rgb_abs_sum",1.0) < 0.001,"non-target material remains visually unchanged: "+str(pattern))
	for pattern in [1,2]:
		_set_sample_pattern(pattern,false,0)
		var name := "coplanar-zero-mask-%d" % pattern
		await _compare(name,false)
		check(report.differences[name].get("mean_rgb_abs_sum",1.0)<0.001,"coplanar unoccluded faces retain original surface: "+str(pattern))
		_set_sample_pattern(pattern,false,255)
		await _compare("sheltered-pattern-%d" % pattern,false)
	# Repeat the exact off state after on: catches incomplete development-toggle restoration.
	_set_weathering(false)
	await _warmup()
	await _capture("negative-split-restored-off")
	report.differences["restored-off"] = _difference("sheltered-pattern-2-off","negative-split-restored-off")
	check(report.differences["restored-off"].get("mean_rgb_abs_sum",1.0)<0.001,"weathering off restores the same rendered state")
	sample_root.queue_free()
	sample_material = null
	await process_frame
	scene._leave_game(false)
	check(await _settle(func(): return scene.pending_action.is_empty()),"demo session releases before real ecology")
	await _ecology()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,false)
	var file := FileAccess.open(output.path_join("metrics.json"),FileAccess.WRITE)
	check(file != null,"weathering metrics can be written")
	report["passed"] = not failed
	if file != null: file.store_string(JSON.stringify(report,"  "))
	print("WEATHERING_REVIEW_PATH=",output)
	print("GODOT_WEATHERING_RESULT=","FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func _settle(until: Callable) -> bool:
	var deadline := Time.get_ticks_msec()+30000
	while Time.get_ticks_msec()<deadline:
		scene._process(0.0)
		await process_frame
		if until.call(): return true
	print("WEATHERING_SETTLE_TIMEOUT=",JSON.stringify(_state_diagnostic()))
	return false

func _state_diagnostic() -> Dictionary:
	var result := {"pending_meshes":scene.terrain_pending.size(),"visible_meshes":scene.terrain_nodes.size(),"pending_action":scene.pending_action,"entering_game":scene.entering_game}
	for key in ["ready","demo","local_running","local_error","shutdown_pending","connected","status","generation","revision","loaded_sections","flying","hp","pending","journal"]:
		result[key] = scene.state.get(key)
	var position: Vector3 = scene.state.get("body_position",Vector3.ZERO)
	result["body_position"] = [position.x,position.y,position.z]
	return result

func _camera(eye: Vector3, target: Vector3) -> void:
	scene.camera.position = eye
	scene.camera.look_at(target)

func _set_weathering(enabled: bool) -> void:
	var strength := 1.0 if enabled else 0.0
	scene.material.set_shader_parameter("surface_weathering_strength",strength)
	if sample_material != null: sample_material.set_shader_parameter("surface_weathering_strength",strength)

func _compare(name: String, atmosphere: bool) -> void:
	for enabled in [false,true]:
		if atmosphere: Lighting.apply_atmosphere(scene.environment,enabled)
		else: _set_weathering(enabled)
		check(_light_state()==fixed_light_state,"comparison preserves light, exposure, GI and AA")
		await _warmup()
		var suffix := name+("-on" if enabled else "-off")
		report.views[suffix] = await _measure()
		await _capture(suffix)
	report.differences[name] = _difference(name+"-off",name+"-on")
	if name in ["negative-split-pattern-1","negative-split-pattern-2","sheltered-pattern-1","sheltered-pattern-2","ecology-Forest","ecology-Desert"]:
		# Subtle roughness changes may be below the helper's broad 0.015 RGB
		# threshold. Count quantized-pixel changes separately; this is presence,
		# never a quality score or a claim about the physical accuracy of wear.
		var presence := _small_change_count(name+"-off",name+"-on")
		report.differences[name]["small_change_samples"] = presence
		check(presence>=4,"effect is present on intended visible geometry: "+name)
	if name.begins_with("negative-split") or name.begins_with("coplanar-zero") or name.begins_with("sheltered"):
		var picture := Image.load_from_file(output.path_join(name+"-on.png"))
		if picture != null:
			var face := picture.get_pixel(600,400)
			var background := picture.get_pixel(32,400)
			check(absf(face.r-background.r)+absf(face.g-background.g)+absf(face.b-background.b)>0.15,"sample face is visibly rendered, not just unchanged background: "+name)
	if atmosphere:
		Lighting.apply_atmosphere(scene.environment,false)
		check(_fog_state()==fog_baseline,"atmosphere off restores all baseline fog parameters")

func _small_change_count(before: String, after: String) -> int:
	var a := Image.load_from_file(output.path_join(before+".png"))
	var b := Image.load_from_file(output.path_join(after+".png"))
	if a==null or b==null or a.get_size()!=b.get_size(): return 0
	var changed := 0
	for y in range(280,520,2):
		for x in range(420,860,2):
			var ca := a.get_pixel(x,y)
			var cb := b.get_pixel(x,y)
			if absf(ca.r-cb.r)+absf(ca.g-cb.g)+absf(ca.b-cb.b)>0.001: changed += 1
	return changed

func _motion(name: String, atmosphere: bool) -> void:
	var original: Transform3D = scene.camera.transform
	for enabled in [false,true]:
		scene.camera.transform = original
		if atmosphere: Lighting.apply_atmosphere(scene.environment,enabled)
		else: _set_weathering(enabled)
		await _warmup()
		for frame in range(32):
			scene.camera.position.x += 0.01
			await process_frame
			if frame in [7,15,23,31]: await _capture("%s-motion-%s-%02d" % [name,"on" if enabled else "off",frame])
	scene.camera.transform = original
	if atmosphere: Lighting.apply_atmosphere(scene.environment,false)

func _build_samples() -> void:
	sample_root = Node3D.new()
	scene.add_child(sample_root)
	sample_material = scene.material.duplicate()
	_set_sample_pattern(1)

func _set_sample_pattern(pattern: int, exposed: bool = true, packed_ao: int = 0) -> void:
	for child in sample_root.get_children():
		sample_root.remove_child(child)
		child.queue_free()
	# Two 1m faces, outer exposed edges only. The shared x=0 edge is coplanar.
	for x in [-1,0]:
		var tool := SurfaceTool.new()
		tool.begin(Mesh.PRIMITIVE_TRIANGLES)
		tool.set_material(sample_material)
		var edges := (5+(8 if x == -1 else 2)) if exposed else 0
		for uv in [Vector2(0,0),Vector2(1,1),Vector2(1,0),Vector2(0,0),Vector2(0,1),Vector2(1,1)]:
			tool.set_normal(Vector3.FORWARD * -1.0)
			tool.set_color(Color(0.7,0.6,0.45))
			tool.set_uv(uv)
			tool.set_uv2(Vector2(pattern,(edges<<8)|packed_ao))
			tool.add_vertex(Vector3(x+uv.x,100.5+uv.y,0))
		var mesh := MeshInstance3D.new()
		mesh.mesh = tool.commit()
		mesh.gi_mode = GeometryInstance3D.GI_MODE_DISABLED
		sample_root.add_child(mesh)

func _ecology() -> void:
	var sites = JSON.parse_string(FileAccess.get_file_as_string("res://../target/ecology-sites.json"))
	if not sites is Array:
		check(false,"Rust ecology fixture exists")
		return
	_set_weathering(true)
	scene._singleplayer()
	if not await _settle(func(): return scene.state.get("ready",false)):
		check(false,"isolated ecology authority starts")
		return
	scene.bridge.send_chat("/fly on")
	check(await _settle(func(): return scene.state.get("flying",false)),"authority enables stable airborne camera position")
	# Authority commands have a 15-tick (0.5s) cooldown. Match ecology.gd;
	# sending /tp immediately after /fly is rejected, not a streaming timeout.
	await create_timer(0.6).timeout
	report["streaming_attempts"] = []
	for site in sites:
		if site.biome not in ["Forest","Desert","Cave"]: continue
		var destination := Vector3(site.x,site.y,site.z)
		scene.bridge.send_chat("/tp %s %s %s" % [site.x,site.y,site.z])
		var ready := await _settle(func(): return scene.state.body_position.distance_to(destination)<0.1 and scene.state.loaded_sections==392 and scene.terrain_pending.is_empty())
		var diagnostic := _state_diagnostic()
		diagnostic["biome"] = site.biome
		diagnostic["destination"] = [destination.x,destination.y,destination.z]
		report.streaming_attempts.append(diagnostic)
		print("WEATHERING_STREAMING_STATE=",JSON.stringify(diagnostic))
		check(ready,"actual biome finishes bounded streaming: "+site.biome)
		if not ready: break
		scene.hud.hide()
		var bounds := _terrain_bounds()
		var eye := destination+Vector3(0,1.58,0)
		var direction := Vector3(0.4,-0.18,-1).normalized() if site.biome=="Cave" else Vector3(0.35,-0.28,-1).normalized()
		_camera(eye,eye+direction*20.0)
		# An airborne camera can legitimately sit above every nonempty mesh.
		# Check the authoritative resident section, including empty air sections.
		var camera_section := _section_origin(eye)
		var target_section := _section_origin(eye+direction*20.0)
		check(scene.bridge.has_chunk(camera_section),"ecology camera lies in a resident section: "+site.biome)
		check(scene.bridge.has_chunk(target_section),"review target lies in a resident section: "+site.biome)
		var entry := {"biome":site.biome,"position":[eye.x,eye.y,eye.z],"loaded_sections":scene.state.loaded_sections,"visible_meshes":scene.terrain_nodes.size(),"resident_mesh_bounds":{"position":[bounds.position.x,bounds.position.y,bounds.position.z],"size":[bounds.size.x,bounds.size.y,bounds.size.z]}}
		entry["camera_resident_section"] = [camera_section.x,camera_section.y,camera_section.z]
		entry["target_resident_section"] = [target_section.x,target_section.y,target_section.z]
		var hit_distance := _camera_hit_distance(eye,direction)
		entry["center_ray_hit_m"] = hit_distance
		check(hit_distance<0.0 or hit_distance>0.4,"actual terrain triangles do not block camera immediately: "+site.biome)
		report.sites.append(entry)
		await _compare("ecology-"+site.biome,true)
		if site.biome=="Forest": await _motion("ecology-Forest",true)
	Lighting.apply_atmosphere(scene.environment,false)
	scene._leave_game(false)
	check(await _settle(func(): return scene.pending_action.is_empty()),"isolated ecology authority saves and closes")
	check(scene.terrain_nodes.is_empty() and scene.terrain_pending.is_empty(),"ecology geometry and queued uploads release")

func _section_origin(point: Vector3) -> Vector3i:
	# Truncation is incorrect for negative coordinates, e.g. -191.5 -> -192.
	return Vector3i((point/16.0).floor())*16

func _terrain_bounds() -> AABB:
	var bounds := AABB()
	var first := true
	for origin in scene.terrain_nodes:
		check(scene.bridge.has_chunk(origin),"visible mesh belongs to a resident authoritative section")
		var node: MeshInstance3D = scene.terrain_nodes[origin]
		var box: AABB = node.global_transform * node.mesh.get_aabb()
		bounds = box if first else bounds.merge(box)
		first = false
	return bounds

func _camera_hit_distance(eye: Vector3, direction: Vector3) -> float:
	var nearest := INF
	for node: MeshInstance3D in scene.terrain_nodes.values():
		var box: AABB = node.global_transform * node.mesh.get_aabb()
		if box.intersects_ray(eye,direction)==null: continue
		var triangles: PackedVector3Array = node.mesh.get_faces()
		for i in range(0,triangles.size(),3):
			var point = Geometry3D.ray_intersects_triangle(eye,direction,node.global_transform*triangles[i],node.global_transform*triangles[i+1],node.global_transform*triangles[i+2])
			if point != null: nearest = minf(nearest,eye.distance_to(point))
	return nearest if is_finite(nearest) else -1.0

func _light_state() -> Array:
	return [scene.environment.tonemap_mode,scene.environment.tonemap_exposure,scene.environment.sdfgi_enabled,scene.environment.ssr_enabled,scene.environment.sky,scene.sun.rotation,scene.sun.light_color,scene.sun.light_energy,root.msaa_3d,scene.camera.fov]

func _fog_state() -> Array:
	var e: Environment = scene.environment
	return [e.fog_enabled,e.fog_mode,e.fog_density,e.fog_light_color,e.fog_light_energy,e.fog_sun_scatter,e.fog_sky_affect,e.fog_height,e.fog_height_density,e.fog_aerial_perspective,e.fog_depth_begin,e.fog_depth_end,e.fog_depth_curve,e.volumetric_fog_enabled]

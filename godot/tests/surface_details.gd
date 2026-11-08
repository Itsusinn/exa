extends "res://tests/lighting.gd"
## Phase 3A uses the phase 2 lighting unchanged. The inherited helpers capture
## real viewport images and render timestamps; no authority/save is opened.
var model_library: RefCounted
var source_states := {}
var target_meshes: Array[ArrayMesh] = []

func run() -> void:
	if DisplayServer.get_name() == "headless" or RenderingServer.get_current_rendering_method() != "forward_plus":
		push_error("Surface detail review requires a real Forward+ viewport")
		quit(1)
		return
	create_timer(85.0).timeout.connect(func():
		push_error("Surface detail review exceeded 85 seconds")
		quit(1))
	output = preload("res://src/app_config.gd").environment("VISUAL_PATH")
	if output.is_empty(): output = ProjectSettings.globalize_path("res://../target/surface-details-review")
	if DirAccess.make_dir_recursive_absolute(output) != OK:
		push_error("Cannot create surface detail evidence directory: "+output)
		quit(1)
		return
	# Snapshot imported resources before constructing any IndustryModels instance.
	var imported: Node = Models.Library.instantiate()
	_remember_imported(imported)
	imported.free()
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
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
	check(settled,"offline terrain finishes loading")
	check(scene.terrain_nodes.size() > 0,"actual game terrain is present")
	var uniform_names: Array = scene.material.shader.get_shader_uniform_list().map(func(value: Dictionary): return value.name)
	check("surface_detail_strength" in uniform_names,"voxel detail control exists")
	scene.set_process(false)
	scene.hud.visible = false
	# Isolate phase 3A from later weathering and atmosphere changes.
	scene.material.set_shader_parameter("surface_weathering_strength",0.0)
	Lighting.apply_atmosphere(scene.environment,false)
	_build_targets()
	await _wait_for_textures()
	_validate_material_contract()
	viewport_rid = root.get_viewport_rid()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,true)
	report = {
		"engine":Engine.get_version_info().string,
		"adapter":RenderingServer.get_video_adapter_name(),
		"renderer":RenderingServer.get_current_rendering_method(),
		"resolution":[root.size.x,root.size.y],
		"warmup_frames":WARMUP_FRAMES,"samples_per_view":SAMPLE_FRAMES,
		"timing_note":"Viewport render timestamps exclude gameplay and script work. Static scene, fixed phase 2 lighting, ACES exposure 1 and MSAA4. GPU clocks and vsync influence measurements.",
		"visual_note":"Image differences only establish effect presence. Human review of close, distant and moving images is required for quality and aliasing.",
		"scale_note":"Same cached material and world triplanar scale on 2x3 and 6x6 tanks; mesh dimensions are baked into vertices and instance scale stays one. This checks coordinate policy, not a metrology measurement of pictured scratches.",
		"views":{},"differences":{}
	}
	var views := [
		{"name":"stone-close","eye":Vector3(37.9,8.7,26.7),"at":Vector3(36.7,8.4,26.0)},
		{"name":"wood-close","eye":Vector3(28.0,7.8,24.0),"at":Vector3(28.6,7.0,23.3)},
		{"name":"equipment-close","eye":Vector3(32.3,8.1,43.0),"at":Vector3(32.3,8.0,41.6)},
		{"name":"equipment-scales","eye":Vector3(34.0,13.0,53.0),"at":Vector3(26.0,9.5,42.0)},
		{"name":"distant","eye":Vector3(48.0,17.0,56.0),"at":Vector3(29.0,8.5,29.0)}
	]
	for view in views:
		scene.camera.position = view.eye
		scene.camera.look_at(view.at)
		for enabled in [false,true]:
			_set_detail(enabled)
			await _warmup()
			var name: String = view.name+("-on" if enabled else "-off")
			report.views[name] = await _measure()
			await _capture(name)
		var difference := _difference(view.name+"-off",view.name+"-on")
		report.differences[view.name] = difference
		if view.name.ends_with("-close"):
			check(difference.get("changed_samples",0) >= 8,"detail comparison differs; visual review required: "+view.name)
	# Moving near and far views make unstable tiny highlights easier to inspect.
	for view in [views[2],views[4]]:
		for enabled in [false,true]:
			_set_detail(enabled)
			scene.camera.position = view.eye
			scene.camera.look_at(view.at)
			await _warmup()
			for frame in range(32):
				scene.camera.position.x += 0.012
				await process_frame
				if frame in [7,15,23,31]:
					await _capture("%s-motion-%s-%02d" % [view.name,"on" if enabled else "off",frame])
	_set_detail(true)
	_validate_sources_unchanged()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,false)
	var file := FileAccess.open(output.path_join("metrics.json"),FileAccess.WRITE)
	check(file != null,"surface detail metrics can be written")
	report["passed"] = not failed
	if file != null: file.store_string(JSON.stringify(report,"  "))
	print("SURFACE_DETAILS_REVIEW_PATH=",output)
	print("GODOT_SURFACE_DETAILS_RESULT=","FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func _set_detail(enabled: bool) -> void:
	scene.material.set_shader_parameter("surface_detail_strength",1.0 if enabled else 0.0)
	model_library.set_surface_detail(enabled)
	check(scene.environment.tonemap_mode == Environment.TONE_MAPPER_ACES,"ACES stays fixed")
	check(is_equal_approx(scene.environment.tonemap_exposure,1.0),"exposure stays fixed")
	check(scene.environment.sdfgi_enabled and scene.environment.ssr_enabled,"phase 2 GI/reflections remain enabled")
	check(root.msaa_3d == Viewport.MSAA_4X,"MSAA4 stays fixed")

func _build_targets() -> void:
	fixtures = Node3D.new()
	fixtures.name = "SurfaceDetailEquipmentTargets"
	scene.add_child(fixtures)
	model_library = Models.new()
	var requests := [
		{"kind":4,"diameter":2,"height":3,"origin":Vector3(28,7,39)},
		{"kind":4,"diameter":6,"height":6,"origin":Vector3(20,7,39)},
		{"kind":9,"diameter":2,"height":3,"origin":Vector3(31,7,39)},
		{"kind":5,"diameter":2,"height":3,"origin":Vector3(35,7,39)}
	]
	for request in requests:
		var layout: Dictionary = scene.bridge.equipment_model_layout({"kind":request.kind,"axis":0,"diameter":request.diameter,"height":request.height,"pipe_ends":[],"column_design":{}})
		check(not layout.is_empty(),"Rust supplies real equipment layout")
		var mesh: ArrayMesh = model_library.build(layout)
		check(mesh.get_surface_count() > 0,"authored equipment geometry loads")
		target_meshes.append(mesh)
		var node := MeshInstance3D.new()
		node.mesh = mesh
		node.position = request.origin
		node.gi_mode = GeometryInstance3D.GI_MODE_STATIC
		fixtures.add_child(node)
		check(node.scale == Vector3.ONE,"device size is baked into geometry, not texture instance scale")
	var small: ArrayMesh = target_meshes[0]
	var large: ArrayMesh = target_meshes[1]
	check(large.get_aabb().size.x > small.get_aabb().size.x*2.5,"scale comparison includes different real tank diameters")
	var shared := 0
	for a in range(small.get_surface_count()):
		for b in range(large.get_surface_count()):
			if small.surface_get_material(a) == large.surface_get_material(b): shared += 1
	check(shared > 0,"different tank dimensions reuse cached finish resources")

func _wait_for_textures() -> void:
	for frame in range(360):
		var pending := false
		var granular: Texture2D = scene.material.get_shader_parameter("granular_normal")
		if granular != null and granular.get_image() == null: pending = true
		for material in model_library.materials.cached_materials.values():
			for texture in [material.normal_texture,material.roughness_texture]:
				if texture != null and texture.get_image() == null: pending = true
		if not pending: return
		await process_frame
	check(false,"shared microtexture generation completes before comparison")

func _validate_material_contract() -> void:
	var cache: Dictionary = model_library.materials.cached_materials
	check(not cache.is_empty(),"runtime finish cache is populated")
	var detailed := 0
	for original in cache:
		var finish: StandardMaterial3D = cache[original]
		check(finish != original,"runtime detail uses a material copy")
		check(finish.albedo_color.is_equal_approx(original.albedo_color),"detail preserves authored base color")
		check(is_equal_approx(finish.metallic,original.metallic),"detail preserves authored metalness")
		check(finish.clearcoat_enabled == original.clearcoat_enabled and is_equal_approx(finish.clearcoat,original.clearcoat),"detail preserves authored clearcoat")
		check(is_equal_approx(finish.clearcoat_roughness,original.clearcoat_roughness),"detail preserves coat roughness")
		if finish.normal_texture != null or finish.roughness_texture != null:
			detailed += 1
			check(finish.uv1_triplanar and finish.uv1_world_triplanar,"detail sampling is world triplanar")
			check(finish.uv1_scale.is_equal_approx(Vector3(4,4,4)),"all tank dimensions retain 0.25 metre texture period")
	check(detailed > 0,"real equipment receives microtexture resources")
	var cached_ids := cache.values().map(func(value: Material): return value.get_instance_id())
	model_library.set_surface_detail(false)
	for original in cache:
		check(_material_state(cache[original]) == _material_state(original),"detail off restores authored material properties: "+original.resource_name)
	model_library.set_surface_detail(true)
	check(cache.values().map(func(value: Material): return value.get_instance_id()) == cached_ids,"off/on reuses existing cached materials")
	_validate_sources_unchanged()

func _remember_imported(node: Node) -> void:
	if node is MeshInstance3D:
		for surface in range(node.mesh.get_surface_count()):
			var material := node.mesh.surface_get_material(surface) as BaseMaterial3D
			if material != null: source_states[material] = _material_state(material)
	for child in node.get_children(): _remember_imported(child)

func _material_state(material: BaseMaterial3D) -> Array:
	return [material.albedo_color,material.metallic,material.roughness,material.clearcoat_enabled,material.clearcoat,material.clearcoat_roughness,material.normal_enabled,material.normal_texture,material.normal_scale,material.roughness_texture,material.roughness_texture_channel,material.uv1_triplanar,material.uv1_world_triplanar,material.uv1_triplanar_sharpness,material.uv1_scale,material.texture_filter]

func _validate_sources_unchanged() -> void:
	for original in source_states:
		check(_material_state(original) == source_states[original],"imported source material is unchanged: "+original.resource_name)

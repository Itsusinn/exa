extends SceneTree
# Run with the graphical renderer. Captures use the offline demo, never real saves.
var scene: Node3D
var output: String
var visual_failed := false

func _initialize() -> void: call_deferred("run")

func snapshot(file: String) -> void:
	for i in range(16): await process_frame
	await RenderingServer.frame_post_draw
	var result := root.get_texture().get_image().save_png(output.path_join(file))
	if result != OK:
		push_error("Cannot write visual evidence: " + file)
		quit(1)

func run() -> void:
	output = OS.get_environment("CDDA_VISUAL_PATH")
	if output.is_empty(): output = ProjectSettings.globalize_path("res://../target/visual-review")
	DirAccess.make_dir_recursive_absolute(output)
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.menu.set_antialiasing("msaa4")
	scene.hud.gi_toggle.button_pressed = true
	scene.hud.fov_slider.value = 72
	scene._demo()
	scene.yaw = -0.28
	scene.pitch = -0.06
	for i in range(90): await process_frame
	scene.hud.visible = false
	await snapshot("background.png")
	# Same camera, geometry, sky and exposure. Disable SSAO in both captures to
	# isolate neighbor-sampled contact AO from screen-space occlusion.
	var ssao_enabled: bool = scene.environment.ssao_enabled
	scene.environment.ssao_enabled = false
	scene.material.set_shader_parameter("contact_ao_strength",0.0)
	await snapshot("contact-ao-off.png")
	scene.material.set_shader_parameter("contact_ao_strength",0.85)
	await snapshot("contact-ao-on.png")
	# Inspect the demo's actual voxel staircase at block scale, not a mock mesh.
	var camera_transform: Transform3D = scene.camera.transform
	scene.set_process(false)
	scene.camera.position = Vector3(40.5,11.5,30.5)
	scene.camera.look_at(Vector3(36.5,8.5,25.5))
	for i in range(90): await process_frame
	scene.material.set_shader_parameter("contact_ao_strength",0.0)
	await snapshot("steps-ao-off.png")
	scene.material.set_shader_parameter("contact_ao_strength",0.85)
	await snapshot("steps-ao-on.png")
	scene.material.set_shader_parameter("bevel_width",0.0)
	scene.material.set_shader_parameter("material_variation",0.0)
	await snapshot("steps-material-before.png")
	scene.material.set_shader_parameter("bevel_width",0.035)
	scene.material.set_shader_parameter("material_variation",0.08)
	await snapshot("steps-material-after.png")
	scene.camera.position = Vector3(38.8,9.8,29.0)
	scene.camera.look_at(Vector3(36.5,8.2,26.0))
	for i in range(60): await process_frame
	scene.material.set_shader_parameter("bevel_width",0.0)
	scene.material.set_shader_parameter("material_variation",0.0)
	await snapshot("detail-material-before.png")
	scene.material.set_shader_parameter("bevel_width",0.035)
	scene.material.set_shader_parameter("material_variation",0.08)
	await snapshot("detail-material-after.png")
	scene.material.set_shader_parameter("normal_strength",0.0)
	await snapshot("detail-normal-off.png")
	scene.material.set_shader_parameter("normal_strength",1.0)
	await snapshot("detail-normal-on.png")
	check_normal_difference("detail-normal")
	await antialiasing_comparison()
	await normal_gallery()
	scene.camera.transform = camera_transform
	scene.set_process(true)
	scene.environment.ssao_enabled = ssao_enabled
	scene.hud.visible = true
	await snapshot("world.png")
	scene.hud.inventory_open = true
	await snapshot("inventory.png")
	scene._close_inventory()
	scene.hud.menu.in_game = true
	scene.hud.menu.show_page("pause")
	await snapshot("pause.png")
	scene._disconnect_game()
	for i in range(2): await process_frame
	scene.hud.menu.in_game = false
	scene.hud.menu.show_page("home")
	await snapshot("menu.png")
	scene.hud.menu.open_settings()
	await snapshot("settings.png")
	print("VISUAL_REVIEW_PATH=",output)
	scene.queue_free()
	await process_frame
	quit(1 if visual_failed else 0)

func normal_gallery() -> void:
	# Rendering fixture for material branches absent from the offline demo.
	var texture: Texture2D = scene.material.get_shader_parameter("granular_normal")
	for i in range(300):
		if texture.get_image() != null: break
		await process_frame
	if texture.get_image() == null:
		push_error("Granular normal texture did not finish generating")
		quit(1)
		return
	texture.get_image().save_png(output.path_join("granular-normal-map.png"))
	var nodes: Array[MeshInstance3D] = []
	var patterns := [7,7,10,11]
	var colors := [Color(0.78,0.70,0.49),Color(0.47,0.43,0.36),Color(0.90,0.94,0.96),Color(0.53,0.75,0.84)]
	for i in range(4):
		var box := BoxMesh.new()
		box.size = Vector3(1.8,1.8,1.8)
		var arrays := box.surface_get_arrays(0)
		var count: int = arrays[Mesh.ARRAY_VERTEX].size()
		var uv2 := PackedVector2Array()
		var tint := PackedColorArray()
		uv2.resize(count)
		uv2.fill(Vector2(patterns[i],0))
		tint.resize(count)
		tint.fill(colors[i])
		arrays[Mesh.ARRAY_TEX_UV2] = uv2
		arrays[Mesh.ARRAY_COLOR] = tint
		var mesh := ArrayMesh.new()
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays)
		mesh.surface_set_material(0,scene.material)
		var node := MeshInstance3D.new()
		node.mesh = mesh
		node.position = Vector3(80+i*2.2,8,80)
		scene.add_child(node)
		nodes.append(node)
	scene.camera.position = Vector3(84,11.5,88)
	scene.camera.look_at(Vector3(83.3,8,80))
	for i in range(60): await process_frame
	scene.material.set_shader_parameter("normal_strength",0.0)
	await snapshot("granular-normal-off.png")
	scene.material.set_shader_parameter("normal_strength",1.0)
	await snapshot("granular-normal-on.png")
	check_normal_difference("granular-normal")
	for node in nodes: node.queue_free()

func check_normal_difference(prefix: String) -> void:
	var off := Image.load_from_file(output.path_join(prefix+"-off.png"))
	var on := Image.load_from_file(output.path_join(prefix+"-on.png"))
	var changed := 0
	for y in range(0,on.get_height(),8):
		for x in range(0,on.get_width(),8):
			var a := off.get_pixel(x,y)
			var b := on.get_pixel(x,y)
			if absf(a.r-b.r)+absf(a.g-b.g)+absf(a.b-b.b) > 0.02: changed += 1
	print("NORMAL_MAP_CHANGED_SAMPLES ",prefix,"=",changed)
	if changed < 16:
		visual_failed = true
		push_error("Normal map has no visible effect: "+prefix)
		quit(1)

func antialiasing_comparison() -> void:
	var original: Transform3D = scene.camera.transform
	for mode in ["off","fxaa","msaa2","msaa4","taa"]:
		scene.hud.menu.set_antialiasing(mode)
		for i in range(32): await process_frame
		await snapshot("aa-"+mode+".png")
	# Capture moving frames without letting the temporal history settle first.
	for mode in ["taa","msaa4"]:
		scene.hud.menu.set_antialiasing(mode)
		scene.camera.transform = original
		for i in range(60):
			scene.camera.position.x += 0.015
			await process_frame
			if i % 15 == 14:
				await RenderingServer.frame_post_draw
				root.get_texture().get_image().save_png(output.path_join("aa-%s-motion-%d.png" % [mode,i]))
	scene.camera.transform = original
	scene.hud.menu.set_antialiasing("msaa4")

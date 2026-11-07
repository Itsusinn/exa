extends SceneTree
## Authored geometry must fit Rust collision, including every tank/column size,
## every feed level, and all pipe/tee/pump orientations. No world save is opened.
const Models = preload("res://src/industry_models.gd")
var failed := false
var checked := 0
var library = Models.new()
var bridge: GameBridge

func _initialize() -> void:
	call_deferred("run")

func check(ok: bool, message: String) -> void:
	if not ok:
		failed = true
		push_error(message)

func specification(diameter: int, trays: int, feed: int) -> Dictionary:
	return {"diameter":diameter,"thermo_revision":"butane-pentane-pr78-cp298-r1","spec":{
		"trays":trays,"feed_tray":feed,"feed_mol_s":1.0,"butane_fraction":0.5,
		"feed_temperature_k":330.0,"pressure_pa":300000.0,"reflux_ratio":2.0,"distillate_fraction":0.5}}

func input(kind: int, axis := 0, diameter := 2, height := 3, ends: Array = [], design: Dictionary = {}) -> Dictionary:
	return {"kind":kind,"axis":axis,"diameter":diameter,"height":height,"pipe_ends":ends,"column_design":design}

func verify(request: Dictionary) -> ArrayMesh:
	var layout := bridge.equipment_model_layout(request)
	check(not layout.is_empty(),"valid model layout: "+str(request))
	var mesh: ArrayMesh = library.build(layout)
	check(mesh.get_surface_count() > 0,"model has surfaces: "+str(request))
	var bounds: Array[AABB] = []
	for box in layout.get("collision_boxes",[]): bounds.append(AABB(box.min,box.max-box.min).grow(0.001))
	var outside := 0
	var first := Vector3.ZERO
	for surface in range(mesh.get_surface_count()):
		check(mesh.surface_get_material(surface) != null,"authored material retained")
		var arrays := mesh.surface_get_arrays(surface)
		var normals: PackedVector3Array = arrays[Mesh.ARRAY_NORMAL]
		for index in range(arrays[Mesh.ARRAY_VERTEX].size()):
			var vertex: Vector3 = arrays[Mesh.ARRAY_VERTEX][index]
			check(vertex.is_finite() and normals[index].is_finite(),"finite authored vertex/normal")
			var inside := false
			for box in bounds:
				if box.has_point(vertex):
					inside = true
					break
			if not inside:
				if outside == 0: first = vertex
				outside += 1
	check(outside == 0,"%d vertices outside shared collision at %s: %s" % [outside,first,request])
	# Every flange begins at its Rust port and points into the occupied footprint.
	for port in layout.get("ports",[]):
		check(mesh.get_aabb().grow(0.001).has_point(port.position),"port endpoint retained")
	checked += 1
	return mesh

func run() -> void:
	bridge = GameBridge.new()
	root.add_child(bridge)
	verify_finishes()
	if "--gallery" in OS.get_cmdline_user_args():
		await gallery()
		bridge.queue_free()
		await process_frame
		print("GODOT_INDUSTRY_GALLERY_RESULT=","FAIL" if failed else "PASS")
		quit(1 if failed else 0)
		return
	check(library.components.size() == 15,"all Blender components imported")
	for invalid in [{},input(99),input(2,3),input(0,0,1),input(1,0,2,3,[1,1]),input(7)]:
		check(bridge.equipment_model_layout(invalid).is_empty(),"invalid layout rejected")
	for kind in [0,4]:
		for diameter in range(2,7):
			for height in range(3,13): verify(input(kind,0,diameter,height))
	for kind in [7,8]:
		for diameter in range(2,7):
			for trays in range(2,13):
				for feed in range(1,trays+1): verify(input(kind,0,diameter,trays+2,[],specification(diameter,trays,feed)))
	for a in range(6):
		for b in range(a+1,6): verify(input(1,0,2,3,[a,b]))
	for axis in range(12): verify(input(11,axis))
	for kind in [2,3]:
		for axis in range(3): verify(input(kind,axis))
	for axis in [0,2]: verify(input(6,axis))
	for kind in [5,9,10]: verify(input(kind))
	print("BLENDER_MODELS_CHECKED=",checked)
	bridge.queue_free()
	await process_frame
	print("GODOT_INDUSTRY_MODELS_RESULT=","FAIL" if failed else "PASS")
	quit(1 if failed else 0)

func verify_finishes() -> void:
	# Check the actual imported glTF, not only the Blender source parameters.
	var materials := {}
	for component in library.components.values():
		var mesh: Mesh = component.mesh
		for i in range(mesh.get_surface_count()):
			var finish := mesh.surface_get_material(i) as BaseMaterial3D
			check(finish != null,"imported PBR finish exists")
			if finish != null: materials[finish.resource_name.replace(" ","_")] = finish
	for name in ["Brushed_stainless","Safety_brass"]:
		check(materials.has(name),"imported metal exists: "+name)
		if materials.has(name): check(is_equal_approx(materials[name].metallic,1.0),"bare metal survives glTF: "+name)
	for name in ["Cool_process","Warm_process","Marine_enamel","Gauge_porcelain","Cast_graphite"]:
		check(materials.has(name),"imported dielectric exists: "+name)
		if not materials.has(name): continue
		var finish: BaseMaterial3D = materials[name]
		check(is_zero_approx(finish.metallic),"coating is dielectric: "+name)
		if name != "Cast_graphite": check(finish.clearcoat_enabled and finish.clearcoat > 0.0,"coat survives glTF: "+name)
	# Runtime port bands must match the corresponding authored finish/color space.
	for i in range(4):
		var name: String = ["Warm_process","Cool_process","Safety_brass","Marine_enamel"][i]
		if not materials.has(name): continue
		var authored: BaseMaterial3D = materials[name]
		var accent: StandardMaterial3D = library.accents[i]
		check(accent.albedo_color.is_equal_approx(authored.albedo_color),"accent color matches imported linear base color: "+name)
		check(is_equal_approx(accent.metallic,authored.metallic),"accent metalness matches: "+name)

func gallery() -> void:
	var world := Node3D.new()
	root.add_child(world)
	var environment := WorldEnvironment.new()
	var settings := Environment.new()
	settings.background_mode = Environment.BG_COLOR
	settings.background_color = Color(0.15,0.20,0.23)
	settings.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	settings.ambient_light_color = Color(0.77,0.86,1.0)
	settings.ambient_light_energy = 0.65
	var sky := Sky.new()
	sky.sky_material = ProceduralSkyMaterial.new()
	settings.sky = sky
	settings.tonemap_mode = Environment.TONE_MAPPER_ACES
	settings.ssao_enabled = true
	settings.sdfgi_enabled = true
	environment.environment = settings
	world.add_child(environment)
	var sun := DirectionalLight3D.new()
	sun.rotation_degrees = Vector3(-52,-35,0)
	sun.light_energy = 1.8
	sun.shadow_enabled = true
	world.add_child(sun)
	var floor := MeshInstance3D.new()
	var slab := BoxMesh.new()
	slab.size = Vector3(23,0.12,16)
	floor.mesh = slab
	floor.position = Vector3(6,-0.08,3)
	var concrete := StandardMaterial3D.new()
	concrete.albedo_color = Color(0.22,0.25,0.25)
	concrete.roughness = 0.9
	floor.material_override = concrete
	world.add_child(floor)
	var items := [input(0),input(4),input(7,0,2,8,[],specification(2,6,3)),input(8,0,3,10,[],specification(3,8,4)),input(9),input(10),input(5),input(2),input(3),input(6),input(1,0,2,3,[1,5]),input(11,2)]
	var locations := [Vector3(0,0,0),Vector3(3,0,0),Vector3(6,0,0),Vector3(9,0,0),Vector3(0,0,4),Vector3(4,0,4),Vector3(8,0,4),Vector3(0,0,8),Vector3(2,0,8),Vector3(4,0,8),Vector3(6,0,8),Vector3(8,0,8)]
	for i in range(items.size()):
		var model := MeshInstance3D.new()
		model.mesh = library.build(bridge.equipment_model_layout(items[i]))
		model.position = locations[i]
		model.gi_mode = GeometryInstance3D.GI_MODE_STATIC
		world.add_child(model)
	var camera := Camera3D.new()
	world.add_child(camera)
	camera.position = Vector3(17,12,22)
	camera.look_at(Vector3(5.5,3.5,3.5))
	camera.projection = Camera3D.PROJECTION_ORTHOGONAL
	camera.size = 16
	camera.current = true
	root.size = Vector2i(1600,1000)
	root.msaa_3d = Viewport.MSAA_4X
	for frame in range(90): await process_frame
	await RenderingServer.frame_post_draw
	var path := ProjectSettings.globalize_path("res://../target/blender-models-gallery.png")
	check(root.get_texture().get_image().save_png(path) == OK,"gallery screenshot saved")
	world.queue_free()

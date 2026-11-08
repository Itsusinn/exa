extends RefCounted
## Blender-authored components, assembled only when a device shape changes.
## Port positions, dimensions and collision envelopes come from the Rust bridge.
const Library = preload("res://assets/industry/chemical_equipment.glb")
const PumpDirection = preload("res://src/pump_direction.gd")
const SurfaceMaterials = preload("res://src/industry_materials.gd")
var components: Dictionary = {}
var surfaces: Dictionary = {}
var accents: Array[StandardMaterial3D] = []
var materials = SurfaceMaterials.new()

func _init() -> void:
	var library := Library.instantiate()
	_read_components(library,Transform3D.IDENTITY)
	library.free()
	for colour in [Color(0.68,0.245,0.055),Color(0.045,0.29,0.57),Color(0.7,0.49,0.13),Color(0.055,0.23,0.24)]:
		var material := StandardMaterial3D.new()
		material.resource_name = ["Warm_process","Cool_process","Safety_brass","Marine_enamel"][accents.size()]
		# Blender/glTF base colors are linear; Godot's color property is sRGB.
		material.albedo_color = colour.linear_to_srgb()
		var brass := accents.size() == 2
		material.metallic = 1.0 if brass else 0.0
		material.roughness = 0.30 if brass else 0.38
		material.clearcoat_enabled = not brass
		material.clearcoat = 0.28
		material.clearcoat_roughness = 0.24
		accents.append(material)

func set_surface_detail(enabled: bool) -> void:
	materials.set_surface_detail(enabled)

func _read_components(node: Node, parent_transform: Transform3D) -> void:
	var transform := parent_transform
	if node is Node3D: transform *= node.transform
	if node is MeshInstance3D:
		components[str(node.name)] = {"mesh":node.mesh,"transform":transform}
	for child in node.get_children(): _read_components(child,transform)

func _part(name: String, position := Vector3.ZERO, scale := Vector3.ONE, basis := Basis.IDENTITY, material: Material = null) -> void:
	assert(components.has(name),"Missing Blender component: "+name)
	var source: Dictionary = components[name]
	var mesh: Mesh = source.mesh
	var transform: Transform3D = Transform3D(basis.scaled_local(scale),position)*source.transform
	for index in range(mesh.get_surface_count()):
		var paint: Material = materials.resolve(material if material != null else mesh.surface_get_material(index))
		if not surfaces.has(paint):
			var tool := SurfaceTool.new()
			tool.begin(Mesh.PRIMITIVE_TRIANGLES)
			tool.set_material(paint)
			surfaces[paint] = tool
		surfaces[paint].append_from(mesh,index,transform)

func _along(direction: Vector3) -> Basis:
	var reference := Vector3.UP if absf(direction.y) < 0.9 else Vector3.BACK
	var radial := reference - direction*reference.dot(direction)
	radial = radial.normalized()
	return Basis(direction,radial,direction.cross(radial))

func build(layout: Dictionary) -> ArrayMesh:
	surfaces.clear()
	if layout.is_empty(): return ArrayMesh.new()
	var kind: int = layout.kind
	var axis: int = layout.axis
	if kind in [0,4,7,8]:
		_vessel(layout)
	elif kind in [2,3]:
		var basis := PumpDirection.direction_basis(axis,false)
		_part("pump" if kind == 2 else "valve",Vector3.ONE*0.5-basis*(Vector3.ONE*0.5),Vector3.ONE,basis)
	elif kind == 5:
		_part("heat_exchanger")
	elif kind == 6:
		var basis := Basis(Vector3.UP,PI*0.5) if axis == 2 else Basis.IDENTITY
		_part("pipe_rack",Vector3.ONE*0.5-basis*(Vector3.ONE*0.5),Vector3.ONE,basis)
	elif kind in [9,10]:
		_part("condenser" if kind == 9 else "reboiler")
		_part("gauge",Vector3(1.5,1.25,0.87))
	elif kind in [1,11]:
		_part("pipe_joint",Vector3.ONE*0.5)
	for port in layout.ports:
		var direction := -Vector3(port.normal)
		var basis := _along(direction)
		var length := 0.5 if kind in [1,11] else 0.3
		if kind in [0,4,7,8]: length = layout.nozzle_length
		var position: Vector3 = port.position
		_part("pipe_shaft",position,Vector3(length,1,1),basis)
		_part("port_flange",position,Vector3.ONE,basis)
		if kind not in [1,6,11]:
			var colour := 1 if port.circuit == 1 else 0
			if kind in [0,4]: colour = 3
			if kind in [7,8]: colour = [2,3,0,1,0][port.id]
			if kind in [9,10]: colour = (1 if kind == 9 else 0) if port.circuit == 1 else 3
			_part("pipe_shaft",position+direction*0.05,Vector3(0.065,1.12,1.12),basis,accents[colour])
	var result := ArrayMesh.new()
	for tool: SurfaceTool in surfaces.values():
		tool.commit(result)
	surfaces.clear()
	return result

func _vessel(layout: Dictionary) -> void:
	var kind: int = layout.kind
	var diameter: float = layout.diameter
	var height: float = layout.height
	var centre := diameter*0.5
	var radius := centre-0.13
	var origin := Vector3(centre,0,centre)
	_part("plinth",origin+Vector3(0,0.015,0),Vector3(radius,0.14,radius))
	var top := height-0.17
	var head := 0.0 if kind == 0 else 0.28
	_part("open_shell",origin+Vector3(0,0.15,0),Vector3(radius,top-head-0.15,radius))
	if head > 0:
		_part("dished_head",origin+Vector3(0,top-head,0),Vector3(radius,head,radius))
	if kind == 4:
		_part("plinth",origin+Vector3(0,height-0.20,0),Vector3(0.20,0.19,0.20))
	for y in [0.18,top-head-0.08]:
		_part("shell_band",origin+Vector3(0,y,0),Vector3(radius+0.02,0.065,radius+0.02))
	if kind in [7,8]:
		# Tray spacing is cosmetic; feed band follows the authoritative feed port.
		for y in range(1,int(height)-1):
			_part("shell_band",origin+Vector3(0,y+0.46,0),Vector3(radius+0.025,0.055,radius+0.025))
		var feed: Dictionary = layout.ports[0]
		_part("shell_band",origin+Vector3(0,feed.position.y-0.035,0),Vector3(radius+0.035,0.07,radius+0.035),Basis.IDENTITY,accents[2])
	# The access ladder is recessed into the visual shell to retain shared collision.
	for step in range(int(height)-1):
		_part("ladder_metre",Vector3(centre,0.20+step,centre-radius+0.015))
	_part("gauge",Vector3(centre+0.28,1.1,centre-radius+0.089),Vector3.ONE*0.75)

extends RefCounted
## Painted direction marks only; flow and controls remain authoritative Rust facts.
## Mesh-local +X points toward discharge. Four curved marks remain visible from
## different sides without occupying space outside the pump's collision envelope.

static func direction_basis(axis: int, reversed: bool) -> Basis:
	var directions := [Vector3.RIGHT,Vector3.UP,Vector3.BACK]
	var along: Vector3 = directions[clampi(axis,0,2)]
	var radial: Vector3 = directions[(clampi(axis,0,2)+1)%3]
	var other: Vector3 = along.cross(radial)
	if reversed:
		along = -along
		radial = -radial
	return Basis(along,radial,other)

static func create_mesh() -> ArrayMesh:
	var vertices := PackedVector3Array()
	var colors := PackedColorArray()
	for side in range(4):
		var angle: float = PI*0.25+side*PI*0.5
		# Small radial offsets make the outline and paint separate from the shell.
		# Diagonal placement also avoids the existing raised axial ridge.
		_shape(vertices,colors,angle,0.292,-0.202,0.032,0.212,0.15,0.36,Color(0.055,0.065,0.055))
		_shape(vertices,colors,angle,0.295,-0.19,0.04,0.195,0.105,0.29,Color(1.0,0.82,0.24))
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = vertices
	arrays[Mesh.ARRAY_COLOR] = colors
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays)
	var paint := StandardMaterial3D.new()
	paint.vertex_color_use_as_albedo = true
	paint.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	paint.cull_mode = BaseMaterial3D.CULL_DISABLED
	mesh.surface_set_material(0,paint)
	return mesh

static func _shape(vertices: PackedVector3Array, colors: PackedColorArray, angle: float, radius: float, tail: float, shoulder: float, tip: float, shaft_width: float, head_width: float, color: Color) -> void:
	_strip(vertices,colors,angle,radius,tail,shoulder,shaft_width,shaft_width,color)
	# Split the arrowhead longitudinally as well as around the circumference so
	# its triangles stay above the cylindrical shell instead of cutting into it.
	for segment in range(2):
		var t0: float = segment/2.0
		var t1: float = (segment+1)/2.0
		_strip(vertices,colors,angle,radius,lerpf(shoulder,tip,t0),lerpf(shoulder,tip,t1),head_width*(1.0-t0),head_width*(1.0-t1),color)

static func _strip(vertices: PackedVector3Array, colors: PackedColorArray, angle: float, radius: float, x0: float, x1: float, width0: float, width1: float, color: Color) -> void:
	for segment in range(4):
		var t0: float = -1.0+segment/2.0
		var t1: float = -1.0+(segment+1)/2.0
		var a := Vector3(x0,cos(angle+width0*t0)*radius,sin(angle+width0*t0)*radius)
		var b := Vector3(x1,cos(angle+width1*t0)*radius,sin(angle+width1*t0)*radius)
		var c := Vector3(x1,cos(angle+width1*t1)*radius,sin(angle+width1*t1)*radius)
		var d := Vector3(x0,cos(angle+width0*t1)*radius,sin(angle+width0*t1)*radius)
		for point in [a,b,c,a,c,d]:
			vertices.append(point)
			colors.append(color)

extends RefCounted
## An independent CPU coverage cut. Empty meshes are ready; missing meshes are not.
## Never expose both an ancestor and a descendant in the same frame.
var available: Dictionary
var near: Dictionary
var split: Dictionary
var center: Vector2i
var present: Dictionary = {}
var memo: Dictionary = {}
var active: Dictionary = {}

func configure(meshes: Dictionary, near_columns: Dictionary, branches: Dictionary, view_center: Vector2i) -> void:
	available = meshes
	near = near_columns
	split = branches
	center = view_center
	present.clear()
	memo.clear()
	active.clear()
	for key: Vector3i in available: _mark(key)
	for column: Vector2i in near:
		if int(near[column])==8: _mark(Vector3i(column.x*16,1,column.y*16))

func _mark(key: Vector3i) -> void:
	while true:
		present[key] = true
		if key.y>=32: break
		var size := key.y*32
		key = Vector3i(floori(float(key.x)/size)*size,key.y*2,floori(float(key.z)/size)*size)

static func children(key: Vector3i) -> Array[Vector3i]:
	if key.y<=1: return []
	var stride := key.y/2
	var size := stride*16
	return [Vector3i(key.x,stride,key.z),Vector3i(key.x+size,stride,key.z),Vector3i(key.x,stride,key.z+size),Vector3i(key.x+size,stride,key.z+size)]

func guard_intersection(key: Vector3i) -> bool:
	var low := (center-Vector2i(4,4))*16
	var high := (center+Vector2i(5,5))*16
	var size := key.y*16
	return key.y>1 and key.x<high.x and key.z<high.y and key.x+size>low.x and key.z+size>low.y

func near_ready(key: Vector3i) -> bool:
	return key.y==1 and int(near.get(Vector2i(key.x/16,key.z/16),0))==8

func covered(key: Vector3i) -> bool:
	if memo.has(key): return memo[key]
	var result := false
	if present.has(key):
		if near_ready(key) or (available.has(key) and not guard_intersection(key)):
			result = true
		elif key.y>1:
			result = true
			for child in children(key):
				if not covered(child):
					result = false
					break
	memo[key] = result
	return result

func visit(key: Vector3i) -> void:
	if not present.has(key) or near_ready(key): return
	if key.y==1:
		if available.has(key): active[key] = true
		return
	var descendants := children(key)
	var children_ready := true
	for child in descendants:
		if not covered(child): children_ready = false
	var forced := guard_intersection(key)
	if not forced and available.has(key) and (not split.has(key) or not children_ready):
		active[key] = true
		return
	# When the parent is missing, show independently completed children. This is
	# explicitly partial cold coverage, not a fabricated empty/ready parent.
	for child in descendants: visit(child)

extends SceneTree
## Deterministic presenter tests; no authority, transport, save, or real GPU run.
const View = preload("res://src/distant_view.gd")
const Cut = preload("res://src/distant_coverage.gd")
var failed := false

class FakeBridge:
	extends RefCounted
	var snapshot: Dictionary = {}
	var ready_key := Vector3i.ZERO
	var deferred_key := Vector3i.ZERO
	var priorities: Array[Vector3i] = []
	var taken: Array[Vector3i] = []
	func distant_state() -> Dictionary: return snapshot.duplicate(true)
	func distant_ready_key() -> Vector3i: return ready_key
	func prioritize_distant_mesh(key: Vector3i) -> bool:
		if ready_key.y==0 or key==ready_key: return false
		priorities.append(key)
		deferred_key = ready_key
		ready_key = key
		return true
	func take_distant_mesh() -> Dictionary:
		if ready_key.y==0: return {}
		var key := ready_key
		taken.append(key)
		ready_key = deferred_key
		deferred_key = Vector3i.ZERO
		return {"key":key,"generation":1,"epoch":1,"revision":1,"mesh_revision":1,"vertices_count":0,"geometry":{},"effective_stride":key.y,"degraded":false}

class FakeWorld:
	extends Node3D
	var bridge: FakeBridge
	var state := {"generation":1,"body_position":Vector3(0.5,100,0.5)}
	var terrain_uploaded: Dictionary = {}
	var terrain_nodes: Dictionary = {}
	func _mesh(_geometry: Dictionary) -> ArrayMesh: return ArrayMesh.new()

func _initialize() -> void: call_deferred("run")

func check(value: bool, message: String) -> void:
	if not value:
		failed = true
		push_error(message)

func _fixture(data: Dictionary) -> Node3D:
	var host := FakeWorld.new()
	host.bridge = FakeBridge.new()
	host.bridge.snapshot = data
	root.add_child(host)
	var view = View.new()
	host.add_child(view)
	view.configure(host)
	# These tests never apply real environment/camera resources.
	view.horizon_applied = true
	view._refresh_state()
	return host

func _add(view: Node3D, key: Vector3i, stamp: int = 1) -> void:
	var node := MeshInstance3D.new()
	node.mesh = ArrayMesh.new()
	view.add_child(node)
	view.nodes[key] = node
	view.versions[key] = {"vertices":0,"mesh_revision":stamp,"revision":stamp,"used":0,"degraded":false,"stride":key.y,"effective_stride":key.y}

func _entry(key: Vector3i, stamp: int = 1) -> Dictionary:
	return {"key":key,"mesh_revision":stamp,"revision":stamp,"desired":true,"leaf":true}

func run() -> void:
	var fine := Vector3i(0,1,0)
	var coarse := Vector3i(1024,16,1024)
	var host := _fixture({"epoch":1,"center":Vector2i.ZERO,"wanted":[fine,coarse],"split":[],"roots":[Vector3i(0,32,0),Vector3i(1024,32,1024),Vector3i(1536,32,1024)],"tiles":[_entry(fine),_entry(coarse)]})
	var view: Node3D = host.get_node("DistantTerrain")
	# A valid partial old cut spans two roots. Every occupied GPU slot is active,
	# so ordinary inactive-LRU eviction cannot make room for the new fine tile.
	for index in range(View.MAX_NODES):
		_add(view,Vector3i(1024+(index%64)*16,1,1024+(index/64)*16))
	view._update_visibility()
	check(view.nodes.size()==View.MAX_NODES and view.active.size()==View.MAX_NODES,"fixture fills every GPU slot with active old coverage")
	var before_area: float = view.coverage_ratio
	host.bridge.ready_key = fine
	check(not view.upload_one(),"blocked fine result defers without deleting coverage")
	check(host.bridge.priorities==[coarse] and host.bridge.taken.is_empty(),"full queue prioritizes the available coarse leaf without consuming the fine result")
	check(view.nodes.size()==View.MAX_NODES and is_equal_approx(view.coverage_ratio,before_area),"priority change keeps existing coverage intact")
	check(view.upload_one(),"coarse leaf uploads despite full GPU capacity")
	check(view.nodes.size()==View.MAX_NODES-256+1,"coarse replacement atomically releases its obsolete fine descendants")
	check(view.active.has(coarse) and is_equal_approx(view.coverage_ratio,before_area),"coarsening preserves the exact covered area")
	check(view.upload_one() and view.active.has(fine),"deferred fine tile uploads after coarsening frees capacity")
	check(host.bridge.taken==[coarse,fine],"single ready slot makes forward progress in coarse then fine order")
	view.horizon_applied = false
	host.queue_free()
	await process_frame
	await _old_versions()
	print("GODOT_DISTANT_HANDOFF_RESULT=","FAIL" if failed else "PASS")
	quit(1 if failed else 0)

func _old_versions() -> void:
	var parent := Vector3i(-1024,16,-1024)
	var unknown := Vector3i(-768,16,-1024)
	var children := Cut.children(parent)
	var host := _fixture({"epoch":1,"center":Vector2i.ZERO,"wanted":[parent,unknown],"split":[],"roots":[Vector3i(-1024,32,-1024)],"tiles":[_entry(parent,1)]})
	var view: Node3D = host.get_node("DistantTerrain")
	_add(view,parent,1)
	_add(view,unknown,1)
	for child in children:
		_add(view,child,2)
		view.active[child] = true
	view.visibility_dirty = false
	host.bridge.snapshot.tiles = [_entry(parent,2)]
	view._refresh_state()
	check(view.visibility_dirty,"new raw revision dirties coverage without changing the view epoch")
	view._update_visibility()
	check(not view.active.has(parent),"inactive known-outdated parent cannot replace newer displayed children")
	for child in children: check(view.active.has(child),"newer child coverage remains active while parent rebuilds")
	check(view.active.has(unknown),"GPU geometry with no raw metadata can retain historical coverage")
	view.active = {parent:true,unknown:true}
	view._update_visibility()
	check(view.active.has(parent),"already-active outdated parent holds coverage until replacement")
	view.active.clear()
	host.bridge.snapshot.tiles = []
	view._refresh_state()
	view._update_visibility()
	check(not view.active.has(parent),"raw eviction cannot erase a known newer revision and revive an obsolete parent")
	check(view.active.has(unknown),"raw eviction does not discard unrelated valid historical coverage")
	view.versions[parent].mesh_revision = 3
	view.active.clear()
	view._update_visibility()
	check(view.active.has(parent),"fresh GPU result stays eligible when periodic raw metadata lags behind it")
	view.horizon_applied = false
	view.clear()
	check(view.known_mesh_revisions.is_empty(),"session cleanup releases revision watermarks")
	host.queue_free()
	await process_frame

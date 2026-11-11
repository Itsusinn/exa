extends Node3D
## Main-thread GPU ownership; coverage is independent of mesh arrival order.
const WorldLighting = preload("res://src/world_lighting.gd")
const Coverage = preload("res://src/distant_coverage.gd")
const MAX_NODES := 1536
const MAX_VERTICES := MAX_NODES*24576
const MAX_FALLBACK_SECTIONS := 96
const MAX_FALLBACK_VERTICES := 300000
const FALLBACK_MSEC := 3000
var world: Node3D
var nodes: Dictionary = {}
var versions: Dictionary = {}
var fallback: Dictionary = {}
var fallback_root: Node3D
var near_counts: Dictionary = {}
var wanted: Dictionary = {}
var split: Dictionary = {}
var root_keys: Dictionary = {}
var active: Dictionary = {}
var coverage = Coverage.new()
var enabled := true
var snapshot: Dictionary = {}
var known_mesh_revisions: Dictionary = {}
var uploads_last_frame := 0
var upload_usec_last_frame := 0
var vertices := 0
var fallback_vertices := 0
var fallback_expired := 0
var peak_vertices := 0
var peak_fallback_sections := 0
var stale_results := 0
var horizon_applied := false
var coverage_ratio := 0.0
var roots_covered := 0
var visibility_dirty := true
var polled_at := -1000

func configure(host: Node3D) -> void:
	world = host
	name = "DistantTerrain"
	fallback_root = Node3D.new()
	fallback_root.name = "PendingNearReplacement"
	add_child(fallback_root)

func set_enabled(value: bool) -> void:
	enabled = value
	visibility_dirty = true
	_update_visibility()

func clear() -> void:
	for node in nodes.values():
		remove_child(node)
		node.queue_free()
	for entry in fallback.values():
		fallback_root.remove_child(entry.node)
		entry.node.queue_free()
	for data in [nodes,versions,fallback,near_counts,wanted,split,root_keys,active,snapshot,known_mesh_revisions]: data.clear()
	vertices = 0
	fallback_vertices = 0
	uploads_last_frame = 0
	upload_usec_last_frame = 0
	fallback_expired = 0
	peak_vertices = 0
	peak_fallback_sections = 0
	stale_results = 0
	roots_covered = 0
	coverage_ratio = 0.0
	visibility_dirty = true
	polled_at = -1000
	if horizon_applied:
		WorldLighting.apply_atmosphere(world.environment,true)
		world.camera.far = 150.0
	horizon_applied = false

func _refresh_state() -> void:
	var previous_epoch: int = snapshot.get("epoch",0)
	snapshot = world.bridge.distant_state()
	polled_at = Time.get_ticks_msec()
	var next_revisions: Dictionary = {}
	# Raw cache eviction must not erase a newer fact already known about a GPU
	# mesh. Keep watermarks only for resident GPU nodes or current raw entries.
	for key: Vector3i in nodes:
		if known_mesh_revisions.has(key): next_revisions[key] = known_mesh_revisions[key]
	for entry in snapshot.get("tiles",[]): next_revisions[entry.key] = maxi(int(next_revisions.get(entry.key,-1)),int(entry.mesh_revision))
	if next_revisions!=known_mesh_revisions:
		known_mesh_revisions = next_revisions
		visibility_dirty = true
	if int(snapshot.get("epoch",0))!=previous_epoch:
		wanted.clear()
		split.clear()
		root_keys.clear()
		for key: Vector3i in snapshot.get("wanted",[]): wanted[key] = true
		for key: Vector3i in snapshot.get("split",[]): split[key] = true
		for key: Vector3i in snapshot.get("roots",[]): root_keys[key] = true
		for key: Vector3i in nodes.keys():
			if not root_keys.has(_root(key)): _remove_tile(key)
		visibility_dirty = true
	if snapshot.get("epoch",0)>0 and not horizon_applied:
		WorldLighting.apply_distance_horizon(world.environment)
		world.camera.far = 2048.0
		horizon_applied = true

static func _root(key: Vector3i) -> Vector3i:
	return Vector3i(floori(float(key.x)/512.0)*512,32,floori(float(key.z)/512.0)*512)

func sync() -> void:
	uploads_last_frame = 0
	upload_usec_last_frame = 0
	var body: Vector3 = world.state.get("body_position",Vector3.ZERO)
	var center := Vector2i(floori(body.x/16.0),floori(body.z/16.0))
	# Cache metadata between polls. Worker results still upload each frame.
	if Time.get_ticks_msec()-polled_at>=100 or center!=snapshot.get("center",Vector2i.ZERO): _refresh_state()
	var next_near: Dictionary = {}
	for origin: Vector3i in world.terrain_uploaded:
		var column := Vector2i(origin.x/16,origin.z/16)
		next_near[column] = int(next_near.get(column,0))+1
	if next_near!=near_counts:
		near_counts = next_near
		visibility_dirty = true
	if visibility_dirty: _update_visibility()
	_expire_fallback()

func _update_visibility() -> void:
	var eligible: Dictionary = {}
	for key: Vector3i in nodes:
		var outdated: bool = known_mesh_revisions.has(key) and int(known_mesh_revisions[key])>int(versions[key].mesh_revision)
		# An already displayed old mesh can hold coverage while its replacement
		# builds. Never reactivate an inactive mesh known to predate received facts.
		# Missing raw metadata alone does not invalidate retained GPU handoff data.
		if not outdated or active.has(key): eligible[key] = nodes[key]
	coverage.configure(eligible,near_counts,split,snapshot.get("center",Vector2i.ZERO))
	roots_covered = 0
	for key: Vector3i in root_keys:
		coverage.visit(key)
		if coverage.covered(key): roots_covered += 1
	active = coverage.active.duplicate()
	var area := 0.0
	for key: Vector3i in nodes:
		nodes[key].visible = enabled and active.has(key)
		if active.has(key):
			area += float(key.y*16)*float(key.y*16)
			versions[key]["used"] = Time.get_ticks_msec()
	for column: Vector2i in near_counts:
		if int(near_counts[column])==8 and root_keys.has(_root(Vector3i(column.x*16,1,column.y*16))): area += 256.0
	coverage_ratio = clampf(area/(root_keys.size()*512.0*512.0),0.0,1.0) if not root_keys.is_empty() else 0.0
	for origin: Vector3i in world.terrain_nodes:
		var key := Vector3i(origin.x,1,origin.z)
		world.terrain_nodes[origin].visible = not enabled or not active.has(key)
	if fallback_root!=null: fallback_root.visible = enabled
	visibility_dirty = false

func _expire_fallback() -> void:
	var now := Time.get_ticks_msec()
	for origin: Vector3i in fallback.keys():
		var key := Vector3i(origin.x,1,origin.z)
		var replaced: bool = active.has(key) or int(near_counts.get(Vector2i(origin.x/16,origin.z/16),0))==8
		var expired: bool = now-int(fallback[origin].created)>FALLBACK_MSEC
		if replaced or expired:
			if expired and not replaced: fallback_expired += 1
			_remove_fallback(origin)

func section_uploaded(origin: Vector3i) -> void:
	if fallback.has(origin): _remove_fallback(origin)
	visibility_dirty = true

func retain_unloaded(origin: Vector3i, node: MeshInstance3D) -> bool:
	var column := Vector2i(origin.x/16,origin.z/16)
	var body: Vector3 = world.state.get("body_position",Vector3.ZERO)
	var center := Vector2i(floori(body.x/16.0),floori(body.z/16.0))
	if nodes.has(Vector3i(origin.x,1,origin.z)) or maxi(absi(column.x-center.x),absi(column.y-center.y))>4: return false
	var count := 0
	for surface in range(node.mesh.get_surface_count()): count += node.mesh.surface_get_array_len(surface)
	if fallback.size()>=MAX_FALLBACK_SECTIONS or fallback_vertices+count>MAX_FALLBACK_VERTICES: return false
	if fallback.has(origin): _remove_fallback(origin)
	node.get_parent().remove_child(node)
	fallback_root.add_child(node)
	fallback[origin] = {"node":node,"vertices":count,"created":Time.get_ticks_msec()}
	fallback_vertices += count
	peak_fallback_sections = maxi(peak_fallback_sections,fallback.size())
	return true

func _make_room() -> bool:
	if nodes.size()<MAX_NODES: return true
	var candidate := Vector3i.ZERO
	var oldest := 9223372036854775807
	for key: Vector3i in nodes:
		if not wanted.has(key) and not active.has(key) and int(versions[key].used)<oldest:
			candidate = key
			oldest = int(versions[key].used)
	if candidate.y==0: return false
	_remove_tile(candidate)
	return true

func upload_one() -> bool:
	# Keep the worker result queued if all slots are needed for current coverage.
	# Replacing an existing tile needs no new node, even at the hard limit.
	var coarsen: Array[Vector3i] = []
	if nodes.size()>=MAX_NODES:
		_refresh_state()
		if visibility_dirty: _update_visibility()
		var ready_key: Vector3i = world.bridge.distant_ready_key()
		if ready_key.y==0: return false
		if not nodes.has(ready_key) and not _make_room():
			coarsen = _coarsen_candidates(ready_key)
			if coarsen.is_empty():
				_prioritize_coarsening()
				return false
	var began := Time.get_ticks_usec()
	var result: Dictionary = world.bridge.take_distant_mesh()
	if result.is_empty(): return false
	if result.epoch!=snapshot.get("epoch",0): _refresh_state()
	var key: Vector3i = result.key
	if result.generation!=world.state.generation or not wanted.has(key):
		stale_results += 1
		return false
	var count: int = result.vertices_count
	var cap := 24576 if key.y==1 else 4096
	var old_count: int = versions.get(key,{}).get("vertices",0)
	var released := 0
	for old_key in coarsen: released += int(versions[old_key].vertices)
	if count<0 or count>cap or vertices-old_count-released+count>MAX_VERTICES:
		push_error("Distant worker exceeded the bounded geometry contract")
		return false
	var mesh: ArrayMesh = world._mesh(result.geometry)
	# A completed desired coarse leaf can replace its obsolete children in this
	# same frame. Prepare its mesh first; never remove coverage just to await work.
	for old_key in coarsen: _remove_tile(old_key)
	var node: MeshInstance3D = nodes.get(key)
	if not is_instance_valid(node):
		node = MeshInstance3D.new()
		node.name = "Distant_%d_%d_%d" % [key.x,key.y,key.z]
		add_child(node)
		nodes[key] = node
	node.mesh = mesh
	node.gi_mode = GeometryInstance3D.GI_MODE_STATIC if key.y==1 else GeometryInstance3D.GI_MODE_DISABLED
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_ON if key.y==1 else GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	vertices += count-old_count
	peak_vertices = maxi(peak_vertices,vertices)
	versions[key] = {"vertices":count,"stride":key.y,"revision":result.revision,"mesh_revision":result.mesh_revision,"effective_stride":result.effective_stride,"degraded":result.degraded,"used":Time.get_ticks_msec()}
	uploads_last_frame += 1
	upload_usec_last_frame += Time.get_ticks_usec()-began
	visibility_dirty = true
	_update_visibility()
	_expire_fallback()
	return true

func _coarsen_candidates(key: Vector3i) -> Array[Vector3i]:
	var result: Array[Vector3i] = []
	if not wanted.has(key) or split.has(key) or coverage.guard_intersection(key): return result
	var high := Vector2i(key.x,key.z)+Vector2i.ONE*(key.y*16)
	for child: Vector3i in nodes:
		if not wanted.has(child) and child.y<key.y and child.x>=key.x and child.z>=key.z and child.x+child.y*16<=high.x and child.z+child.y*16<=high.y:
			result.append(child)
	return result

func _prioritize_coarsening() -> bool:
	# A new fine result may fill the single ready slot while every GPU slot holds
	# useful old coverage. Let a received coarse leaf replace obsolete children
	# first; the worker keeps the deferred fine tile dirty for a later retry.
	var candidates: Dictionary = {}
	for entry in snapshot.get("tiles",[]):
		var key: Vector3i = entry.key
		if entry.get("desired",false) and entry.get("leaf",false) and wanted.has(key) and not split.has(key) and not nodes.has(key) and not coverage.guard_intersection(key): candidates[key] = true
	# Walk at most five ancestors per obsolete node, rather than scanning every
	# GPU node separately for every possible coarse payload at the hard limit.
	for child: Vector3i in nodes:
		if wanted.has(child): continue
		var stride := child.y*2
		while stride<=32:
			var size := stride*16
			var parent := Vector3i(floori(float(child.x)/size)*size,stride,floori(float(child.z)/size)*size)
			if candidates.has(parent):
				candidates.erase(parent)
				if world.bridge.prioritize_distant_mesh(parent): return true
			stride *= 2
	return false

func _remove_tile(key: Vector3i) -> void:
	vertices -= int(versions.get(key,{}).get("vertices",0))
	var node: MeshInstance3D = nodes[key]
	remove_child(node)
	node.queue_free()
	nodes.erase(key)
	versions.erase(key)
	visibility_dirty = true

func _remove_fallback(origin: Vector3i) -> void:
	var entry: Dictionary = fallback[origin]
	fallback_vertices -= int(entry.vertices)
	fallback_root.remove_child(entry.node)
	entry.node.queue_free()
	fallback.erase(origin)

func stats() -> Dictionary:
	var outdated := 0
	var desired_received := 0
	for entry in snapshot.get("tiles",[]):
		if not entry.get("desired",false): continue
		desired_received += 1
		var saved: Dictionary = versions.get(entry.key,{})
		if saved.get("mesh_revision",-1)!=entry.mesh_revision: outdated += 1
	var degraded := 0
	var desired_uploaded := 0
	for key in nodes:
		if versions[key].degraded: degraded += 1
		if wanted.has(key): desired_uploaded += 1
	var guard_ready := true
	var center: Vector2i = snapshot.get("center",Vector2i.ZERO)
	for z in range(center.y-4,center.y+5):
		for x in range(center.x-4,center.x+5):
			if x< -2048 or x>=2048 or z< -2048 or z>=2048: continue
			if int(near_counts.get(Vector2i(x,z),0))!=8 and not nodes.has(Vector3i(x*16,1,z*16)): guard_ready = false
	return {"nodes":nodes.size(),"max_nodes":MAX_NODES,"visible_nodes":active.size(),"active_keys":active.keys(),"vertices":vertices,"triangles":vertices/3,"peak_vertices":peak_vertices,"max_vertices":MAX_VERTICES,"degraded_tiles":degraded,"fallback_nodes":fallback.size(),"fallback_vertices":fallback_vertices,"peak_fallback_sections":peak_fallback_sections,"fallback_expired":fallback_expired,"uploads_last_frame":uploads_last_frame,"upload_usec_last_frame":upload_usec_last_frame,"stale_results":stale_results,"outdated":outdated,"wanted_nodes":wanted.size(),"wanted_received":desired_received,"wanted_uploaded":desired_uploaded,"roots_total":root_keys.size(),"roots_covered":roots_covered,"coverage_ratio":coverage_ratio,"finest_guard_ready":guard_ready,"stream":snapshot}

extends "res://tests/lighting.gd"
## Real streamed ecology, independent render cache and bounded handoff review.
## Root runs this in a serial graphical session, never against a real save.
const MAX_RAW_BYTES := 48 * 1024 * 1024
const COLD_TIMEOUT_SECONDS := 1800.0
const TOTAL_TIMEOUT_SECONDS := 2400.0
var main_cpu: Array[float] = []
var upload_cpu: Array[float] = []
var max_pending := 0
var max_raw_bytes := 0
var max_vertices := 0
var max_fallback := 0
var upload_soft_overruns := 0
var motion_samples: Array = []
var fixture_sites: Array = []
var progress_samples: Array = []
var frame_count := 0
var test_started_msec := 0
var next_progress_msec := 0
var coverage_frame := -1
var covered_keys: Dictionary = {}
var view_started_msec := 0
var first_horizon_coverage_ms := -1
var quick_review := false

func run() -> void:
	quick_review = OS.get_environment("EXA_HORIZON_QUICK")=="1"
	if DisplayServer.get_name()=="headless" or RenderingServer.get_current_rendering_method()!="forward_plus":
		push_error("Distant review requires a real Forward+ viewport")
		quit(1)
		return
	test_started_msec = Time.get_ticks_msec()
	next_progress_msec = test_started_msec+10000
	create_timer(TOTAL_TIMEOUT_SECONDS).timeout.connect(func():
		_write_progress("watchdog")
		push_error("Distant review exceeded its bounded 2400 second watchdog")
		quit(1))
	var data = JSON.parse_string(FileAccess.get_file_as_string("res://../target/ecology-sites.json"))
	if not data is Array:
		push_error("Generate the real Rust ecology-sites fixture before distant review")
		quit(1)
		return
	fixture_sites = data
	output = preload("res://src/app_config.gd").environment("VISUAL_PATH")
	if output.is_empty(): output = ProjectSettings.globalize_path("res://../target/distant-review")
	if DirAccess.make_dir_recursive_absolute(output)!=OK:
		push_error("Cannot create distant evidence directory")
		quit(1)
		return
	var save_dir := ProjectSettings.globalize_path("res://../target/distant-world-%d" % Time.get_ticks_usec())
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
	report = {"engine":Engine.get_version_info().string,"adapter":RenderingServer.get_video_adapter_name(),"renderer":RenderingServer.get_current_rendering_method(),"resolution":[1280,800],"samples_per_view":SAMPLE_FRAMES,"views":{},"differences":{},"sites":[],"save_directory":save_dir,
		"evidence_note":"Far-off changes display only. Fog-free comparisons prove geometry beyond the near cache, not physical accuracy or perfect LOD transitions. Image differences are presence, not quality scores.",
		"timing_note":"Static viewport GPU/render timings exclude gameplay and generation. Main/upload timings are separate. Motion PNG readback and compression disturb frame intervals, so the 60-frame image sequence is visual evidence rather than normal-play FPS.",
		"budget_note":"Near and far uploads share a 2-mesh/2000us soft scheduling budget. A single mesh can exceed the soft deadline. Raw byte counts exclude metadata and worker-held references; they are not total process RAM."}
	viewport_rid = root.get_viewport_rid()
	report["full_horizon_required"] = not quick_review
	RenderingServer.viewport_set_measure_render_time(viewport_rid,true)
	scene._singleplayer()
	var started := await _settle(func(): return scene.state.get("ready",false),30.0)
	check(started,"isolated authority starts")
	if not started:
		await _finish()
		return
	scene._focus_lost()
	scene.bridge.send_chat("/fly on")
	check(await _settle(func(): return scene.state.get("flying",false),10.0),"authority enables flight")
	await create_timer(0.6).timeout
	for biome in ["Forest","Desert"]:
		var site := _site(biome)
		var point := Vector3(site.x,site.y,site.z)
		var cold_start := Time.get_ticks_msec()
		view_started_msec = cold_start
		first_horizon_coverage_ms = -1
		scene.bridge.send_chat("/tp %s %s %s" % [point.x,point.y,point.z])
		var full_horizon: bool = biome=="Forest" and not quick_review
		var ready := await _settle(func(): return scene.state.body_position.distance_to(point)<0.1 and (_complete() if full_horizon else _guard_ready()),COLD_TIMEOUT_SECONDS if full_horizon else 90.0)
		check(ready,("complete 2km GPU coverage: " if full_horizon else "near and fine guard ready; outer horizon may be partial: ")+biome)
		if not ready:
			await _finish()
			return
		scene.hud.hide()
		var entry := _stats()
		entry["biome"] = biome
		entry["cold_ready_ms"] = Time.get_ticks_msec()-cold_start
		entry["first_horizon_coverage_ms"] = first_horizon_coverage_ms
		entry["required_full_horizon"] = full_horizon
		entry["complete_horizon"] = _complete()
		report.sites.append(entry)
		check(scene.distant.stats().visible_nodes>0,"real distant geometry is visible outside the near world: "+biome)
		check(scene.terrain_uploaded.size()==392,"fine render upload accounting includes empty sections")
		_validate_coverage(full_horizon)
		var eye := point+Vector3(0,1.58,0)
		scene.camera.position = eye
		scene.camera.look_at(eye+Vector3(0.35,-0.28,-1).normalized()*20.0)
		await _compare(biome+"-atmosphere")
		var fog_enabled: bool = scene.environment.fog_enabled
		scene.environment.fog_enabled = false
		await _compare(biome+"-no-fog")
		if full_horizon:
			# A diagnostic aerial camera exposes the long horizon beyond foreground
			# trees; it does not move the authoritative player or expand collision.
			var aerial := point+Vector3(0,110,0)
			scene.camera.position = aerial
			scene.camera.look_at(aerial+Vector3(0.35,-0.035,-1).normalized()*1000.0)
			await _compare(biome+"-2km-aerial-no-fog")
			_record_horizon_geometry()
			scene.camera.position = eye
			scene.camera.look_at(eye+Vector3(0.35,-0.28,-1).normalized()*20.0)
		scene.environment.fog_enabled = fog_enabled
		if biome=="Forest": await _cross_boundary_sequence()
	# Queue new distant work, then change views before it can all be uploaded.
	var forest := _site("Forest")
	var desert := _site("Desert")
	var previous_epoch: int = scene.bridge.distant_state().epoch
	for site in [forest,desert,forest]:
		await create_timer(0.6).timeout
		var point := Vector3(site.x,site.y,site.z)
		scene.bridge.send_chat("/tp %s %s %s" % [point.x,point.y,point.z])
		check(await _settle(func(): return scene.state.body_position.distance_to(point)<0.1,15.0),"rapid teleport is acknowledged")
	check(await _settle(_guard_ready,90.0),"latest epoch restores fine guard after rapid A/B/A teleports")
	check(int(scene.bridge.distant_state().epoch)>previous_epoch,"rapid teleport advances the distant view epoch")
	_validate_coverage(false)
	report["rapid_teleport"] = _stats()
	scene._leave_game(false)
	check(await _settle(func(): return scene.pending_action.is_empty(),20.0),"isolated world saves and closes")
	check(await _settle(_worker_idle,5.0),"cancelled distant worker acknowledges shutdown")
	_validate_empty()
	var old_generation: int = scene.state.generation
	scene._singleplayer()
	check(await _settle(func(): return scene.state.get("ready",false) and _guard_ready(),90.0),"saved world reconnects and rebuilds fine guard without waiting for the full horizon")
	check(int(scene.state.generation)>old_generation,"reconnect uses a new session generation")
	_validate_coverage(false)
	report["reconnected"] = _stats()
	await _finish()

func _site(biome: String) -> Dictionary:
	for site in fixture_sites:
		if site.biome==biome: return site
	check(false,"fixture contains biome: "+biome)
	return {"x":0.5,"y":110,"z":0.5}

func _frame(elapsed: float = 0.0) -> void:
	frame_count += 1
	var start := Time.get_ticks_usec()
	scene._process(elapsed)
	main_cpu.append(float(Time.get_ticks_usec()-start)/1000.0)
	upload_cpu.append(float(scene.terrain_upload_usec_last_frame)/1000.0)
	if int(scene.terrain_upload_usec_last_frame)>2000: upload_soft_overruns += 1
	var stats: Dictionary = scene.distant.stats()
	var stream: Dictionary = scene.bridge.distant_state()
	max_pending = maxi(max_pending,int(stream.get("pending",0)))
	max_raw_bytes = maxi(max_raw_bytes,int(stream.get("raw_bytes",0)))
	max_vertices = maxi(max_vertices,int(stats.vertices))
	max_fallback = maxi(max_fallback,int(stats.fallback_nodes))
	check(int(scene.state.get("loaded_sections",0))<=392,"near collision Grid remains bounded")
	check(stream.get("tiles",[]).size()<=1536 and int(stream.get("raw_bytes",0))<=MAX_RAW_BYTES,"independent distant raw cache remains bounded")
	check(int(stream.get("wanted_count",0))<=1024 and int(stream.get("pending",0))<=1536 and int(stream.get("in_flight",0))<=1 and int(stream.get("ready",0))<=1,"distant worker queues remain bounded")
	check(int(stats.nodes)<=1536 and int(stats.vertices)<=int(stats.max_vertices),"distant GPU geometry remains within its transition hard cap")
	check(int(stats.fallback_nodes)<=96 and int(stats.fallback_vertices)<=300000,"temporary handoff geometry remains bounded")
	check(int(scene.terrain_total_updates_last_frame)<=2,"near and far share the two-mesh upload limit")
	check(int(scene.terrain_updates_last_frame)+int(stats.uploads_last_frame)==int(scene.terrain_total_updates_last_frame),"near/far upload accounting agrees")
	_validate_handoff()
	if frame_count%30==0: _validate_coverage(false)
	if Time.get_ticks_msec()>=next_progress_msec:
		_write_progress("loading")
		next_progress_msec = Time.get_ticks_msec()+10000
	await process_frame

func _validate_handoff() -> void:
	# Compare actual node visibility, not just network arrival. A partial near
	# column must not overlap a visible far replacement before all eight uploads.
	for origin: Vector3i in scene.terrain_nodes:
		var key := Vector3i(origin.x,1,origin.z)
		var covered: bool = scene.distant.enabled and scene.distant.nodes.has(key) and scene.distant.nodes[key].visible
		check(scene.terrain_nodes[origin].visible!=covered,"near partial visibility is atomic with its far replacement")

func _settle(done: Callable, seconds: float) -> bool:
	var deadline := Time.get_ticks_msec()+int(seconds*1000.0)
	var next_check := 0
	while Time.get_ticks_msec()<deadline:
		await _frame()
		# Full coverage checks cross the bridge for current resident near sections.
		# Poll at 10Hz during the long exact-generation wait, while presentation and
		# per-frame budget/handoff checks continue at the actual render cadence.
		if Time.get_ticks_msec()>=next_check:
			if done.call(): return true
			next_check = Time.get_ticks_msec()+100
	print("DISTANT_SETTLE_TIMEOUT=",JSON.stringify(_stats()))
	return false

func _complete() -> bool:
	var stream: Dictionary = scene.bridge.distant_state()
	var stats: Dictionary = scene.distant.stats()
	if not _guard_ready() or stream.get("roots",[]).is_empty(): return false
	for key: Vector3i in stream.roots:
		if not _covered(key): return false
	if first_horizon_coverage_ms<0: first_horizon_coverage_ms = Time.get_ticks_msec()-view_started_msec
	return int(stats.get("wanted_nodes",0))>0 and int(stats.get("wanted_received",-1))==int(stats.wanted_nodes) and int(stats.get("wanted_uploaded",-1))==int(stats.wanted_nodes) and int(stats.get("outdated",-1))==0 and int(stream.get("pending",-1))==0 and int(stream.get("in_flight",-1))==0 and int(stream.get("ready",-1))==0

func _near_complete(column: Vector2i) -> bool:
	for y in range(0,128,16):
		var origin := Vector3i(column.x*16,y,column.y*16)
		if not scene.bridge.has_chunk(origin) or not scene.terrain_uploaded.has(origin): return false
	return true

func _children(key: Vector3i) -> Array[Vector3i]:
	var stride: int = key.y/2
	return [Vector3i(key.x,stride,key.z),Vector3i(key.x+stride*16,stride,key.z),Vector3i(key.x,stride,key.z+stride*16),Vector3i(key.x+stride*16,stride,key.z+stride*16)]

func _covered(key: Vector3i) -> bool:
	_refresh_coverage()
	return covered_keys.has(key)

func _refresh_coverage() -> void:
	if coverage_frame==frame_count: return
	coverage_frame = frame_count
	covered_keys.clear()
	for key: Vector3i in scene.distant.nodes:
		if scene.distant.nodes[key].visible: covered_keys[key] = true
	var near_columns: Dictionary = {}
	for origin: Vector3i in scene.terrain_uploaded:
		near_columns[Vector2i(origin.x/16,origin.z/16)] = true
	for column: Vector2i in near_columns:
		if _near_complete(column): covered_keys[Vector3i(column.x*16,1,column.y*16)] = true
	# Merge only four complete child areas. Missing data is never treated as air.
	for stride in [1,2,4,8,16]:
		var parents: Dictionary = {}
		for key: Vector3i in covered_keys:
			if key.y!=stride: continue
			var parent_stride: int = stride*2
			var span: int = parent_stride*16
			parents[Vector3i(floori(float(key.x)/span)*span,parent_stride,floori(float(key.z)/span)*span)] = true
		for parent: Vector3i in parents:
			var complete := true
			for child in _children(parent):
				if not covered_keys.has(child): complete = false
			if complete: covered_keys[parent] = true

func _guard_ready() -> bool:
	if not scene.state.get("ready",false) or int(scene.state.get("loaded_sections",0))!=392 or not scene.terrain_pending.is_empty(): return false
	var stream: Dictionary = scene.bridge.distant_state()
	if int(stream.get("epoch",0))==0: return false
	var center: Vector2i = stream.get("center",Vector2i.ZERO)
	for dz in range(-4,5):
		for dx in range(-4,5):
			if not _covered(Vector3i((center.x+dx)*16,1,(center.y+dz)*16)): return false
	return true

func _compare(name: String) -> void:
	var exposure: float = scene.environment.tonemap_exposure
	var sky: Sky = scene.environment.sky
	for enabled in [false,true]:
		scene.distant.set_enabled(enabled)
		check(scene.environment.tonemap_mode==Environment.TONE_MAPPER_ACES and is_equal_approx(exposure,1.0) and scene.environment.sky==sky,"far display comparison preserves lighting/exposure")
		check(root.msaa_3d==Viewport.MSAA_4X,"far display comparison preserves MSAA4")
		await _warmup()
		var suffix := name+("-on" if enabled else "-off")
		report.views[suffix] = await _measure()
		var camera_position: Vector3 = scene.camera.position
		report.views[suffix]["camera_position"] = [camera_position.x,camera_position.y,camera_position.z]
		report.views[suffix]["camera_far"] = scene.camera.far
		report.views[suffix]["fog_enabled"] = scene.environment.fog_enabled
		await _capture(suffix)
	report.differences[name] = _difference(name+"-off",name+"-on")
	check(report.differences[name].get("changed_samples",0)>=16,"far display adds a visible effect: "+name)

func _record_horizon_geometry() -> void:
	# One diagnostic pass after timing/screenshots. Count actual transformed mesh
	# vertices, including triangle duplicates, rather than tile-key extents. Being
	# in the frustum does not prove freedom from terrain occlusion; PNGs retain
	# that separate visual evidence.
	check(scene.distant.enabled and not scene.environment.fog_enabled,"aerial geometry evidence uses enabled far terrain without fog")
	check(scene.camera.has_method("is_position_in_frustum"),"Camera3D exposes the native frustum query")
	if not scene.camera.has_method("is_position_in_frustum"): return
	var camera_position: Vector3 = scene.camera.global_position
	var horizontal: Dictionary = {"1024":0,"1536":0,"1900":0}
	var spatial: Dictionary = {"1024":0,"1536":0,"1900":0}
	var visible_nonempty := 0
	var inspected_vertices := 0
	var frustum_vertices := 0
	var far_tiles: Dictionary = {}
	var max_horizontal := 0.0
	var max_spatial := 0.0
	for key: Vector3i in scene.distant.nodes:
		var node: MeshInstance3D = scene.distant.nodes[key]
		if not node.visible or not node.is_visible_in_tree() or not node.mesh is ArrayMesh: continue
		var mesh: ArrayMesh = node.mesh
		var nonempty := false
		for surface in range(mesh.get_surface_count()):
			var arrays: Array = mesh.surface_get_arrays(surface)
			var points: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
			nonempty = nonempty or not points.is_empty()
			for local_position: Vector3 in points:
				inspected_vertices += 1
				var world_position: Vector3 = node.global_transform*local_position
				var offset: Vector3 = world_position-camera_position
				var distance: float = offset.length()
				if distance>2048.0 or not scene.camera.is_position_in_frustum(world_position): continue
				frustum_vertices += 1
				var flat_distance: float = Vector2(offset.x,offset.z).length()
				max_horizontal = maxf(max_horizontal,flat_distance)
				max_spatial = maxf(max_spatial,distance)
				for threshold in [1024,1536,1900]:
					if flat_distance>threshold: horizontal[str(threshold)] += 1
					if distance>threshold: spatial[str(threshold)] += 1
				if flat_distance>1536.0: far_tiles[key] = true
		if nonempty: visible_nonempty += 1
	report["aerial_horizon_geometry"] = {"camera_position":[camera_position.x,camera_position.y,camera_position.z],"maximum_3d_distance":2048.0,"visible_nonempty_meshes":visible_nonempty,"inspected_vertices":inspected_vertices,"frustum_vertices":frustum_vertices,"horizontal_distance_over":horizontal,"spatial_distance_over":spatial,"max_horizontal_distance":max_horizontal,"max_spatial_distance":max_spatial,"tiles_with_horizontal_distance_over_1536":far_tiles.size(),"note":"Actual transformed triangle vertices in the camera frustum; duplicates included. Occlusion is not tested by the frustum query. This one-time CPU scan is outside FPS samples."}
	check(int(horizontal["1536"])>0,"aerial frustum contains actual nonempty far geometry beyond 1536 horizontal metres and within 2048 metres")
	_validate_root_extent()

func _validate_root_extent() -> void:
	var roots: Array = scene.distant.snapshot.get("roots",[])
	var position: Vector3 = scene.state.body_position
	var expected_origin := Vector2i(floori(position.x/512.0)*512,floori(position.z/512.0)*512)
	var actual: Dictionary = {}
	for key: Vector3i in roots: actual[key] = true
	check(roots.size()==81 and actual.size()==81,"interior Forest view has 81 distinct 512m roots")
	for dz in range(-4,5):
		for dx in range(-4,5):
			check(actual.has(Vector3i(expected_origin.x+dx*512,32,expected_origin.y+dz*512)),"Forest root plan contains every cell of its 9 by 9 square")
	var low := Vector2(expected_origin)-Vector2(2048,2048)
	var high := Vector2(expected_origin)+Vector2(2560,2560)
	check(low.x<=position.x-2048.0 and low.y<=position.z-2048.0 and high.x>=position.x+2048.0 and high.y>=position.z+2048.0,"Forest root square covers player plus or minus 2048m on both axes")
	report["forest_root_extent"] = {"count":roots.size(),"unique_count":actual.size(),"low_xz":[low.x,low.y],"high_xz":[high.x,high.y],"player_xz":[position.x,position.z],"required_cardinal_distance":2048}

func _cross_boundary_sequence() -> void:
	# Forest fixture starts at x=-191.5, just to the right of x=-192. Move left
	# across that negative boundary using real input and authoritative flight.
	var starting: Vector3 = scene.state.body_position
	scene.yaw = 0.0
	scene.pitch = -0.55
	scene._focus_gained()
	Input.action_press("left")
	Input.action_press("sprint")
	var previous := Time.get_ticks_usec()
	for frame in range(60):
		var now := Time.get_ticks_usec()
		await _frame(clampf(float(now-previous)/1000000.0,0.0,0.1))
		previous = now
		var sample := _stats()
		sample["frame"] = frame
		motion_samples.append(sample)
		await _capture("boundary-motion-%02d" % frame)
	Input.action_release("left")
	Input.action_release("sprint")
	scene._focus_lost()
	check(floori(scene.state.body_position.x/16.0)<floori(starting.x/16.0),"real flight crosses a negative chunk boundary")
	check(await _settle(_guard_ready,90.0),"fine guard restores after boundary motion")
	_validate_coverage(false)
	report["boundary_after"] = _stats()

func _validate_coverage(require_complete: bool = false) -> void:
	# The presenter polls network metadata at 100ms; validate the cut against the
	# plan it actually selected, rather than a newer unprocessed network epoch.
	var stream: Dictionary = scene.distant.snapshot
	var center: Vector2i = stream.get("center",Vector2i.ZERO)
	var roots: Array = stream.get("roots",[])
	for key: Vector3i in scene.distant.nodes:
		if not scene.distant.nodes[key].visible: continue
		var inside := false
		for root_key: Vector3i in roots:
			if key.x>=root_key.x and key.z>=root_key.z and key.x+key.y*16<=root_key.x+root_key.y*16 and key.z+key.y*16<=root_key.z+root_key.y*16: inside = true
		check(inside,"visible GPU tile belongs to a current root")
		if key.y>1:
			var touches_guard: bool = key.x<(center.x+5)*16 and key.x+key.y*16>(center.x-4)*16 and key.z<(center.y+5)*16 and key.z+key.y*16>(center.y-4)*16
			check(not touches_guard,"coarse active tile never overlaps the mandatory 1m guard")
		var parent_stride := key.y*2
		while parent_stride<=32:
			var span := parent_stride*16
			var parent := Vector3i(floori(float(key.x)/span)*span,parent_stride,floori(float(key.z)/span)*span)
			check(not scene.distant.nodes.has(parent) or not scene.distant.nodes[parent].visible,"visible parent and child never double-cover the same area")
			parent_stride *= 2
	if require_complete:
		check(not roots.is_empty(),"full horizon has a nonempty root plan")
		for key: Vector3i in roots: check(_covered(key),"each 512m root has complete actual GPU/near coverage")
		check(_guard_ready(),"complete horizon includes fine guard and near world")
		check(scene.distant.fallback.is_empty(),"complete horizon releases transition fallback")

func _validate_empty() -> void:
	var stream: Dictionary = scene.bridge.distant_state()
	check(scene.terrain_nodes.is_empty() and scene.terrain_pending.is_empty() and scene.terrain_uploaded.is_empty(),"near geometry and pending uploads release")
	check(scene.distant.nodes.is_empty() and scene.distant.fallback.is_empty(),"far and fallback GPU resources release")
	check(stream.get("tiles",[]).is_empty() and int(stream.get("pending",0))==0 and not bool(stream.get("in_flight",false)) and not bool(stream.get("ready",false)),"distant raw/worker session state releases")

func _worker_idle() -> bool:
	var stream: Dictionary = scene.bridge.distant_state()
	return not bool(stream.get("in_flight",false)) and not bool(stream.get("ready",false))

func _stats() -> Dictionary:
	var result: Dictionary = scene.distant.stats().duplicate(true)
	var stream: Dictionary = scene.bridge.distant_state().duplicate(true)
	var center: Vector2i = stream.get("center",Vector2i.ZERO)
	stream["tile_count"] = stream.get("tiles",[]).size()
	stream.erase("tiles")
	for field in ["roots","wanted","split","leaves"]:
		stream[field+"_count"] = stream.get(field,[]).size()
		stream.erase(field)
	result.erase("active_keys")
	stream["center"] = [center.x,center.y]
	result["stream"] = stream
	result["loaded_sections"] = scene.state.get("loaded_sections",0)
	result["near_pending"] = scene.terrain_pending.size()
	result["near_uploaded"] = scene.terrain_uploaded.size()
	result["total_uploads_last_frame"] = scene.terrain_total_updates_last_frame
	result["total_upload_usec_last_frame"] = scene.terrain_upload_usec_last_frame
	var point: Vector3 = scene.state.get("body_position",Vector3.ZERO)
	result["position"] = [point.x,point.y,point.z]
	result["status"] = scene.state.get("status","")
	result["journal"] = scene.state.get("journal",[])
	return result

func _write_progress(stage: String) -> void:
	if output.is_empty() or not is_instance_valid(scene): return
	var sample := _stats()
	sample["stage"] = stage
	sample["elapsed_seconds"] = float(Time.get_ticks_msec()-test_started_msec)/1000.0
	sample["complete_horizon"] = _complete()
	sample["first_horizon_coverage_ms"] = first_horizon_coverage_ms
	progress_samples.append(sample)
	var file := FileAccess.open(output.path_join("progress.json"),FileAccess.WRITE)
	if file!=null: file.store_string(JSON.stringify(progress_samples,"  "))
	print("DISTANT_PROGRESS=",JSON.stringify(sample))

func _finish() -> void:
	Input.action_release("left")
	Input.action_release("sprint")
	if scene.state.get("local_running",false):
		scene._leave_game(false)
		check(await _settle(func(): return scene.pending_action.is_empty(),20.0),"isolated authority closes before test exit")
	check(await _settle(_worker_idle,5.0),"bounded cancelled worker releases before test exit")
	_validate_empty()
	RenderingServer.viewport_set_measure_render_time(viewport_rid,false)
	report["main_cpu_ms"] = _summary(main_cpu)
	report["terrain_upload_ms"] = _summary(upload_cpu)
	report["upload_soft_budget_overruns"] = {"over_2000us":upload_soft_overruns,"total_frames":upload_cpu.size()}
	report["peaks"] = {"pending":max_pending,"raw_bytes":max_raw_bytes,"vertices":max_vertices,"fallback_sections":max_fallback}
	report["motion"] = motion_samples
	report["progress"] = progress_samples
	var file := FileAccess.open(output.path_join("metrics.json"),FileAccess.WRITE)
	check(file!=null,"distant metrics can be written")
	report["passed"] = not failed
	if file!=null: file.store_string(JSON.stringify(report,"  "))
	print("DISTANT_REVIEW_PATH=",output)
	print("GODOT_DISTANT_RESULT=","FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

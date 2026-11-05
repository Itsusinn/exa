extends Node3D

const HudScript = preload("res://src/hud.gd")
const IndustryView = preload("res://src/industry_view.gd")
const DistillationView = preload("res://src/distillation_view.gd")
const MaterialShader = preload("res://materials/voxel.gdshader")
var bridge: GameBridge
var camera: Camera3D
var hud: CanvasLayer
var environment: Environment
var sun: DirectionalLight3D
var terrain_root: Node3D
var terrain_nodes: Dictionary = {}
var terrain_pending: Dictionary = {}
var terrain_updates_last_frame := 0
const TERRAIN_BUDGET_USEC := 2000
const TERRAIN_UPDATES_PER_FRAME := 2
var entities: Dictionary = {}
var item_nodes: Dictionary = {}
var actor_meshes: Array[ArrayMesh] = []
var item_mesh: ArrayMesh
var material: ShaderMaterial
var state: Dictionary = {}
var revision := Vector2i(-1, -1)
var yaw := 0.0
var pitch := -0.10
var attack := false
var pickup := false
var captured := false
var focused := true
var capture_path := ""
var capture_frames := 64
var ready_frames := 0
var capture_pending := false
var capture_requires_world := false
var started := Time.get_ticks_msec()
var options: Dictionary = {}
var pending_action := ""
var entering_game := false
var was_ready := false
var exit_code := 0
var container_requested_at := 0
var hotbar_cursor := 0
var industry: Node3D
var distillation: CanvasLayer

func _ready() -> void:
	bridge = GameBridge.new()
	bridge.name = "GameBridge"
	add_child(bridge)
	_setup_input()
	_setup_world()
	hud = HudScript.new()
	add_child(hud)
	industry = IndustryView.new()
	industry.name = "Industry"
	add_child(industry)
	industry.close_requested.connect(_close_equipment)
	distillation = DistillationView.new()
	distillation.name = "Distillation"
	add_child(distillation)
	distillation.close_requested.connect(_close_distillation)
	hud.connect_requested.connect(_connect_game)
	hud.demo_requested.connect(_demo)
	hud.disconnect_requested.connect(_disconnect_game)
	hud.chat_requested.connect(func(text: String): bridge.send_chat(text))
	hud.container_transfer_requested.connect(func(request: Dictionary): bridge.transfer_container(request))
	hud.inventory_move_requested.connect(func(r: int, f: int, t: int, q: int): bridge.move_inventory(r,f,t,q))
	hud.inventory_quick_move_requested.connect(func(r: int, s: int): bridge.quick_move_inventory(r,s))
	hud.container_close_requested.connect(_close_inventory)
	hud.fov_changed.connect(func(value: float): camera.fov = value)
	hud.lighting_changed.connect(func(enabled: bool): environment.sdfgi_enabled = enabled)
	hud.singleplayer_requested.connect(_singleplayer)
	hud.resume_requested.connect(_resume_game)
	hud.home_requested.connect(func(): _leave_game(false))
	hud.quit_requested.connect(func(): _leave_game(true))
	camera.fov = hud.fov_slider.value
	environment.sdfgi_enabled = hud.gi_toggle.button_pressed
	get_tree().auto_accept_quit = false
	options = _parse_options(OS.get_cmdline_user_args())
	capture_requires_world = options.has("demo") or options.has("connect") or options.has("singleplayer")
	if options.has("server"): hud.address.text = options.server
	if options.has("name"): hud.player_name.text = options.name
	if options.has("cert"): hud.certificate.text = options.cert
	if options.has("fov"): hud.fov_slider.value = clampf(float(options.fov),55,100)
	if options.has("yaw"): yaw = deg_to_rad(float(options.yaw))
	if options.has("capture"): capture_path = options.capture
	if options.has("capture-frames"): capture_frames = clampi(int(options["capture-frames"]),1,4096)
	if options.get("lighting", "gi") == "direct": hud.gi_toggle.button_pressed = false
	if options.has("demo"): _demo()
	if options.has("singleplayer"): _singleplayer()
	if options.has("connect"): _connect_game(hud.address.text,hud.player_name.text,hud.certificate.text)
	state = bridge.snapshot()
	hud.update_state(state, captured)
	get_window().focus_exited.connect(_focus_lost)
	get_window().focus_entered.connect(_focus_gained)
	focused = get_window().has_focus()

func _parse_options(args: PackedStringArray) -> Dictionary:
	var result := {}
	var i := 0
	while i < args.size():
		var key := args[i].trim_prefix("--")
		if key in ["demo", "connect", "singleplayer"]:
			result[key] = true
		elif key in ["server", "name", "cert", "fov", "yaw", "capture", "capture-frames", "lighting"]:
			i += 1
			if i < args.size(): result[key] = args[i]
		i += 1
	return result

func _setup_input() -> void:
	var mappings := {"forward": [KEY_W, KEY_UP], "back": [KEY_S, KEY_DOWN],
		"left": [KEY_A, KEY_LEFT], "right": [KEY_D, KEY_RIGHT], "jump": [KEY_SPACE],
		"descend": [KEY_SHIFT], "sprint": [KEY_CTRL], "interact": [KEY_E], "rotate_equipment": [KEY_R]}
	for action in mappings:
		if not InputMap.has_action(action): InputMap.add_action(action)
		for key in mappings[action]:
			var event := InputEventKey.new()
			if action in ["interact", "rotate_equipment"]: event.keycode = key
			else: event.physical_keycode = key
			InputMap.action_add_event(action,event)
	for i in range(9):
		var action := "hotbar_%d" % i
		if not InputMap.has_action(action): InputMap.add_action(action)
		var key := InputEventKey.new()
		key.keycode = KEY_1 + i
		InputMap.action_add_event(action,key)

func _setup_world() -> void:
	material = ShaderMaterial.new()
	material.shader = MaterialShader
	material.set_shader_parameter("granular_normal",load("res://materials/granular_normal.tres"))
	var surfaces := {"wood":"weathered_brown_planks", "stone":"stone_brick_wall_001", "grass":"leafy_grass", "bark":"bark_brown_02"}
	var maps := {"color":"diff", "normal":"nor_gl", "rough":"rough"}
	for surface in surfaces:
		for map in maps:
			material.set_shader_parameter("%s_%s" % [surface,map],load("res://materials/textures/%s_%s_1k.jpg" % [surfaces[surface],maps[map]]))
	terrain_root = Node3D.new()
	terrain_root.name = "TerrainChunks"
	add_child(terrain_root)
	camera = Camera3D.new()
	camera.name = "FirstPersonCamera"
	camera.near = 0.05
	camera.far = 150
	camera.fov = 72
	camera.position = Vector3(28.5,8.58,32.5)
	camera.current = true
	add_child(camera)
	sun = DirectionalLight3D.new()
	sun.name = "Sun"
	sun.rotation_degrees = Vector3(-38,-35,0)
	sun.light_color = Color(1.0,0.975,0.92)
	sun.light_energy = 1.0
	sun.light_angular_distance = 1.2
	sun.shadow_enabled = true
	sun.directional_shadow_max_distance = 100
	sun.directional_shadow_mode = DirectionalLight3D.SHADOW_PARALLEL_4_SPLITS
	sun.directional_shadow_blend_splits = true
	sun.directional_shadow_split_1 = 0.1
	sun.directional_shadow_split_2 = 0.25
	sun.directional_shadow_split_3 = 0.5
	sun.shadow_bias = 0.03
	sun.shadow_normal_bias = 0.5
	add_child(sun)
	environment = Environment.new()
	environment.background_mode = Environment.BG_SKY
	var sky := Sky.new()
	var sky_material := ProceduralSkyMaterial.new()
	sky_material.sky_top_color = Color("7896b3")
	sky_material.sky_horizon_color = Color("bfcbd1")
	sky_material.ground_horizon_color = Color("bfcbd1")
	sky_material.ground_bottom_color = Color("424b39")
	sky.sky_material = sky_material
	environment.sky = sky
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_SKY
	environment.ambient_light_energy = 0.3
	environment.tonemap_mode = Environment.TONE_MAPPER_ACES
	environment.sdfgi_enabled = true
	environment.sdfgi_min_cell_size = 0.35
	environment.sdfgi_cascades = 4
	environment.sdfgi_use_occlusion = true
	environment.sdfgi_bounce_feedback = 0.3
	environment.ssao_enabled = true
	# Vertex AO supplies block contacts; SSAO adds small dynamic-object contacts.
	environment.ssao_radius = 0.45
	environment.ssao_intensity = 0.65
	environment.fog_enabled = true
	environment.fog_light_color = Color("bfcbd1")
	environment.fog_density = 0.0015
	var world := WorldEnvironment.new()
	world.environment = environment
	add_child(world)
	actor_meshes.append(_mesh(bridge.actor_geometry(0)))
	actor_meshes.append(_mesh(bridge.actor_geometry(1)))
	item_mesh = _mesh(bridge.item_geometry())

func _mesh(data: Dictionary) -> ArrayMesh:
	var result := ArrayMesh.new()
	if data.is_empty() or data.vertices.is_empty(): return result
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = data.vertices
	arrays[Mesh.ARRAY_NORMAL] = data.normals
	arrays[Mesh.ARRAY_COLOR] = data.colors
	arrays[Mesh.ARRAY_TEX_UV] = data.uv
	arrays[Mesh.ARRAY_TEX_UV2] = data.uv2
	result.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays)
	result.surface_set_material(0, material)
	return result

func _demo() -> void:
	_release_mouse()
	bridge.start_demo()
	hud.inventory_open = false
	hud.menu.hide()
	hud.menu.in_game = true
	yaw = deg_to_rad(float(options.get("yaw",0)))
	pitch = -0.10

func _connect_game(address: String, player_name: String, certificate: String) -> void:
	_release_mouse()
	bridge.connect_game(address,player_name,certificate)
	hud.inventory_open = false
	entering_game = true
	hud.menu.in_game = false
	hud.menu.show_page("busy")

func _singleplayer() -> void:
	_release_mouse()
	var save_path := preload("res://src/app_config.gd").environment("SAVE_PATH")
	if save_path.is_empty(): save_path = "save/world3d.bin"
	bridge.start_singleplayer(save_path,hud.player_name.text)
	hud.inventory_open = false
	entering_game = true
	hud.menu.in_game = false
	hud.menu.show_page("busy")

func _resume_game() -> void:
	if not state.get("ready", false): return
	hud.menu.hide()
	hud.inventory_open = false
	var owner := get_viewport().gui_get_focus_owner()
	if owner: owner.release_focus()
	_release_mouse()
	_sync_mouse_capture()

func _leave_game(quit_game: bool) -> void:
	if not pending_action.is_empty(): return
	# A failed save stays visible. A subsequent explicit quit may still close the app.
	if quit_game and state.get("local_error", false) and not state.get("local_running", false) and not state.get("shutdown_pending", false):
		get_tree().quit()
		return
	entering_game = false
	pending_action = "quit" if quit_game else "home"
	_disconnect_game()
	hud.menu.show_page("busy")

func _notification(what: int) -> void:
	if what == NOTIFICATION_WM_CLOSE_REQUEST and is_instance_valid(hud): _leave_game(true)

func _disconnect_game() -> void:
	_close_distillation()
	_close_equipment()
	_close_container()
	_release_mouse()
	bridge.disconnect_game()
	hud.inventory_open = false

func _release_mouse() -> void:
	captured = false
	attack = false
	pickup = false
	Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
	for action in ["forward","back","left","right","jump","descend","sprint"]: Input.action_release(action)
	if is_instance_valid(bridge): bridge.clear_actions()

func _focus_lost() -> void:
	focused = false
	_close_distillation()
	if is_instance_valid(bridge): bridge.cancel_pipe_route()
	_close_equipment()
	_close_container()
	_release_mouse()

func _focus_gained() -> void:
	focused = true
	_sync_mouse_capture()

func _sync_mouse_capture() -> void:
	var owner := get_viewport().gui_get_focus_owner()
	var ui_focused := owner != null and owner.is_visible_in_tree()
	var should_capture: bool = focused and state.get("ready",false) and pending_action.is_empty() and not entering_game and not hud.menu.visible and not hud.inventory_open and not hud.container_wanted and not hud.container_open and not industry.opened and not industry.build_opened and not distillation.opened and not ui_focused
	if should_capture and not captured:
		captured = true
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	elif not should_capture and captured:
		_release_mouse()

func _close_container() -> void:
	if hud.container_wanted or hud.container_open: bridge.close_container()
	hud.container_wanted = false
	hud.container_open = false
	hud.container_panel.hide()
	hud.container_panel.clear_selection()

func _close_inventory() -> void:
	_close_container()
	hud.inventory_open = false
	_release_mouse()
	var owner := get_viewport().gui_get_focus_owner()
	if owner: owner.release_focus()
	_sync_mouse_capture()

func _select_hotbar(slot: int) -> void:
	_close_equipment()
	bridge.cancel_pipe_route()
	hotbar_cursor = posmod(slot,9)
	bridge.select_hotbar(hotbar_cursor)

func _open_container() -> void:
	bridge.cancel_pipe_route()
	var device := bridge.equipment_target(yaw,pitch)
	if not device.is_empty():
		industry.show_device(device)
		_release_mouse()
		return
	var target := bridge.container_target(yaw,pitch)
	if target.is_empty(): return
	hud.container_wanted = true
	container_requested_at = Time.get_ticks_msec()
	hud.inventory_open = false
	bridge.open_container(target.position)
	_release_mouse()

func _close_equipment() -> void:
	if not is_instance_valid(industry): return
	industry.close()
	var owner := get_viewport().gui_get_focus_owner()
	if owner: owner.release_focus()
	_sync_mouse_capture()

func _close_distillation() -> void:
	if not is_instance_valid(distillation): return
	distillation.close()
	var owner := get_viewport().gui_get_focus_owner()
	if owner: owner.release_focus()
	_sync_mouse_capture()

func _input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_ESCAPE:
			if distillation.opened:
				_close_distillation()
				get_viewport().set_input_as_handled()
				return
			if industry.route_state.get("active",false):
				bridge.cancel_pipe_route()
				industry.route_state.clear()
				get_viewport().set_input_as_handled()
				return
			if industry.opened or industry.build_opened:
				_close_equipment()
				get_viewport().set_input_as_handled()
				return
			if hud.panel.visible and not hud.panel.selected.is_empty():
				hud.panel.clear_selection()
				get_viewport().set_input_as_handled()
				return
			if hud.container_wanted or hud.container_open or hud.inventory_open:
				_close_inventory()
				get_viewport().set_input_as_handled()
				return
			_release_mouse()
			if pending_action.is_empty() and not entering_game:
				if hud.menu.visible:
					if hud.menu.page == "settings": hud.menu.show_page(hud.menu.return_page)
					elif hud.menu.page == "multiplayer": hud.menu.show_page("home")
					elif hud.menu.page == "pause": _resume_game()
				elif state.get("ready", false):
					hud.inventory_open = false
					hud.menu.in_game = true
					hud.menu.show_page("pause")
			get_viewport().set_input_as_handled()
		elif event.keycode == KEY_J and captured and focused and state.get("ready",false) and not state.get("demo",false):
			if distillation.show_design(self):
				bridge.cancel_pipe_route()
				_close_equipment()
				_close_container()
				_release_mouse()
			get_viewport().set_input_as_handled()
			return
		elif event.keycode == KEY_B and captured and focused and state.get("ready",false) and not hud.menu.visible and not hud.inventory_open:
			if industry.show_build(self):
				bridge.cancel_pipe_route()
				_release_mouse()
			get_viewport().set_input_as_handled()
			return
		elif event.keycode == KEY_P and captured and focused and state.get("ready",false) and not hud.menu.visible and not hud.inventory_open:
			bridge.toggle_pipe_mode()
			industry.route_state = bridge.pipe_route_state(yaw,pitch)
			get_viewport().set_input_as_handled()
			return
		elif event.is_action_pressed("interact") and captured and not hud.menu.visible and not hud.inventory_open:
			_open_container()
			get_viewport().set_input_as_handled()
		elif event.keycode == KEY_TAB and state.get("ready", false) and not hud.menu.visible:
			_close_distillation()
			bridge.cancel_pipe_route()
			_close_equipment()
			_close_container()
			hud.inventory_open = not hud.inventory_open
			_release_mouse()
			var focus_owner := get_viewport().gui_get_focus_owner()
			if focus_owner: focus_owner.release_focus()
			_sync_mouse_capture()
			get_viewport().set_input_as_handled()
		elif event.keycode == KEY_SLASH and captured and state.get("ready", false) and not state.get("demo", false):
			bridge.cancel_pipe_route()
			hud.inventory_open = true
			_release_mouse()
			hud.update_state(state, false)
			hud.chat.text = "/"
			hud.chat.grab_focus()
			hud.chat.caret_column = 1
			get_viewport().set_input_as_handled()
		elif captured and focused and not hud.menu.visible and not hud.inventory_open and not hud.container_open:
			if event.is_action_pressed("rotate_equipment"):
				bridge.rotate_equipment()
				get_viewport().set_input_as_handled()
				return
			for i in range(9):
				if event.is_action_pressed("hotbar_%d" % i):
					_select_hotbar(i)
					get_viewport().set_input_as_handled()

func _unhandled_input(event: InputEvent) -> void:
	if not captured or not focused or hud.menu.visible: return
	if event is InputEventKey and event.pressed and not event.echo:
		if captured and event.physical_keycode == KEY_F: attack = true
		elif captured and event.physical_keycode == KEY_G: pickup = true
	if event is InputEventMouseMotion and captured:
		var sensitivity: float = 0.0025 * hud.menu.sensitivity.value
		yaw = fposmod(yaw + event.relative.x * sensitivity,TAU)
		pitch = clampf(pitch - event.relative.y * sensitivity,-1.45,1.45)
	if event is InputEventMouseButton and event.pressed:
		if industry.route_state.get("active",false) and event.button_index in [MOUSE_BUTTON_LEFT,MOUSE_BUTTON_RIGHT]:
			if event.button_index == MOUSE_BUTTON_RIGHT: bridge.click_pipe_route(yaw,pitch)
			get_viewport().set_input_as_handled()
			return
		if event.button_index in [MOUSE_BUTTON_LEFT,MOUSE_BUTTON_RIGHT] and state.get("ready",false) and not hud.inventory_open and not hud.container_wanted and not hud.container_open:
			bridge.edit_block(yaw,pitch,event.button_index == MOUSE_BUTTON_RIGHT)
			get_viewport().set_input_as_handled()
		elif captured and event.button_index == MOUSE_BUTTON_WHEEL_UP: _select_hotbar(hotbar_cursor-1)
		elif captured and event.button_index == MOUSE_BUTTON_WHEEL_DOWN: _select_hotbar(hotbar_cursor+1)

func _process(delta: float) -> void:
	_sync_mouse_capture()
	var controls := {"yaw": yaw, "pitch": pitch}
	if captured and focused and not hud.inventory_open and not hud.menu.visible and not hud.container_wanted and not hud.container_open and not industry.opened and not industry.build_opened and not distillation.opened:
		controls.forward = Input.get_action_strength("forward") - Input.get_action_strength("back")
		controls.strafe = Input.get_action_strength("right") - Input.get_action_strength("left")
		controls.jump = Input.is_action_pressed("jump")
		controls.descend = Input.is_action_pressed("descend")
		controls.sprint = Input.is_action_pressed("sprint")
		controls.attack = attack
		controls.pickup = pickup
	attack = false
	pickup = false
	state = bridge.advance(delta,controls)
	var first_ready: bool = state.ready and not was_ready
	if hud.container_wanted and not state.container.is_empty():
		hud.container_open = true
	elif hud.container_open and state.container.is_empty():
		_close_container()
	elif hud.container_wanted and Time.get_ticks_msec() - container_requested_at > 3000:
		_close_container()
	if not state.ready: _close_container()
	if not state.ready: industry.close()
	distillation.update_view(self)
	hud.container_target = bridge.container_target(yaw,pitch) if captured else {}
	hud.block_target = bridge.block_target(yaw,pitch) if captured else {}
	hud.equipment_target = bridge.equipment_target(yaw,pitch) if captured else {}
	if not pending_action.is_empty():
		hud.menu.feedback.text = state.status
		if not state.local_running and not state.shutdown_pending:
			var action := pending_action
			pending_action = ""
			hud.menu.in_game = false
			hud.menu.show_page("home")
			if state.local_error:
				hud.menu.feedback.text = state.status + "\n未自动退出，请检查存档错误。"
			elif action == "quit": get_tree().quit(exit_code)
	elif entering_game:
		hud.menu.feedback.text = state.status
		if state.ready:
			entering_game = false
			hud.menu.in_game = true
			hud.menu.hide()
		elif not state.connected and not state.local_running:
			entering_game = false
			hud.menu.show_page("home")
			hud.menu.feedback.text = state.status
	elif was_ready and not state.ready:
		_release_mouse()
		hud.menu.in_game = false
		hud.menu.show_page("home")
		hud.menu.feedback.text = state.status
	was_ready = state.ready
	_sync_mouse_capture()
	var next_revision := Vector2i(state.generation,state.revision)
	if revision.x != next_revision.x:
		_clear_world()
	if first_ready and not state.demo:
		yaw = state.spawn_rotation.x
		pitch = state.spawn_rotation.y
		hotbar_cursor = state.selected_slot
	if state.ready and revision != next_revision:
		revision = next_revision
		ready_frames = 0
	if state.ready: _rebuild_terrain()
	if state.ready:
		camera.position = state.position + Vector3(0,1.58,0)
		camera.rotation = Vector3(pitch,-yaw,0)
		_update_entities()
		industry.update_view(self)
		hud.equipment_placement = industry.placement
		hud.pipe_route = industry.route_state
		ready_frames += 1
	elif hud.menu.visible and hud.menu.page == "home" and not entering_game:
		ready_frames += 1
	hud.equipment_open = industry.opened or industry.build_opened or distillation.opened
	hud.update_state(state,captured)
	if not capture_path.is_empty() and not capture_pending:
		if Time.get_ticks_msec() - started > 120000:
			push_error("Capture timed out waiting for world")
			capture_pending = true
			exit_code = 1
			_leave_game(true)
		elif (state.ready or (not capture_requires_world and hud.menu.visible and hud.menu.page == "home")) and ready_frames >= capture_frames and terrain_pending.is_empty():
			capture_pending = true
			_capture()

func _clear_world() -> void:
	distillation.reset()
	industry.clear()
	hotbar_cursor = 0
	terrain_nodes.clear()
	terrain_pending.clear()
	for child in terrain_root.get_children():
		terrain_root.remove_child(child)
		child.queue_free()
	for node in entities.values(): node.queue_free()
	for node in item_nodes.values(): node.queue_free()
	entities.clear()
	item_nodes.clear()
	revision = Vector2i(state.generation,-1)

func _rebuild_terrain() -> void:
	var started_at := Time.get_ticks_usec()
	terrain_updates_last_frame = 0
	for origin in bridge.take_dirty_chunks():
		if not bridge.has_chunk(origin):
			terrain_pending.erase(origin)
			_remove_terrain(origin)
		else:
			terrain_pending[origin] = true
	if terrain_pending.is_empty(): return
	# Coalesce repeated changes and use the latest voxel state when work executes.
	# Removed chunks never wait behind uploads, and old sessions clear the queue.
	var origins := terrain_pending.keys()
	var feet: Vector3 = state.body_position
	origins.sort_custom(func(a: Vector3i, b: Vector3i):
		return (Vector3(a)+Vector3(8,8,8)).distance_squared_to(feet) < (Vector3(b)+Vector3(8,8,8)).distance_squared_to(feet))
	for origin in origins:
		if terrain_updates_last_frame >= TERRAIN_UPDATES_PER_FRAME: break
		if terrain_updates_last_frame > 0 and Time.get_ticks_usec()-started_at >= TERRAIN_BUDGET_USEC: break
		terrain_pending.erase(origin)
		terrain_updates_last_frame += 1
		var mesh := _mesh(bridge.chunk_geometry(origin))
		var node: MeshInstance3D = terrain_nodes.get(origin)
		if mesh.get_surface_count() == 0:
			_remove_terrain(origin)
			continue
		if not is_instance_valid(node):
			node = MeshInstance3D.new()
			node.name = "Chunk_%d_%d_%d" % [origin.x,origin.y,origin.z]
			node.gi_mode = GeometryInstance3D.GI_MODE_STATIC
			terrain_root.add_child(node)
			terrain_nodes[origin] = node
		node.mesh = mesh

func _remove_terrain(origin: Vector3i) -> void:
	var node: MeshInstance3D = terrain_nodes.get(origin)
	if is_instance_valid(node):
		terrain_root.remove_child(node)
		node.queue_free()
	terrain_nodes.erase(origin)

func _update_entities() -> void:
	var live := {}
	for actor in state.actors:
		live[actor.id] = true
		if not entities.has(actor.id):
			var node := MeshInstance3D.new()
			node.mesh = actor_meshes[clampi(actor.kind,0,1)]
			node.gi_mode = GeometryInstance3D.GI_MODE_DYNAMIC
			add_child(node)
			entities[actor.id] = node
		entities[actor.id].position = actor.position
		entities[actor.id].rotation.y = -actor.yaw
	for id in entities.keys():
		if not live.has(id):
			entities[id].queue_free()
			entities.erase(id)
	live.clear()
	for item in state.items:
		live[item.id] = true
		if not item_nodes.has(item.id):
			var node := MeshInstance3D.new()
			node.mesh = item_mesh
			add_child(node)
			item_nodes[item.id] = node
		item_nodes[item.id].position = item.position
	for id in item_nodes.keys():
		if not live.has(id):
			item_nodes[id].queue_free()
			item_nodes.erase(id)

func _capture() -> void:
	await RenderingServer.frame_post_draw
	var image := get_viewport().get_texture().get_image()
	var error := image.save_png(capture_path)
	if error != OK: push_error("Capture save failed: %s" % error)
	exit_code = 0 if error == OK else 1
	_leave_game(true)

func _exit_tree() -> void:
	_release_mouse()
	if is_instance_valid(bridge): bridge.disconnect_game()

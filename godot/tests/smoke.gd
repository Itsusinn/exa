extends SceneTree
var failed := false

func check(condition: bool, message: String) -> void:
	if not condition:
		failed = true
		push_error(message)
	else: print("PASS: ",message)

func _initialize() -> void:
	call_deferred("run")

func run() -> void:
	check_configuration_compatibility()
	var scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.set_process(false)
	scene._focus_gained() # Headless windows have no OS focus; simulate the focused game.
	scene.hud.menu.loading_settings = true # Tests must not overwrite user preferences.
	scene.hud.menu.set_antialiasing("taa")
	check(root.use_taa and root.msaa_3d == Viewport.MSAA_DISABLED,"TAA enables without MSAA")
	scene.hud.menu.set_antialiasing("fxaa")
	check(not root.use_taa and root.screen_space_aa == Viewport.SCREEN_SPACE_AA_FXAA,"FXAA clears TAA")
	scene.hud.menu.set_antialiasing("msaa2")
	check(root.msaa_3d == Viewport.MSAA_2X and root.screen_space_aa == Viewport.SCREEN_SPACE_AA_DISABLED,"MSAA clears screen-space AA")
	scene.hud.menu.set_antialiasing("off")
	check(root.msaa_3d == Viewport.MSAA_DISABLED and not root.use_taa,"AA can be disabled")
	scene.hud.menu.set_antialiasing("invalid")
	check(root.msaa_3d == Viewport.MSAA_4X,"Unknown AA preference falls back to MSAA 4x")
	check(scene.hud.menu.visible and scene.hud.menu.page == "home", "startup shows main menu")
	scene.hud.menu.show_page("multiplayer")
	check(scene.hud.address.is_visible_in_tree(), "multiplayer form opens separately")
	scene._demo()
	var bridge: GameBridge = scene.bridge
	var first := bridge.snapshot()
	check(first.ready and first.demo,"offline world and native extension loaded")
	check(bridge.chunk_geometry(Vector3i(16,0,16)).vertices.size() > 0,"chunk geometry crosses extension boundary")
	check(bridge.chunk_geometry(Vector3i(-16,0,0)).is_empty(),"invalid chunk rejected")
	var start: Vector3 = first.body_position
	var peak := start.y
	for frame in range(600):
		var s := bridge.advance(1.0/120.0,{"forward":1.0,"jump":frame < 4})
		peak = maxf(peak,s.body_position.y)
	var last := bridge.snapshot()
	check(peak > start.y + 1.0 and last.grounded,"jump rises and lands through Rust simulation")
	check(last.body_position.z > 19.0 and last.body_position.z < 19.4,"stone wall stops native simulated movement")
	var before: Vector3 = last.body_position
	bridge.advance(0.1,{"forward":99.0})
	check(bridge.snapshot().body_position == before,"invalid bridge input cannot move player")
	scene._demo()
	scene._process(0.0)
	check(scene.terrain_root.get_child_count() > 0,"Godot terrain scene built")
	check(scene.terrain_updates_last_frame <= 2 and not scene.terrain_pending.is_empty(),"initial terrain work is split across frames")
	check(scene.terrain_nodes.has(Vector3i(16,0,32)),"nearest terrain is built first")
	check(scene.entities.size() == 1 and scene.item_nodes.size() == 1,"actors and ground items displayed")
	check(scene.captured,"entering gameplay automatically captures mouse")
	if DisplayServer.get_name() != "headless":
		check(Input.mouse_mode == Input.MOUSE_MODE_CAPTURED,"native window captures mouse without a click")
	Input.action_press("forward")
	scene._process(0.05)
	check(scene.state.body_position.z < start.z,"Godot input reaches Rust prediction")
	scene._focus_lost()
	check(not scene.captured and not Input.is_action_pressed("forward"),"focus loss releases mouse and movement")
	scene._process(0.0)
	check(not scene.captured,"unfocused gameplay does not recapture mouse")
	scene._focus_gained()
	check(scene.captured,"returning to gameplay focus recaptures mouse")
	var key := InputEventKey.new()
	key.pressed = true
	key.keycode = KEY_TAB
	scene._input(key)
	scene._process(0.05)
	check(scene.hud.inventory_open and scene.hud.panel.visible and not scene.captured,"inventory UI opens and releases movement")
	scene._focus_lost()
	scene._focus_gained()
	check(not scene.captured,"inventory retains free cursor after window focus returns")
	scene.hud.chat.grab_focus()
	scene._input(key)
	check(not scene.hud.inventory_open and scene.captured,"Tab closes focused inventory and recaptures mouse")
	scene.hud.fov_slider.value = 85
	check(scene.camera.fov == 85,"FOV control updates camera")
	key.keycode = KEY_ESCAPE
	Input.action_press("forward")
	scene.attack = true
	scene._input(key)
	scene._process(0.0)
	check(scene.hud.menu.page == "pause" and scene.hud.menu.visible, "Escape opens game menu")
	check(not scene.captured and not scene.attack and not Input.is_action_pressed("forward"), "game menu clears held movement and attack")
	if DisplayServer.get_name() != "headless":
		check(Input.mouse_mode == Input.MOUSE_MODE_VISIBLE,"native window releases mouse for pause menu")
	scene._focus_lost()
	scene._focus_gained()
	check(not scene.captured,"pause menu retains free cursor after window focus returns")
	var click := InputEventMouseButton.new()
	click.button_index = MOUSE_BUTTON_LEFT
	click.pressed = true
	scene._unhandled_input(click)
	check(not scene.captured and not scene.attack, "menu click cannot capture or attack")
	scene.hud.menu.open_settings()
	scene._input(key)
	check(scene.hud.menu.page == "pause", "settings Escape returns to game menu")
	scene._input(key)
	check(not scene.hud.menu.visible and scene.captured, "second Escape resumes gameplay and captures mouse")
	if DisplayServer.get_name() != "headless":
		check(Input.mouse_mode == Input.MOUSE_MODE_CAPTURED,"native window recaptures mouse after resume")
	scene._disconnect_game()
	scene._process(0.0)
	check(not scene.state.ready and scene.entities.is_empty(),"disconnect clears scene and session")
	check(scene.terrain_pending.is_empty() and scene.terrain_nodes.is_empty(),"disconnect cancels unfinished terrain work")
	scene._demo()
	scene._process(0.0)
	check(scene.camera.position.distance_to(Vector3(28.5,8.58,32.5)) < 0.001,"new session resets camera interpolation")
	# At least one chunk advances per call; the time budget can defer the second.
	var pending_count: int = scene.terrain_pending.size()
	for i in range(pending_count + 1): scene._process(0.0)
	check(scene.terrain_pending.is_empty(),"terrain queue drains without a new map revision")
	var previous_save := preload("res://src/app_config.gd").environment("SAVE_PATH")
	var isolated_save := ProjectSettings.globalize_path("res://../target/godot-ui-%d/world3d.bin" % Time.get_ticks_usec())
	OS.set_environment("EXA_SAVE_PATH", isolated_save)
	scene.bridge.set_identity_path(isolated_save + ".client.uuid")
	scene._singleplayer()
	for i in range(300):
		scene._process(0.0)
		if scene.state.ready and not scene.state.demo: break
		await create_timer(0.02).timeout
	check(scene.state.ready and not scene.state.demo and scene.state.local_running, "single-player menu starts embedded authority")
	check(not scene.hud.menu.visible, "successful connection dismisses loading menu")
	check(scene.captured, "successful connection automatically captures mouse")
	scene._leave_game(false)
	for i in range(300):
		scene._process(0.0)
		if scene.pending_action.is_empty(): break
		await create_timer(0.02).timeout
	check(not scene.state.local_running and scene.pending_action.is_empty(), "return home waits for local authority shutdown")
	check(FileAccess.file_exists(isolated_save), "return home saves isolated map")
	check(scene.hud.menu.visible and scene.hud.menu.page == "home", "return home restores main menu")
	check(not scene.captured, "return home releases mouse")
	scene._connect_game("invalid address", "Test", "missing.der")
	scene._process(0.0)
	check(scene.hud.menu.page == "home" and not scene.entering_game and not scene.hud.menu.feedback.text.is_empty(), "connection failure returns to menu with diagnostic")
	OS.set_environment("EXA_SAVE_PATH", previous_save)
	scene.queue_free()
	await process_frame
	print("GODOT_SMOKE_RESULT=", "FAIL" if failed else "PASS")
	quit(1 if failed else 0)

func check_configuration_compatibility() -> void:
	var config = preload("res://src/app_config.gd")
	check(ProjectSettings.get_setting("application/config/name") == "exa", "project uses the exa name")
	var folder := ProjectSettings.globalize_path("res://../target/rename-config-%d" % Time.get_ticks_usec())
	var current := folder.path_join("exa")
	var legacy := folder.path_join("Dungeon 3D")
	DirAccess.make_dir_recursive_absolute(current)
	DirAccess.make_dir_recursive_absolute(legacy)
	for filename in ["settings.cfg", "column-design.json"]:
		check(config.user_file(filename,current) == current.path_join(filename), "new installations use the exa user directory")
		var old_file := FileAccess.open(legacy.path_join(filename),FileAccess.WRITE)
		old_file.store_string("legacy bytes")
		old_file.close()
		check(config.user_file(filename,current) == legacy.path_join(filename), "existing settings and designs retain their original paths")
		var new_file := FileAccess.open(current.path_join(filename),FileAccess.WRITE)
		new_file.store_string("exa bytes")
		new_file.close()
		check(config.user_file(filename,current) == current.path_join(filename), "existing exa data takes precedence")
		check(FileAccess.get_file_as_string(legacy.path_join(filename)) == "legacy bytes", "legacy user files are preserved")
	var suffix := "RENAME_TEST_%d" % Time.get_ticks_usec()
	OS.set_environment("CDDA_"+suffix,"old")
	check(config.environment(suffix) == "old", "legacy CDDA environment remains supported")
	OS.set_environment("EXA_"+suffix,"new")
	check(config.environment(suffix) == "new", "EXA environment overrides CDDA")
	OS.unset_environment("EXA_"+suffix)
	OS.unset_environment("CDDA_"+suffix)

extends SceneTree
var failed := false
var scene: Node3D
var chest_position: Vector3

func check(condition: bool, message: String) -> void:
	if condition: print("PASS: ", message)
	else:
		failed = true
		push_error(message)

func _initialize() -> void: call_deferred("run")

func settle(until: Callable) -> bool:
	for i in range(400):
		scene._process(0.0)
		if until.call(): return true
		await create_timer(0.02).timeout
	return false

func open_chest() -> bool:
	var eye: Vector3 = scene.state.body_position + Vector3(0,1.58,0)
	var offset := chest_position + Vector3(0.5,0.5,0.5) - eye
	scene.yaw = atan2(offset.x,-offset.z)
	scene.pitch = atan2(offset.y,Vector2(offset.x,offset.z).length())
	scene.captured = true
	scene.focused = true
	var event := InputEventKey.new()
	event.pressed = true
	event.physical_keycode = KEY_E
	event.keycode = KEY_E
	scene._input(event)
	return await settle(func(): return scene.hud.container_open)

func run() -> void:
	var fixture = JSON.parse_string(FileAccess.get_file_as_string("res://../target/spawn-site.json"))
	if not fixture is Dictionary:
		push_error("Run natural_spawn_is_safe_seeded_and_supports_sixteen_players first")
		quit(1)
		return
	chest_position = Vector3(fixture.chest[0],fixture.chest[1],fixture.chest[2])
	var previous_save := preload("res://src/app_config.gd").environment("SAVE_PATH")
	var path := ProjectSettings.globalize_path("res://../target/godot-chest-%d/world3d.bin" % Time.get_ticks_usec())
	OS.set_environment("EXA_SAVE_PATH",path)
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.set_process(false)
	scene._focus_gained()
	scene.bridge.set_identity_path(path + ".client.uuid")
	scene.hud.menu.loading_settings = true
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"chest test starts isolated single-player authority")
	for i in range(30):
		scene.bridge.advance(1.0/30.0,{"pickup":true})
		await create_timer(1.0/30.0).timeout
	check(await settle(func(): return scene.state.pending == 0 and not scene.state.inventory.is_empty()),"collect existing world item for storage")
	var bag_key := InputEventKey.new()
	bag_key.pressed = true
	bag_key.keycode = KEY_TAB
	scene._input(bag_key)
	scene._process(0.0)
	var slots_panel = scene.hud.panel
	check(slots_panel.visible and slots_panel.player_slots.size() == 36,"27 inventory slots plus 9 shared hotbar slots")
	slots_panel.player_slots[0].activated.emit("player",0,MOUSE_BUTTON_LEFT,true)
	check(await settle(func(): return not scene.state.inventory_slots[9].is_empty()),"Shift moves hotbar stack into backpack")
	var drag: Dictionary = slots_panel.player_slots[9].payload.duplicate(true)
	slots_panel.player_slots[2]._drop_data(Vector2.ZERO,drag)
	check(await settle(func(): return not scene.state.inventory_slots[2].is_empty() and scene.state.inventory_slots[9].is_empty()),"slot drag/drop routes through server authority")
	slots_panel.player_slots[2].activated.emit("player",2,MOUSE_BUTTON_RIGHT,false)
	scene._process(0.0)
	bag_key.keycode = KEY_ESCAPE
	scene._input(bag_key)
	check(slots_panel.selected.is_empty() and scene.hud.inventory_open and not scene.state.inventory_slots[2].is_empty(),"Escape cancels selection without losing items")
	slots_panel.player_slots[2].activated.emit("player",2,MOUSE_BUTTON_LEFT,false)
	slots_panel.player_slots[0].activated.emit("player",0,MOUSE_BUTTON_LEFT,false)
	check(await settle(func(): return not scene.state.inventory_slots[0].is_empty() and scene.state.inventory_slots[2].is_empty()),"click-to-move returns item to first hotbar slot")
	scene.hud.chat.grab_focus()
	bag_key.keycode = KEY_9
	scene._input(bag_key)
	scene._process(0.0)
	check(scene.state.selected_slot == 0,"chat focus prevents hotbar selection")
	bag_key.keycode = KEY_ESCAPE
	scene._input(bag_key)
	scene._resume_game()
	bag_key.keycode = KEY_9
	scene._input(bag_key)
	check(await settle(func(): return scene.state.selected_slot == 8),"number key selects ninth hotbar slot")
	var old_fov: float = scene.camera.fov
	var wheel := InputEventMouseButton.new()
	wheel.pressed = true
	wheel.button_index = MOUSE_BUTTON_WHEEL_DOWN
	scene._unhandled_input(wheel)
	check(await settle(func(): return scene.state.selected_slot == 0),"wheel cycles hotbar from ninth to first")
	check(scene.camera.fov == old_fov,"hotbar wheel does not alter FOV")
	check(await open_chest(),"crosshair E opens authoritative chest")
	check(not scene.captured and scene.hud.container_panel.visible,"chest panel releases mouse")
	var panel = scene.hud.container_panel
	if scene.hud.container_open and not scene.state.inventory.is_empty():
		panel.player_slots[0].activated.emit("player",0,MOUSE_BUTTON_LEFT,true)
		check(await settle(func(): return not scene.state.container.is_empty() and scene.state.container.items.size() == 1 and scene.state.inventory.is_empty()),"deposit UI transfers item to shared chest")
		panel.chest_slots[0].activated.emit("chest",0,MOUSE_BUTTON_LEFT,true)
		check(await settle(func(): return not scene.state.inventory.is_empty() and scene.state.container.items.is_empty()),"withdraw UI returns item to inventory")
		panel.player_slots[0].activated.emit("player",0,MOUSE_BUTTON_LEFT,true)
		check(await settle(func(): return scene.state.inventory.is_empty()),"deposit again for save/reopen verification")
	var key := InputEventKey.new()
	key.pressed = true
	key.keycode = KEY_ESCAPE
	scene._input(key)
	check(not scene.hud.container_open and not scene.hud.menu.visible,"Escape closes chest before opening game menu")
	check(scene.captured,"closing chest automatically recaptures mouse")
	await settle(func(): return scene.state.container.is_empty())
	check(await open_chest(),"closed chest can reopen")
	scene._focus_lost()
	check(not scene.hud.container_open and not scene.captured,"focus loss closes chest and releases controls")
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"local world saves and closes")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"saved world reopens")
	check(await open_chest(),"persisted chest opens after restarting authority")
	check(scene.state.container.get("items",[]).size() == 1,"chest contents persist across restart")
	panel = scene.hud.container_panel
	panel.chest_slots[0].activated.emit("chest",0,MOUSE_BUTTON_LEFT,true)
	check(await settle(func(): return not scene.state.inventory_slots[0].is_empty()),"withdraw saved item for player persistence")
	scene.bridge.move_inventory(scene.state.inventory_revision,0,35,1)
	check(await settle(func(): return not scene.state.inventory_slots[35].is_empty()),"persistent item occupies exact backpack slot")
	scene.bridge.select_hotbar(8)
	check(await settle(func(): return scene.state.selected_slot == 8),"selected hotbar ready for save")
	scene._close_inventory()
	scene.yaw = 1.1
	scene.pitch = -0.2
	scene.bridge.advance(1.0/30.0,{"yaw":scene.yaw,"pitch":scene.pitch})
	await settle(func(): return scene.state.pending == 0)
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"player checkpoint saves on leaving world")
	scene.hud.player_name.text = "RenamedExplorer"
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"same UUID rejoins after nickname change")
	check(not scene.state.inventory_slots[35].is_empty() and scene.state.selected_slot == 8,"backpack slot and hotbar restore after server restart")
	check(absf(scene.yaw-1.1) < 0.001 and absf(scene.pitch+0.2) < 0.001,"saved camera orientation restores")
	check(scene.state.items.is_empty(),"collected ground item stays collected after restart")
	print("GODOT_CONTAINER_RESULT=", "FAIL" if failed else "PASS")
	print("CONTAINER_TEST_SAVE=",path)
	if preload("res://src/app_config.gd").environment("CHEST_MANUAL") == "1" and not failed:
		scene._close_container()
		scene.set_process(true)
		return
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	OS.set_environment("EXA_SAVE_PATH",previous_save)
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

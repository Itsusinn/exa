extends "res://tests/industry_flow.gd"
## Real server fixture, authoritative controls, and actual rendered thermal UI.
var live_session := false
var diagnostic_printed := false

func diagnostic(message: String) -> void:
	if diagnostic_printed: return
	diagnostic_printed = true
	print("THERMAL_DIAGNOSTIC ",message," status=",scene.state.get("status",""),
		" ready=",scene.state.get("ready",false)," connected=",scene.state.get("connected",false),
		" hp=",scene.state.get("hp",-1)," body=",scene.state.get("body_position"),
		" captured=",scene.captured," focused=",scene.focused," menu=",scene.hud.menu.visible,
		" target=",scene.bridge.equipment_target(scene.yaw,scene.pitch),
		" equipment_count=",scene.state.get("equipment_count",-1),
		" error=",scene.state.get("container_error","")," journal=",scene.state.get("journal",[]))

func check(ok: bool, message: String) -> void:
	super.check(ok,message)
	if not ok and is_instance_valid(scene): diagnostic(message)

func settle(until: Callable) -> bool:
	for i in range(600):
		scene._process(0.0)
		if live_session and (not scene.state.ready or scene.state.hp <= 0):
			diagnostic("session lost or player died while awaiting condition")
			return false
		if until.call(): return true
		await create_timer(0.02).timeout
	return false

func open_checked(current: Dictionary) -> bool:
	if current.is_empty():
		check(false,"equipment exists before interaction")
		return false
	if scene.industry.opened: key(KEY_ESCAPE)
	# This is an injected-input test. Native focus can be stolen by another
	# desktop window while the expensive initial terrain frames are rendered.
	print("THERMAL_INTERACT id=",current.id," focused=",scene.focused," captured=",scene.captured,
		" hp=",scene.state.get("hp",-1)," body=",scene.state.get("body_position")," status=",scene.state.get("status",""))
	scene._focus_gained()
	var offset := Vector3(0.5,0.5,0.5)
	if current.kind in [0,4]: offset = Vector3(1,1.4,1)
	elif current.kind == 5: offset = Vector3(1,0.5,1.5)
	aim(Vector3(current.origin)+offset)
	if not await settle(func(): return scene.captured and scene.bridge.equipment_target(scene.yaw,scene.pitch).get("id",0) == current.id):
		check(false,"equipment is targeted with captured input")
		return false
	key(KEY_E)
	scene._process(0.0)
	var opened: bool = scene.industry.opened and scene.industry.selected_id == current.id
	check(opened,"E opens thermal equipment #%d" % current.id)
	return opened

func at(origin: Vector3i) -> Dictionary:
	for value in scene.bridge.equipment_state():
		if value.origin == origin: return value
	return {}

func teleport(position: Vector3) -> bool:
	if scene.industry.opened: key(KEY_ESCAPE)
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/tp %s %s %s" % [position.x,position.y,position.z])
	var moved := await settle(func(): return scene.state.body_position.distance_to(position) < 0.2)
	check(moved,"teleport to thermal station")
	return moved

func thermal() -> Dictionary:
	return device(5).get("reading",{}).get("thermal",{})

func screenshot(name: String) -> void:
	# The scenario disables automatic processing. Input opens the panel at once,
	# but the main loop must run before the HUD can reflect equipment_open.
	scene._process(0.0)
	if scene.industry.opened:
		check(not scene.hud.reticle.visible,"equipment panel hides reticle: "+name)
		check(not scene.hud.hotbar.visible,"equipment panel hides hotbar: "+name)
		check(not scene.hud.interaction_hint.visible,"equipment panel hides target hint: "+name)
	await super.screenshot(name)

func pump(z: int, enabled: bool) -> bool:
	if not await teleport(Vector3(-1.5,81,13.5 if z == 16 else 21.5)): return false
	var current := at(Vector3i(-2,81,z))
	if not await open_checked(current): return false
	if current.enabled != enabled: scene.industry.pump_switch.pressed.emit()
	var acknowledged := await settle(func(): return at(Vector3i(-2,81,z)).get("enabled",not enabled) == enabled and scene.industry.pending_revision < 0)
	check(acknowledged,"pump state acknowledged")
	key(KEY_ESCAPE)
	return acknowledged

func finish() -> void:
	live_session = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("INDUSTRY_THERMAL_TEST_SAVE=",path)
	print("GODOT_INDUSTRY_THERMAL_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/industry-thermal-fixture-path.txt")
	if not FileAccess.file_exists(pointer):
		push_error("Run industrial_thermal_controls_sync_and_emit_graphics_fixture first")
		quit(1)
		return
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-industry-thermal-%d/world3d.bin" % Time.get_ticks_usec())
	if not copy_directory(source.get_base_dir(),path.get_base_dir()):
		push_error("Cannot copy thermal fixture")
		quit(1)
		return
	OS.set_environment("EXA_SAVE_PATH",path)
	OS.set_environment("EXA_IDENTITY_PATH",path.get_base_dir().path_join("thermal-client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	check(scene.environment.sdfgi_enabled,"thermal scene uses SDFGI")
	scene.set_process(false)
	scene._singleplayer()
	if not await settle(func(): return scene.state.ready):
		check(false,"singleplayer ready")
		await finish()
		return
	scene._focus_gained()
	live_session = true
	if not await teleport(Vector3(0.5,81,13.5)):
		await finish()
		return
	check(await settle(func(): return scene.state.equipment_count == 12 and not thermal().is_empty()),"all thermal equipment and accepted readings arrive")
	if failed:
		await finish()
		return
	check(await settle(func(): return scene.state.loaded_sections == 392 and scene.terrain_pending.is_empty()),"terrain ready for graphics")
	if failed or device(5).is_empty():
		await finish()
		return
	var exchanger := device(5)
	var mesh: Mesh = scene.industry.nodes[exchanger.id].mesh
	var instance_id: int = scene.industry.nodes[exchanger.id].get_instance_id()
	check(exchanger.ports.size() == 4,"exchanger exposes four physical ports")
	if not await open_checked(exchanger):
		await finish()
		return
	check(scene.industry.thermal_controls.visible,"heat controls available")
	check(scene.industry.details.text.contains("两流道"),"panel distinguishes independent flow passages")
	# Incoming samples must not overwrite a locally edited, unapplied draft.
	scene.industry.thermal_target.value = 333.3
	scene.industry.thermal_fraction.value = 42
	await create_timer(0.7).timeout
	scene._process(0.0)
	check(is_equal_approx(scene.industry.thermal_target.value,333.3) and is_equal_approx(scene.industry.thermal_fraction.value,42),"running sample updates preserve control draft")
	scene.industry.thermal_reload.pressed.emit()
	check(is_equal_approx(scene.industry.thermal_target.value,exchanger.thermal.target_temperature_k),"explicit reload restores authority settings")
	if failed:
		await finish()
		return
	scene.industry.thermal_mode.select(1)
	scene.industry.thermal_fraction.value = 70
	scene.industry.thermal_apply.pressed.emit()
	check(await settle(func(): return device(5).get("thermal",{}).get("mode",-1) == 1 and scene.industry.pending_revision < 0),"manual thermal control acknowledged")
	if failed:
		await finish()
		return
	key(KEY_ESCAPE)
	if not await pump(16,true) or not await pump(18,true):
		await finish()
		return
	check(await settle(func(): return absf(thermal().get("heat_w",0.0)) > 1.0),"two running streams exchange heat")
	if failed or not await teleport(Vector3(0.5,81,13.5)):
		await finish()
		return
	if not await open_checked(device(5)):
		await finish()
		return
	check(scene.industry.thermal_facts.text.contains("A 橙") and scene.industry.thermal_facts.text.contains("B 蓝"),"both stream observations shown")
	await screenshot("thermal-manual.png")
	scene.industry.thermal_reload.pressed.emit()
	scene.industry.thermal_mode.select(2)
	scene.industry.thermal_target.value = 340
	scene.industry.thermal_apply.pressed.emit()
	check(await settle(func(): return device(5).get("thermal",{}).get("mode",-1) == 2 and scene.industry.pending_revision < 0),"outlet temperature control acknowledged")
	if failed:
		await finish()
		return
	check(await settle(func(): return thermal().get("quality","") == "available" and thermal().get("control_state","") in ["regulating","limited"]),"outlet control publishes accepted operating state")
	if failed:
		await finish()
		return
	scene.industry.scroll_area.scroll_vertical = 1000
	await screenshot("thermal-control.png")
	if not scene.industry.nodes.has(exchanger.id):
		check(false,"exchanger remains visible")
		await finish()
		return
	check(scene.industry.nodes[exchanger.id].mesh == mesh and scene.industry.nodes[exchanger.id].get_instance_id() == instance_id,"heat readings and control reuse geometry")
	key(KEY_ESCAPE)
	if not await teleport(Vector3(-5,81,25.5)):
		await finish()
		return
	var vessel := device(4)
	if not await open_checked(vessel):
		await finish()
		return
	check(scene.industry.thermal_facts.text.contains("气相摩尔分率") and scene.industry.thermal_facts.text.contains("正戊烷"),"two-phase component inventory visible")
	scene._process(0.0)
	await process_frame
	scene.industry.scroll_area.ensure_control_visible(scene.industry.thermal_facts)
	await screenshot("vessel-phases.png")
	key(KEY_ESCAPE)
	if not await pump(16,false) or not await pump(18,false):
		await finish()
		return
	check(await settle(func(): return absf(thermal().get("heat_w",NAN)) < 0.000001),"stopped flow ceases heat exchange")
	if failed or not await teleport(Vector3(0.5,81,11)):
		await finish()
		return
	aim(Vector3(0,81.8,18))
	await screenshot("thermal-circuit.png")
	var expected: Dictionary = device(5).get("thermal",{}).duplicate(true)
	live_session = false
	scene._leave_game(false)
	check(await settle(func(): return scene.pending_action.is_empty()),"save and clean shutdown")
	check(scene.industry.nodes.is_empty(),"thermal instances released on disconnect")
	scene._singleplayer()
	check(await settle(func(): return scene.state.ready),"saved thermal world reopens")
	if failed:
		await finish()
		return
	live_session = true
	if not await teleport(Vector3(0.5,81,13.5)):
		await finish()
		return
	check(await settle(func(): return scene.state.equipment_count == 12),"saved devices restored")
	check(device(5).get("thermal",{}) == expected,"heat control settings survive restart")
	check(device(4).get("contents",[]).size() == 2,"both sealed-vessel components survive restart")
	await finish()

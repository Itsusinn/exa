extends "res://tests/industry_flow.gd"
## Isolated singleplayer with the real bounded server calculation service.
var connected_test := false

func check_panel_layout(context: String) -> void:
	for i in range(3): await process_frame
	var viewport: Rect2 = scene.get_viewport().get_visible_rect()
	check(viewport.encloses(scene.distillation.panel.get_global_rect()),context+": panel stays inside viewport")
	check(viewport.encloses(scene.distillation.close_button.get_global_rect()),context+": close button stays inside viewport")
	check(scene.distillation.panel.get_global_rect().encloses(scene.distillation.close_button.get_global_rect()),context+": close button stays inside panel")

func settle(until: Callable) -> bool:
	for i in range(3000):
		scene._process(0.0)
		if connected_test and (not scene.state.ready or scene.state.hp <= 0): return false
		if until.call(): return true
		await create_timer(0.02).timeout
	return false

func expect(until: Callable, message: String) -> bool:
	var ok := await settle(until)
	check(ok,message)
	if not ok:
		print("DISTILLATION_DIAGNOSTIC state=",scene.bridge.distillation_state()," ready=",scene.state.ready," hp=",scene.state.hp," status=",scene.state.status)
		await finish()
	return ok

func finish() -> void:
	connected_test = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("DISTILLATION_TEST_SAVE=",path)
	print("GODOT_DISTILLATION_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	path = ProjectSettings.globalize_path("res://../target/godot-distillation-%d/world3d.bin" % Time.get_ticks_usec())
	DirAccess.make_dir_recursive_absolute(path.get_base_dir())
	OS.set_environment("CDDA_SAVE_PATH",path)
	OS.set_environment("CDDA_CERT_PATH",path.get_base_dir().path_join("server.cert.der"))
	OS.set_environment("CDDA_IDENTITY_PATH",path.get_base_dir().path_join("design-client.uuid"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	scene.set_process(false)
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"isolated design world starts"): return
	connected_test = true
	scene._focus_gained()
	scene.bridge.send_chat("/fly on")
	if not await expect(func(): return scene.state.flying,"safe suspended test position enables flight"): return
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/tp 0.5 120 0.5")
	if not await expect(func(): return scene.state.body_position.distance_to(Vector3(0.5,120,0.5)) < 0.2,"test leaves enemies below"): return
	if not await expect(func(): return scene.terrain_pending.is_empty(),"initial geometry settles"): return
	scene._focus_gained()
	Input.action_press("forward")
	scene.attack = true
	key(KEY_J)
	scene._process(0.0)
	check(scene.distillation.opened and not scene.captured,"J opens design workspace and releases mouse")
	check(not Input.is_action_pressed("forward") and not scene.attack,"opening clears world actions")
	check(not scene.hud.reticle.visible and not scene.hud.hotbar.visible,"design workspace hides world interaction HUD")
	var start: Vector3 = scene.state.body_position
	var count_before: int = scene.state.equipment_count
	var bag_before: Array = scene.state.inventory_slots.duplicate(true)
	var click := InputEventMouseButton.new()
	click.pressed = true
	click.button_index = MOUSE_BUTTON_RIGHT
	scene._unhandled_input(click)
	scene._process(0.1)
	check(scene.state.body_position.distance_to(start) < 0.05 and scene.state.equipment_count == count_before,"panel input cannot move or construct")
	if failed:
		await finish()
		return
	await check_panel_layout("inputs")
	await screenshot("distillation-inputs.png")
	scene.distillation.calculate.pressed.emit()
	check(scene.bridge.distillation_state().pending,"accepted queue submission enters pending")
	# Cancel before consuming the server response; even a fast completion is late.
	scene.distillation.cancel.pressed.emit()
	check(not scene.bridge.distillation_state().pending,"cancel immediately retires local request")
	await create_timer(0.8).timeout
	scene._process(0.0)
	check(not scene.bridge.distillation_state().has("result"),"cancelled request cannot publish a late result")
	check(scene.distillation.status.text.contains("取消"),"cancelled status is visible")
	await screenshot("distillation-cancelled.png")
	key(KEY_ESCAPE)
	check(not scene.distillation.opened and not scene.hud.menu.visible,"Escape closes design before pause menu")
	key(KEY_J)
	scene.distillation.calculate.pressed.emit()
	# The editable draft may change while the immutable submitted input computes.
	scene.distillation.fields.reflux_ratio.value = 3
	if not await expect(func(): return not scene.bridge.distillation_state().pending,"default design calculation terminates"): return
	var reply: Dictionary = scene.bridge.distillation_state()
	check(reply.get("status","") == "completed","real PR78 design succeeds")
	if failed:
		print("DISTILLATION_DIAGNOSTIC ",reply)
		await finish()
		return
	var result: Dictionary = reply.result
	check(result.stages.size() == result.spec.trays+1,"table includes trays and separate equilibrium reboiler")
	check(result.distillate_butane_fraction > result.spec.butane_fraction and result.bottoms_butane_fraction < result.spec.butane_fraction,"products separate the light component")
	check(scene.distillation.stages.get_root().get_child_count() == result.stages.size(),"stage table displays every returned stage")
	check(scene.distillation.summary.text.contains("馏出液") and scene.distillation.summary.text.contains("冷凝器"),"products and thermal duties are visible")
	check(scene.state.equipment_count == count_before and scene.state.inventory_slots == bag_before,"design calculation does not consume equipment or inventory")
	check(scene.distillation.fields.reflux_ratio.value == 3,"completed reply cannot overwrite edits made during calculation")
	check(scene.distillation.result_note.text.contains("参数已修改"),"editing marks the result as belonging to old parameters")
	check(scene.bridge.distillation_state().result.spec.reflux_ratio == 2,"editing cannot rewrite accepted result inputs")
	scene.distillation.fields.reflux_ratio.value = 2
	if failed:
		await finish()
		return
	await check_panel_layout("completed result")
	await screenshot("distillation-result.png")
	check(scene.distillation.scroll.get_v_scroll_bar().max_value > scene.distillation.scroll.get_v_scroll_bar().page,"long results expose a vertical scroll range")
	scene.distillation.scroll.scroll_vertical = int(scene.distillation.scroll.get_v_scroll_bar().max_value)
	for i in range(3): await process_frame
	check(scene.distillation.scroll.get_global_rect().encloses(scene.distillation.audits.get_global_rect()),"scrolling to bottom exposes the full residual audit")
	check(scene.distillation.audits.text.contains("×10^"),"small nonzero residuals retain scientific precision")
	check(scene.distillation._scientific(0.00000000001) != "0","small residual is never rounded into a false exact zero")
	await check_panel_layout("scrolled audit")
	await screenshot("distillation-audit.png")
	scene.distillation.scroll.scroll_vertical = 0
	# At atmospheric pressure a strongly butane-rich product needs a condenser
	# below the package's 293.15 K lower limit; this must be an honest failure.
	scene.distillation.fields.pressure_pa.value = 100
	scene.distillation.fields.butane_fraction.value = 0.99
	scene.distillation.calculate.pressed.emit()
	if not await expect(func(): return not scene.bridge.distillation_state().pending,"unsupported design terminates with diagnostic"): return
	check(scene.bridge.distillation_state().get("status","") == "failed","no converged out-of-domain result is published")
	check(not scene.bridge.distillation_state().has("result") and not scene.distillation.status.text.is_empty(),"failure shows a diagnostic without partial stages")
	if failed:
		print("DISTILLATION_DIAGNOSTIC ",scene.bridge.distillation_state())
		await finish()
		return
	check(scene.distillation.status.text.contains("诊断：") and scene.distillation.status.text.contains("气液平衡"),"scientific failure explains the cause in Chinese and preserves the diagnostic")
	await check_panel_layout("failure")
	await screenshot("distillation-failure.png")
	scene.distillation.fields.pressure_pa.value = 400
	scene.distillation.fields.butane_fraction.value = 0.5
	scene.distillation.calculate.pressed.emit()
	scene._focus_lost()
	check(not scene.distillation.opened and not scene.bridge.distillation_state().pending,"focus loss closes and cancels design")
	scene._focus_gained()
	await create_timer(0.8).timeout
	scene._process(0.0)
	check(not scene.bridge.distillation_state().has("result"),"closed workspace ignores later replies")
	key(KEY_J)
	check(not scene.bridge.distillation_state().has("result"),"reopened workspace starts without stale reports")
	key(KEY_TAB)
	check(not scene.distillation.opened and scene.hud.inventory_open,"Tab switches to inventory with design closed")
	key(KEY_TAB)
	await finish()

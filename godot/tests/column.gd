extends "res://tests/industry_design.gd"

func finish() -> void:
	live_session = false
	scene._leave_game(false)
	await settle(func(): return scene.pending_action.is_empty())
	print("COLUMN_TEST_SAVE=",path)
	print("GODOT_COLUMN_RESULT=", "FAIL" if failed else "PASS")
	scene.queue_free()
	await process_frame
	quit(1 if failed else 0)

func run() -> void:
	var pointer := ProjectSettings.globalize_path("res://../target/column-fixture-path.txt")
	var source := FileAccess.get_file_as_string(pointer).strip_edges().trim_prefix("\\\\?\\").replace("\\","/")
	path = ProjectSettings.globalize_path("res://../target/godot-column-%d/world3d.bin" % Time.get_ticks_usec())
	if source.is_empty() or not copy_directory(source.get_base_dir(),path.get_base_dir()):
		quit(1)
		return
	OS.set_environment("EXA_SAVE_PATH",path)
	OS.set_environment("EXA_IDENTITY_PATH",path.get_base_dir().path_join("column-client.uuid"))
	OS.set_environment("EXA_COLUMN_DESIGN_PATH",path.get_base_dir().path_join("design.json"))
	scene = load("res://scenes/main.tscn").instantiate()
	root.add_child(scene)
	current_scene = scene
	scene.hud.menu.loading_settings = true
	scene.hud.gi_toggle.button_pressed = true
	scene.set_process(false)
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"column world ready"): return
	live_session = true
	scene._focus_gained()
	if not await teleport(Vector3(12.5,81,13.5)):
		await finish()
		return
	if not await expect(func(): return scene.terrain_pending.is_empty(),"column terrain loaded"): return
	key(KEY_J)
	scene._process(0.0)
	check(scene.distillation.opened,"J opens column design")
	scene.distillation.fields.diameter.value = 3
	var proposed: Dictionary = scene.distillation.column_draft()
	check(scene.bridge.validate_column_design(proposed),"shared Rust validation accepts structural design")
	if failed:
		print("COLUMN_DESIGN_DIAGNOSTIC proposed=",proposed)
		await finish()
		return
	var invalid := proposed.duplicate(true)
	invalid.result = 1
	check(not scene.bridge.validate_column_design(invalid),"import rejects result and unknown fields")
	invalid = proposed.duplicate(true)
	invalid.thermo_revision = "unsupported"
	check(not scene.bridge.validate_column_design(invalid),"import rejects incompatible property revision")
	invalid = proposed.duplicate(true)
	invalid.spec.trays = 6.5
	check(not scene.bridge.validate_column_design(invalid),"import rejects fractional tray counts")
	scene.distillation._save_design()
	check(FileAccess.file_exists(scene.distillation.design_path()),"design saved in isolated directory")
	if failed:
		print("COLUMN_SAVE_DIAGNOSTIC ",scene.distillation.design_notice.text)
		await finish()
		return
	scene.distillation.fields.diameter.value = 2
	scene.distillation._save_design()
	check(scene.distillation.read_saved_design().get("diameter",0) == 2,"saving replaces previous design successfully")
	if failed:
		print("COLUMN_SAVE_DIAGNOSTIC ",scene.distillation.design_notice.text)
		await finish()
		return
	scene.distillation.fields.diameter.value = 3
	scene.distillation._save_design()
	scene.distillation.fields.diameter.value = 5
	scene.distillation._load_design()
	check(scene.distillation.fields.diameter.value == 3,"saved design restores independent draft")
	var saved := FileAccess.get_file_as_string(scene.distillation.design_path())
	var file := FileAccess.open(scene.distillation.design_path(),FileAccess.WRITE)
	file.store_string('{"schema":1,"design":{"result":123}}')
	file.close()
	scene.distillation._load_design()
	check(scene.distillation.fields.diameter.value == 3,"invalid import preserves draft")
	file = FileAccess.open(scene.distillation.design_path(),FileAccess.WRITE)
	file.store_string(saved)
	file.close()
	var bad_revision := proposed.duplicate(true)
	bad_revision.thermo_revision = "unknown"
	var bad_range := proposed.duplicate(true)
	bad_range.spec.pressure_pa = 9e9
	for bad in [JSON.stringify({"schema":2,"design":proposed}),JSON.stringify({"schema":1,"design":bad_revision}),JSON.stringify({"schema":1,"design":bad_range})," ".repeat(65537),'{"schema":1,"design":{"diameter":NaN}}']:
		file = FileAccess.open(scene.distillation.design_path(),FileAccess.WRITE)
		file.store_string(bad)
		file.close()
		scene.distillation._load_design()
		check(scene.distillation.column_draft() == proposed,"invalid file leaves entire draft unchanged")
	file = FileAccess.open(scene.distillation.design_path(),FileAccess.WRITE)
	file.store_string(saved)
	file.close()
	scene.distillation._use_design()
	if not scene.bridge.column_build_design().has("diameter"):
		check(false,"construction design prepared")
		print("COLUMN_USE_DIAGNOSTIC ",scene.distillation.design_notice.text)
		await finish()
		return
	check(scene.bridge.column_build_design().diameter == 3,"building design frozen without numerical solve")
	scene.distillation.fields.diameter.value = 4
	check(scene.bridge.column_build_design().diameter == 3,"editing draft cannot mutate construction design")
	await screenshot("column-design.png")
	key(KEY_ESCAPE)
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/industry kit")
	if not await expect(func(): return slot("蒸馏塔") >= 0,"kit supplies column"): return
	scene._select_hotbar(slot("蒸馏塔"))
	if not await expect(func(): return scene.bridge.column_selected(),"column selected"): return
	key(KEY_B)
	scene._process(0.0)
	check(scene.industry.build_opened,"B opens frozen column construction")
	check(scene.industry.build_capacity.text.contains("直径 3"),"B displays frozen dimensions")
	await screenshot("column-construction.png")
	scene.industry.build_apply.pressed.emit()
	aim(Vector3(12.5,80.99,16.5))
	if not await expect(func(): return scene.industry.placement.get("valid",false),"column preview valid"): return
	var preview: Dictionary = scene.industry.placement
	var preview_mesh: Mesh = scene.industry.preview.mesh
	check(preview.diameter == 3 and preview.height == 8,"preview uses design dimensions")
	await screenshot("column-preview.png")
	click(MOUSE_BUTTON_RIGHT)
	if not await expect(func(): return not at(Vector3i(12,81,16)).is_empty(),"column placed atomically"): return
	var built := at(Vector3i(12,81,16))
	check(built.column_design == scene.bridge.column_build_design(),"authority stores frozen design")
	check(scene.industry.nodes[built.id].mesh == preview_mesh,"placed column reuses preview geometry")
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/fly on")
	if not await expect(func(): return scene.state.flying,"flight enables overview without falling"): return
	if not await teleport(Vector3(18,85,10)):
		await finish()
		return
	aim(Vector3(13.5,85,17.5))
	await screenshot("column-overview.png")
	if not await teleport(Vector3(0.5,81,13.5)):
		await finish()
		return
	if not await open_checked(at(Vector3i(0,81,16))):
		await finish()
		return
	check(scene.industry.details.text.contains("未运行"),"column panel describes nonoperating boundary")
	scene.industry.assembly_button.pressed.emit()
	if not await expect(func(): return scene.bridge.assembly_report().get("status","") == "complete","three vessel external connections pass"): return
	check(scene.bridge.assembly_report().lines.size() == 3,"column report has three distinct lines")
	for i in range(3): await process_frame
	check(scene.get_viewport().get_visible_rect().encloses(scene.industry.panel.get_global_rect()),"column inspection panel remains inside viewport")
	check(scene.get_viewport().get_visible_rect().encloses(scene.industry.panel_close.get_global_rect()),"inspection close button remains visible")
	await screenshot("column-connections.png")
	scene.industry.scroll_area.scroll_vertical = int(scene.industry.scroll_area.get_v_scroll_bar().max_value)
	for i in range(3): await process_frame
	check(scene.industry.scroll_area.get_global_rect().encloses(scene.industry.assembly_facts.get_global_rect()),"all three connection lines are visible after scrolling")
	check(scene.get_viewport().get_visible_rect().encloses(scene.industry.panel_close.get_global_rect()),"close button remains visible below full connection report")
	await screenshot("column-connections-detail.png")
	key(KEY_ESCAPE)
	live_session = false
	scene._leave_game(false)
	if not await expect(func(): return scene.pending_action.is_empty(),"column world saved"): return
	scene._singleplayer()
	if not await expect(func(): return scene.state.ready,"column world reopened"): return
	live_session = true
	if not await teleport(Vector3(12.5,81,13.5)):
		await finish()
		return
	if not await expect(func(): return not at(Vector3i(12,81,16)).is_empty(),"built column restored"): return
	check(at(Vector3i(12,81,16)).id == built.id and at(Vector3i(12,81,16)).column_design == built.column_design,"column stable identity and design survive save")
	check(not scene.bridge.assembly_report().has("master_id"),"derived report not restored")
	await create_timer(0.6).timeout
	scene.bridge.send_chat("/industry kit")
	if not await expect(func(): return slot("蒸馏塔") >= 0,"reopened world has column item"): return
	scene._select_hotbar(slot("蒸馏塔"))
	if not await expect(func(): return scene.bridge.column_selected(),"column selected after reopen"): return
	check(scene.bridge.column_build_design().is_empty(),"new session has no implicit construction design")
	key(KEY_B)
	check(scene.industry.build_opened,"B opens before J in fresh session")
	scene.industry.build_import.pressed.emit()
	check(scene.bridge.column_build_design().get("diameter",0) == 3,"B imports persisted design without opening J")
	await finish()

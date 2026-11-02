extends CanvasLayer
## Presentation-only design workspace. Scientific calculation stays in Rust.
const Palette = preload("res://src/interface_theme.gd")
signal close_requested
var opened := false
var host_view: Node3D
var panel: Panel
var scroll: ScrollContainer
var close_button: Button
var fields: Dictionary = {}
var inputs: GridContainer
var status: Label
var result_note: Label
var summary: Label
var audits: Label
var stages: Tree
var calculate: Button
var cancel: Button
var draft_loaded := false
var report_key := ""
var result_spec: Dictionary = {}
var last_size := Vector2.ZERO
var design_notice: Label

func _ready() -> void:
	layer = 6
	# A plain Panel deliberately does not inherit its contents' minimum size.
	# Header and footer stay anchored while only the middle region scrolls.
	panel = Panel.new()
	panel.theme = Palette.create()
	panel.add_theme_stylebox_override("panel",panel.theme.get_stylebox("panel","PanelContainer"))
	add_child(panel)
	var title := Label.new()
	title.text = "二元蒸馏设计器"
	title.add_theme_font_size_override("font_size",24)
	panel.add_child(title)
	title.position = Vector2(28,24)
	scroll = ScrollContainer.new()
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	scroll.vertical_scroll_mode = ScrollContainer.SCROLL_MODE_AUTO
	panel.add_child(scroll)
	scroll.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	scroll.offset_left = 28
	scroll.offset_right = -28
	scroll.offset_top = 76
	scroll.offset_bottom = -96
	var content := VBoxContainer.new()
	content.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content.add_theme_constant_override("separation",12)
	scroll.add_child(content)
	var description := Label.new()
	description.text = "正丁烷 / 正戊烷 · 单进料 · 恒压 · 全凝器与平衡再沸器\n仅计算设计，不消耗或改变世界物料。理想板由塔顶向下编号，再沸器另列。"
	description.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	content.add_child(description)
	inputs = GridContainer.new()
	inputs.columns = 4
	inputs.add_theme_constant_override("h_separation",16)
	inputs.add_theme_constant_override("v_separation",8)
	content.add_child(inputs)
	_number("trays","理想板数",2,12,1)
	_number("feed_tray","进料板",1,12,1)
	_number("feed_mol_s","进料 (mol/s)",0.001,1000,0.001)
	_number("butane_fraction","进料丁烷摩尔分率",0.01,0.99,0.01)
	_number("feed_temperature_k","进料温度 (K)",293.15,400,0.05)
	_number("pressure_pa","压力 (kPa)",100,2000,1)
	_number("reflux_ratio","回流比",0.05,20,0.05)
	_number("distillate_fraction","馏出量 / 进料量",0.01,0.99,0.01)
	_number("diameter","建造塔径 (m)",2,6,1)
	fields.diameter.value = 2
	var design_buttons := HBoxContainer.new()
	content.add_child(design_buttons)
	for entry in [["保存设计",_save_design],["读取保存设计",_load_design],["用于建造",_use_design]]:
		var button := Button.new()
		button.text = entry[0]
		button.pressed.connect(entry[1])
		design_buttons.add_child(button)
	design_notice = Label.new()
	design_notice.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	design_notice.text = "保存结构参数无需计算成功；塔径只决定几何，不代表热工认证。塔体暂不运行。"
	content.add_child(design_notice)
	fields.trays.value_changed.connect(func(value): fields.feed_tray.max_value = value)
	var buttons := HBoxContainer.new()
	buttons.add_theme_constant_override("separation",12)
	content.add_child(buttons)
	calculate = Button.new()
	calculate.text = "计算设计"
	calculate.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	calculate.pressed.connect(_calculate)
	buttons.add_child(calculate)
	cancel = Button.new()
	cancel.text = "取消计算"
	cancel.pressed.connect(_cancel)
	buttons.add_child(cancel)
	status = Label.new()
	status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	content.add_child(status)
	result_note = Label.new()
	result_note.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	result_note.add_theme_color_override("font_color",Color(0.96,0.78,0.45))
	content.add_child(result_note)
	summary = Label.new()
	summary.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	content.add_child(summary)
	stages = Tree.new()
	stages.columns = 6
	stages.hide_root = true
	stages.column_titles_visible = true
	stages.custom_minimum_size.y = 200
	stages.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	for i in range(6):
		stages.set_column_title(i,["级","温度 K","液相丁烷 x","气相丁烷 y","液流 mol/s","汽流 mol/s"][i])
		stages.set_column_expand(i,true)
		stages.set_column_custom_minimum_width(i,70 if i == 0 else 110)
	content.add_child(stages)
	audits = Label.new()
	audits.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	audits.add_theme_font_size_override("font_size",14)
	content.add_child(audits)
	close_button = Button.new()
	close_button.text = "关闭 · Esc"
	close_button.pressed.connect(func(): close_requested.emit())
	panel.add_child(close_button)
	close_button.set_anchors_and_offsets_preset(Control.PRESET_BOTTOM_WIDE)
	close_button.offset_left = 28
	close_button.offset_right = -28
	close_button.offset_top = -84
	close_button.offset_bottom = -28
	panel.hide()

func _number(field: String, caption: String, minimum: float, maximum: float, step: float) -> void:
	var label := Label.new()
	label.text = caption
	inputs.add_child(label)
	var number := SpinBox.new()
	number.min_value = minimum
	number.max_value = maximum
	number.step = step
	number.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	number.custom_minimum_size.x = 130
	number.value_changed.connect(func(_value): _describe_draft())
	inputs.add_child(number)
	fields[field] = number

func show_design(host: Node3D) -> bool:
	if not host.bridge.open_distillation(): return false
	host_view = host
	if not draft_loaded:
		var defaults: Dictionary = host.bridge.distillation_defaults()
		for name in fields:
			if name == "diameter": continue
			fields[name].value = defaults[name] / 1000.0 if name == "pressure_pa" else defaults[name]
		draft_loaded = true
	opened = true
	report_key = ""
	result_spec.clear()
	scroll.scroll_vertical = 0
	panel.show()
	update_view(host)
	return true

func draft() -> Dictionary:
	return {"trays":int(fields.trays.value),"feed_tray":int(fields.feed_tray.value),
		"feed_mol_s":fields.feed_mol_s.value,"butane_fraction":fields.butane_fraction.value,
		"feed_temperature_k":fields.feed_temperature_k.value,"pressure_pa":fields.pressure_pa.value*1000.0,
		"reflux_ratio":fields.reflux_ratio.value,"distillate_fraction":fields.distillate_fraction.value}

func _calculate() -> void:
	if not opened: return
	host_view.bridge.calculate_distillation(draft())
	update_view(host_view)

func _cancel() -> void:
	if not opened: return
	host_view.bridge.cancel_distillation()
	update_view(host_view)

func _describe_draft() -> void:
	if not is_instance_valid(result_note): return
	if result_spec.is_empty():
		result_note.text = ""
		return
	var values := draft()
	var changed := false
	for name in result_spec:
		if not is_equal_approx(float(values[name]),float(result_spec[name])): changed = true
	result_note.text = "参数已修改；下方结果属于上次提交的参数。重新计算后才会更新。" if changed else "结果对应当前参数。"

func update_view(host: Node3D) -> void:
	if not opened: return
	if not host.state.get("ready",false) or host.state.get("hp",0) <= 0 or host.state.get("demo",false):
		close_requested.emit()
		return
	var viewport_size := host.get_viewport().get_visible_rect().size
	if viewport_size != last_size:
		last_size = viewport_size
		var size := Vector2(minf(1000,viewport_size.x-40),minf(700,viewport_size.y-40))
		panel.position = (viewport_size-size)/2
		panel.size = size
		inputs.columns = 4 if size.x >= 800 else 2
	var reply: Dictionary = host.bridge.distillation_state()
	calculate.disabled = reply.get("pending",false)
	cancel.disabled = not reply.get("pending",false)
	var key := "%s:%s:%s:%s" % [reply.get("request_id",0),reply.get("status",""),reply.get("pending",false),reply.get("local_message","")]
	if key != report_key:
		report_key = key
		var names := {"running":"计算中…","completed":"计算完成","cancelled":"请求已取消","rejected":"请求未执行","failed":"未得到有效设计"}
		status.text = str(names.get(reply.get("status",""),"正在提交…" if reply.get("pending",false) else "输入参数后点击计算设计。"))
		if not reply.get("message","").is_empty():
			status.text += "\n"+(_failure_message(reply.message) if reply.get("status","") == "failed" else str(reply.message))
		if not reply.get("local_message","").is_empty(): status.text = reply.local_message
		_render_result(reply.get("result",{}))
	_describe_draft()

func _render_result(result: Dictionary) -> void:
	stages.clear()
	result_spec.clear()
	summary.text = ""
	audits.text = ""
	stages.visible = not result.is_empty()
	if result.is_empty(): return
	result_spec = result.spec.duplicate()
	var spec: Dictionary = result.spec
	summary.text = "已计算：%d 板，进料板 %d · F %.6f mol/s · z丁烷 %.4f · %.2f K · %.1f kPa · 回流比 %.3f · D/F %.3f\n" % [spec.trays,spec.feed_tray,spec.feed_mol_s,spec.butane_fraction,spec.feed_temperature_k,spec.pressure_pa/1000.0,spec.reflux_ratio,spec.distillate_fraction]
	summary.text += "馏出液：%.6f mol/s · 丁烷 %.5f / 戊烷 %.5f\n塔底液：%.6f mol/s · 丁烷 %.5f / 戊烷 %.5f\n" % [result.distillate_mol_s,result.distillate_butane_fraction,1-result.distillate_butane_fraction,result.bottoms_mol_s,result.bottoms_butane_fraction,1-result.bottoms_butane_fraction]
	summary.text += "冷凝器：%.2f K · 移出热量 %.4f kW    再沸器：加入热量 %.4f kW" % [result.condenser_temperature_k,result.condenser_duty_w/1000.0,result.reboiler_duty_w/1000.0]
	var root := stages.create_item()
	for tray in result.stages:
		var row := stages.create_item(root)
		row.set_text(0,"再沸器" if tray.number > spec.trays else str(tray.number))
		row.set_text(1,"%.2f" % tray.temperature_k)
		row.set_text(2,"%.5f" % tray.liquid_butane_fraction)
		row.set_text(3,"%.5f" % tray.vapor_butane_fraction)
		row.set_text(4,"%.6f" % tray.liquid_mol_s)
		row.set_text(5,"%.6f" % tray.vapor_mol_s)
	audits.text = "独立复核最大残差：组分 %s mol/s · 能量 %s W · 相平衡 %s\n迭代 %d 次；结果仅适用于当前有限二元物性与理想级模型。" % [_scientific(result.max_component_residual_mol_s),_scientific(result.max_energy_residual_w),_scientific(result.max_equilibrium_residual),result.iterations]

func _scientific(value: float) -> String:
	if value == 0.0: return "0"
	var exponent := int(floor(log(absf(value))/log(10.0)))
	return "%.3f ×10^%d" % [value/pow(10.0,exponent),exponent]

func _failure_message(diagnostic: String) -> String:
	var explanation := "当前参数未得到满足守恒与相平衡的有效设计，请调整压力、回流比或产品比例后重试。"
	if diagnostic.contains("bubble") or diagnostic.contains("temperature") or diagnostic.contains("equilibrium phases"):
		explanation = "当前压力与组成未能在物性范围 293.15–400 K 内找到有效气液平衡，请调整压力或组成后重试。"
	elif diagnostic.contains("time budget"):
		explanation = "计算达到时间上限，请减少理想板数或调整参数后重试。"
	elif diagnostic.contains("audit"):
		explanation = "计算结果未通过独立守恒或相平衡复核，因此未发布结果。"
	return explanation+"\n诊断："+diagnostic

func close() -> void:
	if is_instance_valid(host_view): host_view.bridge.close_distillation()
	opened = false
	panel.hide()
	report_key = ""
	result_spec.clear()

func reset() -> void:
	close()
	draft_loaded = false

func design_path() -> String:
	var override := OS.get_environment("CDDA_COLUMN_DESIGN_PATH")
	return override if not override.is_empty() else "user://column-design.json"

func column_draft() -> Dictionary:
	return {"diameter":int(fields.diameter.value),"thermo_revision":"butane-pentane-pr78-cp298-r1","spec":draft()}

func _save_design() -> void:
	var design := column_draft()
	if not host_view.bridge.validate_column_design(design):
		design_notice.text = "设计参数无效，未保存。"
		return
	var path := design_path()
	var file := FileAccess.open(path+".tmp",FileAccess.WRITE)
	if file == null:
		design_notice.text = "设计文件无法写入。"
		return
	file.store_string(JSON.stringify({"schema":1,"design":design}))
	file.flush()
	var error := file.get_error()
	file.close()
	if error != OK or DirAccess.rename_absolute(ProjectSettings.globalize_path(path+".tmp"),ProjectSettings.globalize_path(path)) != OK:
		design_notice.text = "保存失败，原设计保留。"
		return
	design_notice.text = "已保存结构设计："+ProjectSettings.globalize_path(path)

func read_saved_design() -> Dictionary:
	var file := FileAccess.open(design_path(),FileAccess.READ)
	if file == null: return {}
	if file.get_length() > 65536: return {}
	var parser := JSON.new()
	if parser.parse(file.get_as_text()) != OK: return {}
	var decoded = parser.data
	if not decoded is Dictionary or decoded.size() != 2 or decoded.get("schema",0) != 1 or not decoded.get("design") is Dictionary: return {}
	if not host_view.bridge.validate_column_design(decoded.design): return {}
	return decoded.design

func _load_design() -> void:
	var design := read_saved_design()
	if design.is_empty():
		design_notice.text = "无法读取有效设计；原草稿保留。"
		return
	# Update trays first so the feed selector uses the imported range.
	fields.trays.value = design.spec.trays
	for name in design.spec:
		fields[name].value = design.spec[name]/1000.0 if name == "pressure_pa" else design.spec[name]
	fields.diameter.value = design.diameter
	design_notice.text = "已读取结构设计；计算结果仍独立，不代表热工验证。"

func _use_design() -> void:
	if host_view.bridge.use_column_design(column_draft()):
		design_notice.text = "建造设计已冻结。持蒸馏塔按 B 查看，右键放置；当前塔体不运行。"
	else: design_notice.text = "设计参数无效，未用于建造。"

extends Node3D
## Presentation only. Geometry, targeting and placement validation come from Rust.
const Palette = preload("res://src/interface_theme.gd")
const PumpDirection = preload("res://src/pump_direction.gd")
var nodes: Dictionary = {}
var arrows: Dictionary = {}
var arrow_mesh: ArrayMesh
var preview_arrow: MeshInstance3D
var meshes: Dictionary = {}
var records: Dictionary = {}
var last_revision := -1
var preview: MeshInstance3D
var preview_material: StandardMaterial3D
var metal: StandardMaterial3D
var panel: PanelContainer
var details: Label
var selected_id := 0
var opened := false
var placement: Dictionary = {}
var pump_controls: HBoxContainer
var valve_controls: HBoxContainer
var pump_switch: Button
var pump_direction: Button
var valve_buttons: Array[Button] = []
var feedback: Label
var host_view: Node3D
var pending_revision := -1
var pending_since := 0
var thermal_controls: VBoxContainer
var thermal_mode: OptionButton
var thermal_fraction: SpinBox
var thermal_target: SpinBox
var thermal_apply: Button
var thermal_reload: Button
var thermal_facts: Label
var draft_revision := -1
var scroll_area: ScrollContainer
var pending_thermal: Dictionary = {}
var route_state: Dictionary = {}
var route_nodes: Array[MeshInstance3D] = []
var route_revision := -1
var route_start: MeshInstance3D
var route_target: MeshInstance3D
signal close_requested

func _ready() -> void:
	metal = StandardMaterial3D.new()
	metal.vertex_color_use_as_albedo = true
	metal.metallic = 0.65
	metal.roughness = 0.38
	preview_material = StandardMaterial3D.new()
	preview_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	preview_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	preview_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	preview = MeshInstance3D.new()
	preview.material_override = preview_material
	preview.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	preview.hide()
	add_child(preview)
	arrow_mesh = PumpDirection.create_mesh()
	preview_arrow = _new_arrow(preview)
	preview_arrow.hide()
	route_start = _route_marker(Color(0.98,0.72,0.18))
	route_target = _route_marker(Color(0.2,0.8,1.0))
	var canvas := CanvasLayer.new()
	add_child(canvas)
	panel = PanelContainer.new()
	panel.theme = Palette.create()
	panel.set_anchors_and_offsets_preset(Control.PRESET_CENTER)
	panel.offset_left = -285
	panel.offset_top = -250
	panel.offset_right = 285
	panel.offset_bottom = 250
	canvas.add_child(panel)
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation",16)
	panel.add_child(column)
	var scroll := ScrollContainer.new()
	scroll_area = scroll
	scroll.custom_minimum_size = Vector2(530,355)
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	column.add_child(scroll)
	var content := VBoxContainer.new()
	content.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content.add_theme_constant_override("separation",12)
	scroll.add_child(content)
	details = Label.new()
	details.custom_minimum_size = Vector2(500,0)
	details.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	content.add_child(details)
	thermal_facts = Label.new()
	thermal_facts.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	content.add_child(thermal_facts)
	thermal_controls = VBoxContainer.new()
	thermal_controls.add_theme_constant_override("separation",8)
	content.add_child(thermal_controls)
	thermal_mode = OptionButton.new()
	for label in ["换热关闭","手动换热","A 侧出口温度控制"]: thermal_mode.add_item(label)
	thermal_controls.add_child(thermal_mode)
	thermal_fraction = _thermal_number("手动换热比例 (%)",0,100,1,100)
	thermal_target = _thermal_number("A 侧出口温度目标 (K)",250,600,0.1,330)
	var thermal_buttons := HBoxContainer.new()
	thermal_controls.add_child(thermal_buttons)
	thermal_apply = Button.new()
	thermal_apply.text = "应用热控制"
	thermal_apply.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	thermal_apply.pressed.connect(_apply_thermal)
	thermal_buttons.add_child(thermal_apply)
	thermal_reload = Button.new()
	thermal_reload.text = "读取当前设置"
	thermal_reload.pressed.connect(func():
		if records.has(selected_id): _load_thermal_draft(records[selected_id]))
	thermal_buttons.add_child(thermal_reload)
	pump_controls = HBoxContainer.new()
	column.add_child(pump_controls)
	pump_switch = Button.new()
	pump_switch.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	pump_switch.pressed.connect(func(): _configure("enabled",not records[selected_id].enabled))
	pump_controls.add_child(pump_switch)
	pump_direction = Button.new()
	pump_direction.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	pump_direction.pressed.connect(func(): _configure("pump_reversed",not records[selected_id].pump_reversed))
	pump_controls.add_child(pump_direction)
	valve_controls = HBoxContainer.new()
	column.add_child(valve_controls)
	for percent in [0,25,50,75,100]:
		var button := Button.new()
		button.text = "%d%%" % percent
		button.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		button.pressed.connect(func(): _configure("opening",percent/100.0))
		valve_controls.add_child(button)
		valve_buttons.append(button)
	feedback = Label.new()
	feedback.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(feedback)
	var close := Button.new()
	close.text = "关闭  ·  Esc"
	close.pressed.connect(func(): close_requested.emit())
	column.add_child(close)
	panel.hide()

func shape_mesh(host: Node3D, kind: int, axis: int, pipe_ends: Array = []) -> ArrayMesh:
	var key := "%d:%d:%s" % [kind,axis,str(pipe_ends)]
	if not meshes.has(key):
		var data: Dictionary = host.bridge.pipe_geometry(axis,pipe_ends[0],pipe_ends[1]) if kind == 1 and pipe_ends.size() == 2 else host.bridge.equipment_geometry(kind,axis)
		var mesh: ArrayMesh = host._mesh(data)
		if mesh.get_surface_count() > 0: mesh.surface_set_material(0,metal)
		meshes[key] = mesh
	return meshes[key]

func update_view(host: Node3D) -> void:
	host_view = host
	var current: int = host.state.get("equipment_revision",0)
	if current != last_revision:
		last_revision = current
		records.clear()
		for device in host.bridge.equipment_state():
			records[device.id] = device
			var node: MeshInstance3D = nodes.get(device.id)
			if not is_instance_valid(node):
				node = MeshInstance3D.new()
				node.name = "Equipment_%d" % device.id
				node.gi_mode = GeometryInstance3D.GI_MODE_STATIC
				add_child(node)
				nodes[device.id] = node
			var shape_key := "%d:%d:%s" % [device.kind,device.axis,str(device.get("pipe_ends",[]))]
			if node.get_meta("shape","") != shape_key:
				node.mesh = shape_mesh(host,device.kind,device.axis,device.get("pipe_ends",[]))
				node.set_meta("shape",shape_key)
			if node.position != Vector3(device.origin): node.position = Vector3(device.origin)
			if device.kind == 2:
				if not arrows.has(device.id): arrows[device.id] = _new_arrow(node)
				_set_arrow(arrows[device.id],device.axis,device.pump_reversed)
			elif arrows.has(device.id):
				arrows[device.id].queue_free()
				arrows.erase(device.id)
		for id in nodes.keys():
			if not records.has(id):
				nodes[id].queue_free()
				nodes.erase(id)
				arrows.erase(id)
	if opened:
		if not records.has(selected_id) or host.bridge.equipment_target(host.yaw,host.pitch).get("id",0) != selected_id:
			close_requested.emit()
		else:
			if pending_revision >= 0 and records[selected_id].revision != pending_revision:
				pending_revision = -1
				if not pending_thermal.is_empty() and records[selected_id].get("thermal",{}) != pending_thermal:
					feedback.text = "设备设置已被其他操作更新，请读取当前设置。"
				else:
					feedback.text = "设置已同步"
					if not pending_thermal.is_empty(): draft_revision = records[selected_id].revision
				pending_thermal.clear()
			elif pending_revision >= 0 and Time.get_ticks_msec()-pending_since > 2000:
				pending_revision = -1
				pending_thermal.clear()
				feedback.text = "未确认更改，请查看状态后重试"
			if not host.state.container_error.is_empty(): feedback.text = host.state.container_error
			_describe(records[selected_id])
	_update_route(host)
	placement = host.bridge.equipment_preview(host.yaw,host.pitch) if host.captured and not opened and not route_state.get("active",false) else {}
	preview.visible = not placement.is_empty()
	preview_arrow.visible = preview.visible and placement.get("kind",-1) == 2
	if preview.visible:
		preview.mesh = shape_mesh(host,placement.kind,placement.axis)
		preview.position = Vector3(placement.origin)
		preview_material.albedo_color = Color(0.4,0.85,0.55,0.32) if placement.valid else Color(0.95,0.3,0.25,0.32)
		if preview_arrow.visible: _set_arrow(preview_arrow,placement.axis,placement.get("pump_reversed",false))

func _route_marker(colour: Color) -> MeshInstance3D:
	var marker := MeshInstance3D.new()
	var box := BoxMesh.new()
	box.size = Vector3(0.16,0.16,0.16)
	marker.mesh = box
	var paint := StandardMaterial3D.new()
	paint.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	paint.albedo_color = colour
	marker.material_override = paint
	marker.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	marker.gi_mode = GeometryInstance3D.GI_MODE_DISABLED
	marker.hide()
	add_child(marker)
	return marker

func _update_route(host: Node3D) -> void:
	route_state = host.bridge.pipe_route_state(host.yaw,host.pitch)
	var active: bool = route_state.get("active",false) and host.captured and not opened
	for pair in [[route_start,"start"],[route_target,"target"]]:
		var anchor: Dictionary = route_state.get(pair[1],{})
		pair[0].visible = active and not anchor.is_empty()
		if pair[0].visible: pair[0].position = anchor.position
	var cells: Array = route_state.get("cells",[]) if active else []
	for i in range(cells.size()):
		if i >= route_nodes.size():
			var node := MeshInstance3D.new()
			node.material_override = preview_material
			node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
			node.gi_mode = GeometryInstance3D.GI_MODE_DISABLED
			add_child(node)
			route_nodes.append(node)
		var cell: Dictionary = cells[i]
		var shape := Vector2i(cell.first,cell.second)
		if route_nodes[i].get_meta("route_shape",Vector2i(-1,-1)) != shape:
			route_nodes[i].mesh = shape_mesh(host,1,0,[cell.first,cell.second])
			route_nodes[i].set_meta("route_shape",shape)
		route_nodes[i].position = Vector3(cell.origin)
		route_nodes[i].show()
	for i in range(cells.size(),route_nodes.size()): route_nodes[i].hide()
	if active: preview_material.albedo_color = Color(0.4,0.85,0.55,0.32) if route_state.get("valid",false) else Color(0.95,0.3,0.25,0.32)
	route_revision = route_state.get("revision",-1)

func _new_arrow(parent: Node3D) -> MeshInstance3D:
	var marker := MeshInstance3D.new()
	marker.name = "PumpDirection"
	marker.mesh = arrow_mesh
	marker.position = Vector3(0.5,0.5,0.5)
	marker.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	marker.gi_mode = GeometryInstance3D.GI_MODE_DISABLED
	parent.add_child(marker)
	return marker

func _set_arrow(marker: MeshInstance3D, axis: int, reversed: bool) -> void:
	var direction_key := Vector2i(axis,1 if reversed else 0)
	if marker.get_meta("direction",Vector2i(-1,-1)) != direction_key:
		marker.basis = PumpDirection.direction_basis(axis,reversed)
		marker.set_meta("direction",direction_key)

func show_device(device: Dictionary) -> void:
	if device.is_empty(): return
	selected_id = device.id
	opened = true
	pending_revision = -1
	pending_thermal.clear()
	feedback.text = ""
	scroll_area.scroll_vertical = 0
	_load_thermal_draft(device)
	_describe(device)
	panel.show()

func _describe(device: Dictionary) -> void:
	var lines := PackedStringArray(["%s  #%d" % [device.name,device.id], "位置  %s" % device.origin])
	if device.kind == 1 and device.get("pipe_ends",[]).size() == 2:
		var faces := ["−X","+X","−Y","+Y","−Z","+Z"]
		lines.append("接头  %s ↔ %s" % [faces[device.pipe_ends[0]],faces[device.pipe_ends[1]]])
	elif device.kind not in [0,4]: lines.append("方向  %s 轴 · %d 个端口" % [["X","Y","Z"][device.axis],device.ports.size()])
	if device.kind == 6 and records.values().any(func(other): return other.kind == 1 and other.origin == device.origin):
		lines.append("正在承托管道 · 先拆管后拆架")
	elif device.empty: lines.append("库存为空 · 可拆除回收")
	else:
		lines.append("含有物料 · 禁止拆除")
		var names := {"icheme:water":"水","icheme:n_butane":"正丁烷","icheme:n_pentane":"正戊烷"}
		for item in device.contents: lines.append("%s  %.3f mol" % [names.get(item.material,item.material),item.moles])
	var reading: Dictionary = device.get("reading",{})
	if reading.is_empty(): lines.append("等待运行读数")
	else:
		var statuses := {"disconnected":"未连通","closed":"已隔离","flowing":"输送中","idle":"静止"}
		lines.append("状态  %s" % statuses.get(reading.status,reading.status))
		if not str(reading.status).begins_with("暂停："):
			if device.kind in [0,4]:
				lines.append("液量  %.1f / %.1f L" % [reading.volume_m3*1000,reading.capacity_m3*1000])
			# A heat exchanger has two independent passages, not one aggregate stream.
			# Its temperatures and signed flows are presented per side below.
			if device.kind != 5:
				lines.append("绝对压力  %s · %s %+.3f L/s" % ["未计算" if reading.pressure_pa == null else "%.2f kPa" % (reading.pressure_pa/1000),"净流入" if device.kind in [0,4] else "流量",reading.flow_m3_s*1000])
				if reading.temperature_k != null: lines.append("温度  %.2f K / %.1f °C" % [reading.temperature_k,reading.temperature_k-273.15])
				else: lines.append("温度  未计算")
	if device.kind == 2:
		lines.append("测试动力 · 关闭时隔离流路")
		lines.append("输送方向  %s%s" % ["−" if device.pump_reversed else "+",["X","Y","Z"][device.axis]])
	elif device.kind == 3: lines.append("阀门开度  %.0f%%" % (device.opening*100))
	if device.kind == 0: lines.append("开口水罐 · 单液态水模型")
	elif device.kind == 4: lines.append("密闭罐 · PR78 正丁烷 / 正戊烷")
	elif device.kind == 5: lines.append("橙色 A 侧 / 蓝色 B 侧 · 两流道只交换热量")
	_describe_thermal(device,reading.get("thermal",{}))
	details.text = "\n".join(lines)
	pump_controls.visible = device.kind == 2
	valve_controls.visible = device.kind == 3
	pump_switch.text = "关闭泵" if device.enabled else "启动泵"
	pump_direction.text = "反转方向"
	pump_switch.disabled = pending_revision >= 0
	pump_direction.disabled = pending_revision >= 0
	for index in range(valve_buttons.size()):
		valve_buttons[index].disabled = pending_revision >= 0 or is_equal_approx(device.opening,index*0.25)
	thermal_apply.disabled = pending_revision >= 0
	thermal_reload.disabled = pending_revision >= 0

func _thermal_number(title: String, minimum: float, maximum: float, step: float, initial: float) -> SpinBox:
	var row := HBoxContainer.new()
	thermal_controls.add_child(row)
	var label := Label.new()
	label.text = title
	label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	row.add_child(label)
	var input := SpinBox.new()
	input.min_value = minimum
	input.max_value = maximum
	input.step = step
	input.value = initial
	input.custom_minimum_size.x = 130
	row.add_child(input)
	return input

func _load_thermal_draft(device: Dictionary) -> void:
	var settings: Dictionary = device.get("thermal",{})
	draft_revision = device.revision
	if settings.is_empty(): return
	thermal_mode.select(settings.mode)
	thermal_fraction.value = settings.duty_fraction*100.0
	thermal_target.value = settings.target_temperature_k

func _apply_thermal() -> void:
	if not opened or not records.has(selected_id) or pending_revision >= 0: return
	pending_revision = records[selected_id].revision
	pending_since = Time.get_ticks_msec()
	feedback.text = "等待服务端确认…"
	pending_thermal = {"mode":thermal_mode.selected,"duty_fraction":thermal_fraction.value/100.0,"target_temperature_k":thermal_target.value}
	host_view.bridge.configure_thermal(selected_id,draft_revision,thermal_mode.selected,thermal_fraction.value/100.0,thermal_target.value,host_view.yaw,host_view.pitch)

func _temperature(value: Variant) -> String:
	return "无有效读数" if value == null else "%.2f K" % value

func _describe_thermal(device: Dictionary, reading: Dictionary) -> void:
	thermal_controls.visible = device.kind == 5
	thermal_facts.visible = not reading.is_empty() or device.kind in [4,5]
	if reading.is_empty():
		thermal_facts.text = "等待热过程读数"
		return
	var qualities := {"available":"有效","no_flow":"无流动（不代表当前流体温度）","unavailable":"不可用"}
	var lines := PackedStringArray(["仪表  %s · 接受步 %d" % [qualities.get(reading.quality,reading.quality),reading.sampled_at_tick]])
	if reading.quality != "unavailable":
		if reading.vapor_fraction != null:
			lines.append("气相摩尔分率  %.2f%%" % (reading.vapor_fraction*100.0))
			lines.append("相库存 (mol)         液相         气相")
			for i in range(2): lines.append("%s     %.3f     %.3f" % [["正丁烷","正戊烷"][i],reading.liquid_moles[i],reading.vapor_moles[i]])
		if device.kind == 5:
			lines.append("传入 A 侧热功率  %+.3f kW" % (reading.heat_w/1000.0))
			for i in range(2): lines.append("%s  %s → %s · %+.3f L/s" % [["A 橙","B 蓝"][i],_temperature(reading.inlet_temperature_k[i]),_temperature(reading.outlet_temperature_k[i]),reading.flow_m3_s[i]*1000.0])
	if device.kind == 5:
		var states := {"off":"关闭","manual":"手动","regulating":"调节中","limited":"受限，未达到目标"}
		lines.append("热控制  %s" % states.get(reading.control_state,reading.control_state))
		var applied: Dictionary = device.get("thermal",{})
		if not applied.is_empty(): lines.append("已应用：模式 %s · 比例 %.0f%% · 目标 %.1f K" % [["关闭","手动","出口温度"][applied.mode],applied.duty_fraction*100.0,applied.target_temperature_k])
		if draft_revision != device.revision: lines.append("设置版本已变化；读取当前设置后再编辑。")
	thermal_facts.text = "\n".join(lines)

func _configure(field: String, value: Variant) -> void:
	if not opened or not records.has(selected_id) or pending_revision >= 0: return
	var device: Dictionary = records[selected_id].duplicate(true)
	device[field] = value
	pending_revision = device.revision
	pending_since = Time.get_ticks_msec()
	feedback.text = "等待服务端确认…"
	host_view.bridge.configure_equipment(selected_id,device.revision,device.enabled,device.opening,device.pump_reversed,host_view.yaw,host_view.pitch)
	_describe(records[selected_id])

func close() -> void:
	opened = false
	selected_id = 0
	pending_revision = -1
	pending_thermal.clear()
	panel.hide()

func clear() -> void:
	close()
	for node in nodes.values(): node.queue_free()
	nodes.clear()
	arrows.clear()
	records.clear()
	last_revision = -1
	preview.hide()
	preview_arrow.hide()
	placement.clear()
	route_state.clear()
	for node in route_nodes: node.queue_free()
	route_nodes.clear()
	route_start.hide()
	route_target.hide()
	route_revision = -1

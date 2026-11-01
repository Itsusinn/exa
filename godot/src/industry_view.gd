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
	var canvas := CanvasLayer.new()
	add_child(canvas)
	panel = PanelContainer.new()
	panel.theme = Palette.create()
	panel.set_anchors_and_offsets_preset(Control.PRESET_CENTER)
	panel.offset_left = -250
	panel.offset_top = -235
	panel.offset_right = 250
	panel.offset_bottom = 235
	canvas.add_child(panel)
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation",16)
	panel.add_child(column)
	details = Label.new()
	details.custom_minimum_size = Vector2(460,270)
	details.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(details)
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

func shape_mesh(host: Node3D, kind: int, axis: int) -> ArrayMesh:
	var key := Vector2i(kind,axis)
	if not meshes.has(key):
		var mesh: ArrayMesh = host._mesh(host.bridge.equipment_geometry(kind,axis))
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
			var shape_key := Vector2i(device.kind,device.axis)
			if node.get_meta("shape",Vector2i(-1,-1)) != shape_key:
				node.mesh = shape_mesh(host,device.kind,device.axis)
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
				feedback.text = "设置已同步"
			elif pending_revision >= 0 and Time.get_ticks_msec()-pending_since > 2000:
				pending_revision = -1
				feedback.text = "未确认更改，请查看状态后重试"
			if not host.state.container_error.is_empty(): feedback.text = host.state.container_error
			_describe(records[selected_id])
	placement = host.bridge.equipment_preview(host.yaw,host.pitch) if host.captured and not opened else {}
	preview.visible = not placement.is_empty()
	preview_arrow.visible = preview.visible and placement.get("kind",-1) == 2
	if preview.visible:
		preview.mesh = shape_mesh(host,placement.kind,placement.axis)
		preview.position = Vector3(placement.origin)
		preview_material.albedo_color = Color(0.4,0.85,0.55,0.32) if placement.valid else Color(0.95,0.3,0.25,0.32)
		if preview_arrow.visible: _set_arrow(preview_arrow,placement.axis,placement.get("pump_reversed",false))

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
	feedback.text = ""
	_describe(device)
	panel.show()

func _describe(device: Dictionary) -> void:
	var lines := PackedStringArray(["%s  #%d" % [device.name,device.id], "位置  %s" % device.origin])
	if device.kind != 0: lines.append("方向  %s 轴 · %d 个端口" % [["X","Y","Z"][device.axis],device.ports.size()])
	if device.empty: lines.append("库存为空 · 可拆除回收")
	else:
		lines.append("含有物料 · 禁止拆除")
		for item in device.contents: lines.append("%s  %.3f mol" % [item.material,item.moles])
	var reading: Dictionary = device.get("reading",{})
	if reading.is_empty(): lines.append("等待运行读数")
	else:
		var statuses := {"disconnected":"未连通","closed":"已隔离","flowing":"输送中","idle":"静止"}
		lines.append("状态  %s" % statuses.get(reading.status,reading.status))
		if not str(reading.status).begins_with("暂停："):
			if device.kind == 0:
				lines.append("液量  %.1f / %.1f L" % [reading.volume_m3*1000,reading.capacity_m3*1000])
			lines.append("压力  %s · %s %+.3f L/s" % ["未计算" if reading.pressure_pa == null else "%.2f kPa" % (reading.pressure_pa/1000),"净流入" if device.kind == 0 else "流量",reading.flow_m3_s*1000])
			if reading.temperature_k != null: lines.append("温度  %.2f K / %.1f °C" % [reading.temperature_k,reading.temperature_k-273.15])
			else: lines.append("温度  未计算")
	if device.kind == 2:
		lines.append("测试动力 · 关闭时隔离流路")
		lines.append("输送方向  %s%s" % ["−" if device.pump_reversed else "+",["X","Y","Z"][device.axis]])
	elif device.kind == 3: lines.append("阀门开度  %.0f%%" % (device.opening*100))
	lines.append("单液态水模型 · 不含相变")
	details.text = "\n".join(lines)
	pump_controls.visible = device.kind == 2
	valve_controls.visible = device.kind == 3
	pump_switch.text = "关闭泵" if device.enabled else "启动泵"
	pump_direction.text = "反转方向"
	pump_switch.disabled = pending_revision >= 0
	pump_direction.disabled = pending_revision >= 0
	for index in range(valve_buttons.size()):
		valve_buttons[index].disabled = pending_revision >= 0 or is_equal_approx(device.opening,index*0.25)

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

extends CanvasLayer
signal connect_requested(address: String, player_name: String, certificate: String)
signal demo_requested
signal disconnect_requested
signal chat_requested(text: String)
signal fov_changed(value: float)
signal lighting_changed(enabled: bool)
signal singleplayer_requested
signal resume_requested
signal home_requested
signal quit_requested
signal container_transfer_requested(request: Dictionary)
signal inventory_move_requested(revision: int, source: int, target: int, quantity: int)
signal inventory_quick_move_requested(revision: int, slot: int)
signal container_close_requested

const MenuScript = preload("res://src/menu.gd")
const Palette = preload("res://src/interface_theme.gd")
const ContainerPanel = preload("res://src/container_panel.gd")
const ItemSlot = preload("res://src/item_slot.gd")
var hotbar: VBoxContainer
var hotbar_slots: Array = []
var held_name: Label
var container_panel: PanelContainer
var container_open := false
var container_wanted := false
var container_target: Dictionary = {}
var interaction_hint: Label
var block_target: Dictionary = {}
var equipment_target: Dictionary = {}
var equipment_placement: Dictionary = {}
var equipment_open := false
var pipe_route: Dictionary = {}
var menu: Control
var top: PanelContainer

var panel: PanelContainer
var address: LineEdit
var player_name: LineEdit
var certificate: LineEdit
var chat: LineEdit
var health: Label
var coordinates: Label
var journal: RichTextLabel
var reticle: Label
var fov_slider: HSlider
var gi_toggle: CheckButton
var inventory_open := false
var health_bar: ProgressBar

func _ready() -> void:
	var theme := Palette.create()
	var root := Control.new()
	root.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	root.mouse_filter = Control.MOUSE_FILTER_IGNORE
	root.theme = theme
	add_child(root)
	top = PanelContainer.new()
	top.set_anchors_and_offsets_preset(Control.PRESET_TOP_RIGHT)
	top.offset_left = -338
	top.offset_right = -18
	top.offset_top = 16
	top.offset_bottom = 80
	var bar_style := Palette.box(Color(0.045,0.055,0.045,0.72), 14)
	bar_style.border_width_bottom = 1
	bar_style.border_color = Color(0.8,0.82,0.72,0.16)
	top.add_theme_stylebox_override("panel",bar_style)
	root.add_child(top)
	var vitals := HBoxContainer.new()
	vitals.add_theme_constant_override("separation", 30)
	top.add_child(vitals)
	health = label(vitals, "", 17)
	health.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	health.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	health.add_theme_font_override("font",Palette.technical_font())
	health_bar = ProgressBar.new()
	health_bar.custom_minimum_size = Vector2(120,6)
	health_bar.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	health_bar.show_percentage = false
	health_bar.add_theme_stylebox_override("background",Palette.box(Color("515b64"),0))
	health_bar.add_theme_stylebox_override("fill",Palette.box(Palette.ACCENT,0))
	vitals.add_child(health_bar)
	panel = ContainerPanel.new()
	root.add_child(panel)
	container_panel = panel
	var column: VBoxContainer = panel.details
	label(column, "行动记录  /  LOG", 21).add_theme_font_override("font",Palette.display_font())
	journal = RichTextLabel.new()
	journal.add_theme_font_size_override("normal_font_size",14)
	journal.custom_minimum_size = Vector2(225,120)
	journal.scroll_following = true
	column.add_child(journal)
	chat = field(column, "通讯  /  CHAT", "")
	chat.placeholder_text = "输入消息或 /help，按 Enter 发送"
	chat.text_submitted.connect(func(text: String):
		if not text.strip_edges().is_empty():
			chat_requested.emit(text)
			chat.clear())
	coordinates = label(column, "", 14)
	reticle = label(root, "+", 25)
	reticle.add_theme_color_override("font_color",Color.WHITE)
	reticle.set_anchors_and_offsets_preset(Control.PRESET_CENTER)
	reticle.mouse_filter = Control.MOUSE_FILTER_IGNORE
	interaction_hint = label(root, "E  打开箱子", 17)
	interaction_hint.set_anchors_and_offsets_preset(Control.PRESET_CENTER)
	interaction_hint.position += Vector2(-55, 38)
	interaction_hint.mouse_filter = Control.MOUSE_FILTER_IGNORE
	container_panel.transfer_requested.connect(func(request: Dictionary): container_transfer_requested.emit(request))
	container_panel.move_requested.connect(func(r: int, f: int, t: int, q: int): inventory_move_requested.emit(r,f,t,q))
	container_panel.quick_move_requested.connect(func(r: int, s: int): inventory_quick_move_requested.emit(r,s))
	container_panel.close_requested.connect(func(): container_close_requested.emit())
	hotbar = VBoxContainer.new()
	hotbar.set_anchors_and_offsets_preset(Control.PRESET_CENTER_BOTTOM)
	hotbar.offset_left = -250
	hotbar.offset_right = 250
	hotbar.offset_top = -94
	hotbar.offset_bottom = -16
	hotbar.mouse_filter = Control.MOUSE_FILTER_IGNORE
	root.add_child(hotbar)
	held_name = label(hotbar,"空手",15)
	held_name.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	held_name.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var bar := HBoxContainer.new()
	bar.mouse_filter = Control.MOUSE_FILTER_IGNORE
	hotbar.add_child(bar)
	for i in range(9):
		var slot := ItemSlot.new()
		slot.number = str(i+1)
		slot.mouse_filter = Control.MOUSE_FILTER_IGNORE
		bar.add_child(slot)
		hotbar_slots.append(slot)
	menu = MenuScript.new()
	root.add_child(menu)
	address = menu.address
	player_name = menu.player_name
	certificate = menu.certificate
	fov_slider = menu.fov_slider
	gi_toggle = menu.gi_toggle
	menu.connect_requested.connect(func(a: String, n: String, c: String): connect_requested.emit(a,n,c))
	menu.singleplayer_requested.connect(func(): singleplayer_requested.emit())
	menu.resume_requested.connect(func(): resume_requested.emit())
	menu.home_requested.connect(func(): home_requested.emit())
	menu.quit_requested.connect(func(): quit_requested.emit())
	menu.fov_changed.connect(func(value: float): fov_changed.emit(value))
	menu.lighting_changed.connect(func(enabled: bool): lighting_changed.emit(enabled))

func label(parent: Node, text: String, size: int) -> Label:
	var result := Label.new()
	result.text = text
	result.add_theme_font_size_override("font_size", size)
	parent.add_child(result)
	return result

func field(parent: Node, caption: String, text: String) -> LineEdit:
	label(parent, caption, 14)
	var result := LineEdit.new()
	result.text = text
	parent.add_child(result)
	return result

func button(parent: Node, text: String, callback: Callable) -> Button:
	var result := Button.new()
	result.text = text
	result.custom_minimum_size.y = 34
	parent.add_child(result)
	result.pressed.connect(callback)
	return result

func update_state(state: Dictionary, captured: bool) -> void:
	panel.visible = (inventory_open or container_open) and state.ready and not menu.visible
	if panel.visible: panel.update_view(state,container_open)
	else: panel.clear_selection()
	hotbar.visible = state.ready and not menu.visible and not panel.visible and not equipment_open
	for i in range(9): hotbar_slots[i].update_slot(state.inventory_slots[i],i == state.selected_slot,false,{})
	var held: Dictionary = state.inventory_slots[state.selected_slot]
	held_name.text = "空手" if held.is_empty() else held.name
	top.visible = state.ready and not menu.visible
	top.offset_left = -440 if state.get("flying", false) else -338
	health.text = "HP   %03d / 100" % state.hp if state.ready else ""
	if state.get("flying", false): health.text += "  ·  飞行"
	health_bar.value = state.hp
	var p: Vector3 = state.body_position
	coordinates.text = "X %.1f   Y %.1f   Z %.1f" % [p.x, p.y, p.z]
	var log_text := "\n".join(PackedStringArray(state.journal))
	if journal.text != log_text: journal.text = log_text
	chat.editable = state.ready and not state.demo
	reticle.visible = state.ready and not panel.visible and not menu.visible and not container_open and not equipment_open
	var hint := ""
	if pipe_route.get("active",false):
		hint = pipe_route.get("message","")
		if pipe_route.get("length",0) > 0: hint += " · %d 弯头 · %d 件管道" % [pipe_route.get("elbows",0),pipe_route.get("materials",0)]
	elif not state.container_error.is_empty(): hint = state.container_error
	elif not equipment_placement.is_empty():
		hint = "%s · %s · 右键建造" % [equipment_placement.name,"可放置" if equipment_placement.valid else equipment_placement.reason]
		if equipment_placement.kind in [1,2,3,6]: hint += " · %s 轴 / R 旋转" % ["X","Y","Z"][equipment_placement.axis]
		if equipment_placement.kind == 1: hint += " · P 铺管模式"
		if not equipment_target.is_empty(): hint += " · E 查看 / 左键拆除"
	elif not equipment_target.is_empty(): hint = "%s · E 查看 / 左键拆除" % equipment_target.name
	elif not block_target.is_empty():
		hint = "%s  ·  左键破坏 / 右键放置" % block_target.name
		if not container_target.is_empty(): hint += "  ·  E 打开"
	interaction_hint.text = hint
	interaction_hint.position.x = (get_viewport().get_visible_rect().size.x - interaction_hint.size.x) * 0.5
	interaction_hint.visible = captured and reticle.visible and not hint.is_empty()

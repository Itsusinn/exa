extends Control
const Palette = preload("res://src/interface_theme.gd")
const Wood = preload("res://materials/textures/weathered_brown_planks_diff_1k.jpg")
const Stone = preload("res://materials/textures/stone_brick_wall_001_diff_1k.jpg")
const Grass = preload("res://materials/textures/leafy_grass_diff_1k.jpg")
const Bark = preload("res://materials/textures/bark_brown_02_diff_1k.jpg")
signal activated(area: String, index: int, mouse_button: int, quick: bool)
signal dropped(area: String, index: int, payload: Dictionary)
var area := "player"
var index := 0
var item: Dictionary = {}
var payload: Dictionary = {}
var selected := false
var picked := false
var number := ""

func _ready() -> void:
	custom_minimum_size = Vector2(52,52)
	mouse_default_cursor_shape = Control.CURSOR_POINTING_HAND
	mouse_entered.connect(queue_redraw)
	mouse_exited.connect(queue_redraw)

func update_slot(value: Dictionary, active: bool, chosen: bool, data: Dictionary) -> void:
	item = value
	selected = active
	picked = chosen
	payload = data
	tooltip_text = "空槽位" if item.is_empty() else "%s × %d" % [item.name,item.quantity]
	queue_redraw()

func _draw() -> void:
	var frame := Rect2(Vector2.ZERO,size)
	draw_rect(frame,Color(0.045,0.055,0.045,0.8))
	draw_rect(frame.grow(-1),Color(0.6,0.65,0.55,0.26),false,1)
	if frame.has_point(get_local_mouse_position()): draw_rect(frame.grow(-2),Color(1,1,1,0.06))
	if selected:
		draw_rect(frame.grow(-1),Palette.ACCENT,false,1)
		draw_rect(Rect2(1,size.y-3,size.x-2,2),Palette.ACCENT)
	if picked: draw_rect(frame.grow(-3),Palette.BLUE,false,1)
	var font := ThemeDB.fallback_font
	if not item.is_empty():
		var center := size * 0.5
		var texture: Texture2D = {"石块":Stone,"草方块":Grass,"泥土":Grass,"树叶":Grass,"原木":Bark}.get(item.name,Wood)
		var tint := Color.WHITE
		if item.name == "泥土": tint = Color("8d6948")
		elif item.name == "树叶": tint = Color("749a65")
		draw_rect(Rect2(center-Vector2(16,13),Vector2(33,29)),Color(0,0,0,0.45))
		draw_texture_rect(texture,Rect2(center-Vector2(16,16),Vector2(32,28)),false,tint)
		draw_rect(Rect2(center-Vector2(16,16),Vector2(32,28)),Color("797468"),false,1)
		var minerals := {"沙子":"c7b37d","砂岩":"b08f5c","砾石":"7a7a70","黏土":"786e5c","雪块":"e5eff5","冰块":"87bfd6","苔石":"5c7347","玄武岩":"3d4247","煤矿石":"34363a","铁矿石":"a66e4c","铜矿石":"69a37a"}
		var plants := {"白桦原木":"d1cfb3","白桦树叶":"85a147","松木":"573d2b","松针叶":"305742"}
		if minerals.has(item.name) or plants.has(item.name):
			var color := Color(str(minerals.get(item.name,plants.get(item.name,"ffffff"))))
			draw_rect(Rect2(center-Vector2(16,16),Vector2(32,28)),color)
			for n in range(7):
				var point := center+Vector2((n*11)%27-13,(n*7)%23-13)
				draw_rect(Rect2(point,Vector2(3,2)),color.darkened(0.25))
		if item.name == "箱子" or item.name in ["Supplies","Tool"]:
			draw_line(center-Vector2(15,8),center+Vector2(15,-8),Color("363a32"),2)
			draw_rect(Rect2(center-Vector2(11,15),Vector2(3,26)),Color("4a4f46"))
			draw_rect(Rect2(center+Vector2(8,-15),Vector2(3,26)),Color("4a4f46"))
			draw_rect(Rect2(center-Vector2(2,10),Vector2(4,6)),Palette.MUTED)
		if item.name in ["储罐","管道","测试泵","阀门"]:
			draw_rect(Rect2(center-Vector2(16,16),Vector2(32,28)),Color("202728"))
			if item.name == "储罐":
				draw_rect(Rect2(center-Vector2(11,12),Vector2(22,23)),Color("87969b"))
				draw_ellipse_icon(center-Vector2(0,12),Vector2(11,4),Color("b3bec1"))
				draw_ellipse_icon(center+Vector2(0,11),Vector2(11,4),Color("62757c"))
			else:
				draw_line(center-Vector2(13,0),center+Vector2(13,0),Color("a8b5ba"),9)
				draw_line(center-Vector2(12,6),center-Vector2(12,-6),Color("62757c"),3)
				draw_line(center+Vector2(12,-6),center+Vector2(12,6),Color("62757c"),3)
				if item.name == "测试泵":
					draw_circle(center,9,Color("527a6b"))
					draw_colored_polygon(PackedVector2Array([center+Vector2(-3,-5),center+Vector2(5,0),center+Vector2(-3,5)]),Color("ced9d4"))
				elif item.name == "阀门":
					draw_colored_polygon(PackedVector2Array([center+Vector2(-8,-7),center,center+Vector2(-8,7)]),Color("b18a54"))
					draw_colored_polygon(PackedVector2Array([center+Vector2(8,-7),center+Vector2(8,7),center]),Color("b18a54"))
					draw_line(center,center-Vector2(0,11),Color("c5b091"),2)
					draw_line(center-Vector2(6,11),center+Vector2(6,-11),Color("c5b091"),3)
		var count := str(item.quantity)
		var width := font.get_string_size(count,HORIZONTAL_ALIGNMENT_LEFT,-1,16).x
		draw_string_outline(font,Vector2(size.x-width-4,size.y-5),count,HORIZONTAL_ALIGNMENT_LEFT,-1,16,4,Color("101318"))
		draw_string(font,Vector2(size.x-width-4,size.y-5),count,HORIZONTAL_ALIGNMENT_LEFT,-1,16,Color.WHITE)
	if not number.is_empty(): draw_string(font,Vector2(5,14),number,HORIZONTAL_ALIGNMENT_LEFT,-1,12,Color("bfc9d1"))

func draw_ellipse_icon(center: Vector2, radius: Vector2, color: Color) -> void:
	var points := PackedVector2Array()
	for i in range(20):
		var angle := TAU * i / 20.0
		points.append(center+Vector2(cos(angle),sin(angle))*radius)
	draw_colored_polygon(points,color)

func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed and event.button_index in [MOUSE_BUTTON_LEFT,MOUSE_BUTTON_RIGHT]:
		activated.emit(area,index,event.button_index,event.shift_pressed)
		accept_event()

func _get_drag_data(_position: Vector2) -> Variant:
	if item.is_empty() or payload.is_empty(): return null
	var preview := Label.new()
	preview.text = "%s × %d" % [item.name,payload.quantity]
	preview.add_theme_color_override("font_color",Color.WHITE)
	preview.add_theme_color_override("font_shadow_color",Color.BLACK)
	preview.add_theme_constant_override("shadow_offset_x",2)
	preview.add_theme_constant_override("shadow_offset_y",2)
	set_drag_preview(preview)
	return payload.duplicate(true)

func _can_drop_data(_position: Vector2, data: Variant) -> bool:
	return data is Dictionary and data.get("kind","") == "inventory_stack"

func _drop_data(_position: Vector2, data: Variant) -> void:
	dropped.emit(area,index,data)

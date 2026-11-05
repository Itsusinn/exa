extends Control
signal singleplayer_requested
signal connect_requested(address: String, player_name: String, certificate: String)
signal resume_requested
signal home_requested
signal quit_requested
signal fov_changed(value: float)
signal lighting_changed(enabled: bool)

const Palette = preload("res://src/interface_theme.gd")
const Tile = preload("res://src/menu_tile.gd")
const Backdrop = preload("res://materials/exa_background.png")
const GlassShader = preload("res://src/menu_glass.gdshader")
const ACCENT := Palette.ACCENT
var page := "home"
var return_page := "home"
var in_game := false
var address: LineEdit
var player_name: LineEdit
var certificate: LineEdit
var fov_slider: HSlider
var gi_toggle: CheckButton
var sensitivity: HSlider
var fullscreen: CheckButton
const AA_MODES := ["off", "fxaa", "msaa2", "msaa4", "taa"]
var antialiasing: OptionButton
var aa_hint: Label
var pages: Dictionary = {}
var heading: Label
var subtitle: Label
var feedback: Label
var hero: VBoxContainer
var card: PanelContainer
var settings := ConfigFile.new()
var loading_settings := true
var transition: Tween
var home_tiles: Control
var glass: ColorRect
var section_label: Label
var pause_title: VBoxContainer

func _ready() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_STOP
	theme = Palette.create()
	glass = ColorRect.new()
	glass.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	glass.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var glass_material := ShaderMaterial.new()
	glass_material.shader = GlassShader
	glass.material = glass_material
	add_child(glass)
	var header_bar := ColorRect.new()
	header_bar.color = Color(0.025,0.035,0.025,0.60)
	header_bar.set_anchors_and_offsets_preset(Control.PRESET_TOP_WIDE)
	header_bar.offset_bottom = 82
	header_bar.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(header_bar)
	var badge := ColorRect.new()
	badge.color = Palette.ACCENT
	badge.position = Vector2(36,27)
	badge.size = Vector2(2,28)
	badge.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(badge)
	var header := Label.new()
	header.text = "exa"
	header.position = Vector2(52, 29)
	header.add_theme_font_override("font", Palette.technical_font())
	header.add_theme_font_size_override("font_size", 20)
	header.add_theme_color_override("font_color", Palette.PAPER)
	add_child(header)
	section_label = Label.new()
	section_label.text = "首页  /  HOME"
	section_label.position = Vector2(205, 31)
	section_label.add_theme_font_size_override("font_size", 15)
	section_label.modulate = Palette.MUTED
	add_child(section_label)
	var system_label := Label.new()
	system_label.text = "探索 · 生存 · 同行"
	system_label.set_anchors_and_offsets_preset(Control.PRESET_TOP_RIGHT)
	system_label.offset_left = -440
	system_label.offset_right = -40
	system_label.offset_top = 32
	system_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	system_label.add_theme_font_size_override("font_size", 14)
	add_child(system_label)
	hero = VBoxContainer.new()
	hero.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	hero.anchor_left = 0.055
	hero.anchor_right = 0.46
	hero.anchor_top = 0.38
	hero.anchor_bottom = 0.70
	hero.add_theme_constant_override("separation", 5)
	hero.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(hero)
	_text(hero, "荒野之间", 15).modulate = ACCENT
	_text(hero, "exa", 46).add_theme_font_override("font", Palette.display_font())
	_text(hero, "探索 · 建造 · 生存", 16).add_theme_font_override("font", Palette.technical_font())
	_text(hero, "在林地与石墙之间，开始新的旅程。", 15).modulate = Palette.MUTED
	pause_title = VBoxContainer.new()
	pause_title.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	pause_title.anchor_left = 0.075
	pause_title.anchor_right = 0.46
	pause_title.anchor_top = 0.38
	pause_title.anchor_bottom = 0.75
	add_child(pause_title)
	_text(pause_title, "稍作停留", 40).add_theme_font_override("font", Palette.technical_font())
	_text(pause_title, "游戏菜单", 26).add_theme_font_override("font", Palette.display_font())
	_text(pause_title, "世界仍在运行，请注意周围。", 16).modulate = ACCENT
	home_tiles = Control.new()
	home_tiles.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	home_tiles.anchor_left = 0.53
	home_tiles.anchor_right = 0.925
	home_tiles.anchor_top = 0.245
	home_tiles.anchor_bottom = 0.835
	home_tiles.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(home_tiles)
	pages.home = home_tiles
	_tile("单人游戏", "独自启程，探索与生存", Rect2(0,0,1,0.39), func(): singleplayer_requested.emit(), true)
	_tile("多人游戏", "与伙伴一起探索边境", Rect2(0,0.435,1,0.285), func(): show_page("multiplayer"), false, Palette.BLUE)
	_tile("设置", "画面与操作", Rect2(0,0.765,0.475,0.235), open_settings)
	_tile("退出游戏", "结束本次行动", Rect2(0.525,0.765,0.475,0.235), func(): quit_requested.emit(), false, Palette.INK)
	card = PanelContainer.new()
	card.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	card.anchor_left = 0.53
	card.anchor_right = 0.925
	card.anchor_top = 0.18
	card.anchor_bottom = 0.89
	add_child(card)
	var scroll := ScrollContainer.new()
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	card.add_child(scroll)
	var content := VBoxContainer.new()
	content.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content.add_theme_constant_override("separation", 12)
	scroll.add_child(content)
	heading = _text(content, "开始旅程", 32)
	heading.add_theme_font_override("font", Palette.display_font())
	subtitle = _text(content, "选择你的探索方式", 15)
	subtitle.modulate = Palette.MUTED
	content.add_child(HSeparator.new())
	for key in ["multiplayer", "settings", "pause", "busy"]:
		var column := VBoxContainer.new()
		column.add_theme_constant_override("separation", 12)
		content.add_child(column)
		pages[key] = column
	var multi: VBoxContainer = pages.multiplayer
	address = _field(multi, "服务器地址", "127.0.0.1:9876")
	player_name = _field(multi, "玩家名称", "Explorer")
	certificate = _field(multi, "服务器证书 (.der)", ProjectSettings.globalize_path("res://../save/server.cert.der"))
	_button(multi, "连接服务器  →", func(): connect_requested.emit(address.text.strip_edges(), player_name.text.strip_edges(), certificate.text.strip_edges()))
	_button(multi, "返回", func(): show_page("home"))
	var config: VBoxContainer = pages.settings
	_text(config, "视野范围", 16)
	fov_slider = HSlider.new()
	fov_slider.min_value = 55
	fov_slider.max_value = 100
	fov_slider.value = 72
	fov_slider.custom_minimum_size.y = 30
	config.add_child(fov_slider)
	var fov_value := _text(config, "72°", 14)
	fov_slider.value_changed.connect(func(value: float):
		fov_value.text = "%d°" % value
		fov_changed.emit(value)
		_save_settings())
	_text(config, "鼠标灵敏度", 16)
	sensitivity = HSlider.new()
	sensitivity.min_value = 0.5
	sensitivity.max_value = 2.0
	sensitivity.step = 0.05
	sensitivity.value = 1.0
	sensitivity.custom_minimum_size.y = 30
	config.add_child(sensitivity)
	sensitivity.value_changed.connect(func(_value: float): _save_settings())
	antialiasing = OptionButton.new()
	for label in ["抗锯齿：关闭", "抗锯齿：FXAA（轻量）", "抗锯齿：MSAA 2×", "抗锯齿：MSAA 4×（推荐）", "抗锯齿：TAA（时间累积）"]:
		antialiasing.add_item(label)
	config.add_child(antialiasing)
	aa_hint = _text(config,"",14)
	antialiasing.item_selected.connect(func(index: int):
		set_antialiasing(AA_MODES[index])
		_save_settings())
	gi_toggle = CheckButton.new()
	gi_toggle.text = "全局光照"
	gi_toggle.button_pressed = true
	config.add_child(gi_toggle)
	gi_toggle.toggled.connect(func(enabled: bool):
		lighting_changed.emit(enabled)
		_save_settings())
	fullscreen = CheckButton.new()
	fullscreen.text = "全屏显示"
	config.add_child(fullscreen)
	fullscreen.toggled.connect(func(enabled: bool):
		DisplayServer.window_set_mode(DisplayServer.WINDOW_MODE_FULLSCREEN if enabled else DisplayServer.WINDOW_MODE_WINDOWED)
		_save_settings())
	_button(config, "返回", func(): show_page(return_page))
	var pause: VBoxContainer = pages.pause
	_button(pause, "继续游戏  →", func(): resume_requested.emit())
	_button(pause, "设置", open_settings)
	_button(pause, "返回主界面", func(): home_requested.emit())
	_button(pause, "退出游戏", func(): quit_requested.emit())
	_text(pages.busy, "请稍候…", 20)
	_button(pages.busy, "返回主界面", func(): home_requested.emit())
	feedback = _text(self, "", 14)
	feedback.set_anchors_and_offsets_preset(Control.PRESET_BOTTOM_WIDE)
	feedback.offset_left = 52
	feedback.offset_right = -52
	feedback.offset_top = -44
	feedback.offset_bottom = -16
	feedback.modulate = ACCENT
	settings.load(preload("res://src/app_config.gd").user_file("settings.cfg"))
	fov_slider.value = clampf(float(settings.get_value("video", "fov", 72)), 55, 100)
	sensitivity.value = clampf(float(settings.get_value("input", "sensitivity", 1.0)), 0.5, 2.0)
	gi_toggle.button_pressed = bool(settings.get_value("video", "gi", true))
	fullscreen.button_pressed = bool(settings.get_value("video", "fullscreen", false))
	set_antialiasing(str(settings.get_value("video", "antialiasing", "msaa4")))
	loading_settings = false
	show_page("home")
	resized.connect(queue_redraw)

func _draw() -> void:
	if not in_game:
		draw_texture_rect(Backdrop,Rect2(Vector2.ZERO,size),false)
		draw_rect(Rect2(Vector2.ZERO,size),Color(0.02,0.03,0.02,0.53 if page == "home" else 0.68))
	# The header is drawn by a separate overlay so the glass stays below it.

func _tile(caption: String, description: String, area: Rect2, callback: Callable, large := false, tint := Palette.PAPER) -> void:
	var tile := Tile.new()
	tile.caption = caption
	tile.description = description
	tile.large = large
	tile.tint = tint
	tile.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	tile.anchor_left = area.position.x
	tile.anchor_top = area.position.y
	tile.anchor_right = area.end.x
	tile.anchor_bottom = area.end.y
	home_tiles.add_child(tile)
	tile.pressed.connect(callback)

func show_page(next: String) -> void:
	page = next
	visible = true
	for key in pages: pages[key].visible = key == page
	card.visible = page != "home"
	glass.visible = in_game
	pause_title.visible = in_game
	section_label.text = {"home":"首页  /  HOME", "multiplayer":"联机  /  NETWORK", "settings":"设置  /  SETTINGS", "pause":"行动  /  OPERATIONS", "busy":"连接  /  CONNECTING"}[page]
	heading.text = {"home":"开始旅程", "multiplayer":"多人游戏", "settings":"设置", "pause":"游戏菜单", "busy":"正在处理"}[page]
	subtitle.text = {"home":"选择你的探索方式", "multiplayer":"与伙伴一起探索边境", "settings":"调整你的视野与操作", "pause":"世界仍在运行，请注意周围", "busy":"正在准备或关闭世界"}[page]
	feedback.text = ""
	hero.visible = not in_game
	if transition: transition.kill()
	var target: Control = home_tiles if page == "home" else card
	target.modulate.a = 0.4
	transition = create_tween()
	target.pivot_offset = target.size * 0.5
	target.scale = Vector2(0.975,0.975)
	transition.set_parallel(true)
	transition.tween_property(target, "modulate:a", 1.0, 0.22)
	transition.tween_property(target, "scale", Vector2.ONE, 0.28).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	var first: Array[Node] = pages[page].find_children("*", "Button", true, false)
	if not first.is_empty(): first[0].grab_focus()
	queue_redraw()

func open_settings() -> void:
	return_page = "pause" if in_game else "home"
	show_page("settings")

func set_antialiasing(mode: String) -> void:
	var index := AA_MODES.find(mode)
	if index < 0: index = 3
	antialiasing.select(index)
	var viewport := get_viewport()
	# Keep techniques exclusive, including when leaving a temporal mode.
	viewport.use_taa = false
	viewport.screen_space_aa = Viewport.SCREEN_SPACE_AA_DISABLED
	viewport.msaa_3d = Viewport.MSAA_DISABLED
	match AA_MODES[index]:
		"fxaa": viewport.screen_space_aa = Viewport.SCREEN_SPACE_AA_FXAA
		"msaa2": viewport.msaa_3d = Viewport.MSAA_2X
		"msaa4": viewport.msaa_3d = Viewport.MSAA_4X
		"taa": viewport.use_taa = true
	aa_hint.text = ["保留原始像素边缘。", "开销较低，画面会稍柔和。", "平滑轮廓，保留纹理清晰度。", "更细致地平滑轮廓，GPU 开销较高。", "改善细节闪烁；移动时可能模糊或拖影。"][index]

func _save_settings() -> void:
	if loading_settings: return
	settings.set_value("video", "fov", fov_slider.value)
	settings.set_value("video", "gi", gi_toggle.button_pressed)
	settings.set_value("video", "antialiasing", AA_MODES[antialiasing.selected])
	settings.set_value("video", "fullscreen", fullscreen.button_pressed)
	settings.set_value("input", "sensitivity", sensitivity.value)
	if settings.save(preload("res://src/app_config.gd").user_file("settings.cfg")) != OK: feedback.text = "设置保存失败，请检查用户目录权限。"

func _text(parent: Node, text: String, font_size: int) -> Label:
	var result := Label.new()
	result.text = text
	result.add_theme_font_size_override("font_size", font_size)
	result.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	parent.add_child(result)
	return result

func _button(parent: Node, text: String, callback: Callable) -> Button:
	var result := Button.new()
	result.text = text
	result.alignment = HORIZONTAL_ALIGNMENT_LEFT
	result.custom_minimum_size.y = 54
	parent.add_child(result)
	result.pressed.connect(callback)
	return result

func _field(parent: Node, caption: String, text: String) -> LineEdit:
	_text(parent, caption, 14)
	var result := LineEdit.new()
	result.text = text
	result.custom_minimum_size.y = 38
	parent.add_child(result)
	return result

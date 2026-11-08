extends RefCounted
## Session-owned copies of the imported finishes. World triplanar projection
## keeps the physical detail scale independent of vessel diameter/height/UVs.
## These are small isotropic imperfections, not simulated brushed anisotropy.
const MetalNormal = preload("res://materials/industry/metal_normal.tres")
const MetalRoughness = preload("res://materials/industry/metal_roughness.tres")
const PaintNormal = preload("res://materials/industry/paint_normal.tres")
const PaintRoughness = preload("res://materials/industry/paint_roughness.tres")
const DETAIL_SCALE := Vector3(4.0,4.0,4.0) # One tile per 0.25 world metres.
const METALS := ["Brushed_stainless","Safety_brass"]
const PAINTS := ["Cool_process","Warm_process","Marine_enamel","Cast_graphite"]
var surface_detail_enabled := true
var cached_materials: Dictionary = {}

func resolve(source: Material) -> Material:
	var base := source as StandardMaterial3D
	if base == null: return source
	if cached_materials.has(source): return cached_materials[source]
	var name := source.resource_name.replace(" ","_")
	if name not in METALS and name not in PAINTS: return source
	# Leave future textured authored finishes intact: world triplanar changes all
	# UV1 sampling, and this finite layer is for the existing scalar finishes.
	for slot in range(BaseMaterial3D.TEXTURE_MAX):
		if base.get_texture(slot) != null: return source
	var finish := source.duplicate() as StandardMaterial3D
	cached_materials[source] = finish
	_apply(base,finish)
	return finish

func set_surface_detail(enabled: bool) -> void:
	if surface_detail_enabled == enabled: return
	surface_detail_enabled = enabled
	# Existing device meshes share these owned copies; no geometry rebuild and
	# no imported resource mutation are needed for a development comparison.
	for source: StandardMaterial3D in cached_materials:
		_apply(source,cached_materials[source])

func _apply(source: StandardMaterial3D, finish: StandardMaterial3D) -> void:
	if not surface_detail_enabled:
		finish.normal_enabled = source.normal_enabled
		finish.normal_texture = source.normal_texture
		finish.normal_scale = source.normal_scale
		finish.roughness = source.roughness
		finish.roughness_texture = source.roughness_texture
		finish.roughness_texture_channel = source.roughness_texture_channel
		finish.uv1_triplanar = source.uv1_triplanar
		finish.uv1_world_triplanar = source.uv1_world_triplanar
		finish.uv1_triplanar_sharpness = source.uv1_triplanar_sharpness
		finish.uv1_scale = source.uv1_scale
		finish.texture_filter = source.texture_filter
		return
	var metal := source.resource_name.replace(" ","_") in METALS
	finish.normal_enabled = true
	finish.normal_texture = MetalNormal if metal else PaintNormal
	finish.normal_scale = 0.16 if metal else 0.10
	finish.roughness_texture = MetalRoughness if metal else PaintRoughness
	finish.roughness_texture_channel = BaseMaterial3D.TEXTURE_CHANNEL_RED
	# The grayscale ramps average near 0.92/0.95. Keep the nominal roughness
	# near the authored value while allowing bounded local variation around it.
	finish.roughness = minf(source.roughness/(0.92 if metal else 0.95),1.0)
	finish.uv1_triplanar = true
	finish.uv1_world_triplanar = true
	finish.uv1_triplanar_sharpness = 4.0
	finish.uv1_scale = DETAIL_SCALE
	finish.texture_filter = BaseMaterial3D.TEXTURE_FILTER_LINEAR_WITH_MIPMAPS_ANISOTROPIC

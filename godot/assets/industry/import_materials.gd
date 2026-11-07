@tool
extends EditorScenePostImport
## Godot's glTF importer does not retain KHR_materials_clearcoat. Read scalar
## coat factors from the authored GLB at import time; exports need no runtime IO.

func _post_import(scene: Node) -> Object:
	var file := FileAccess.open(get_source_file(),FileAccess.READ)
	if file == null or file.get_32() != 0x46546c67 or file.get_32() != 2:
		push_error("Industry material import requires glTF 2.0 GLB")
		return null
	file.get_32() # Total container length.
	var length := file.get_32()
	if file.get_32() != 0x4e4f534a or length > file.get_length()-file.get_position():
		push_error("Industry material import has no valid JSON chunk")
		return null
	var parsed: Variant = JSON.parse_string(file.get_buffer(length).get_string_from_utf8())
	if not parsed is Dictionary: return null
	var coats := {}
	for material in parsed.get("materials",[]):
		var coat: Dictionary = material.get("extensions",{}).get("KHR_materials_clearcoat",{})
		# The authored library uses scalar coats. Do not silently drop future maps.
		for key in coat:
			if key not in ["clearcoatFactor","clearcoatRoughnessFactor"]:
				push_error("Unsupported industry coat property: "+key)
				return null
		coats[str(material.get("name","")).replace(" ","_")] = coat
	_apply_coats(scene,coats)
	return scene

func _apply_coats(node: Node, coats: Dictionary) -> void:
	if node is MeshInstance3D:
		for index in range(node.mesh.get_surface_count()):
			var material := node.mesh.surface_get_material(index) as BaseMaterial3D
			if material == null: continue
			var coat: Dictionary = coats.get(material.resource_name.replace(" ","_"),{})
			var weight: float = coat.get("clearcoatFactor",0.0)
			material.clearcoat_enabled = weight > 0.0
			material.clearcoat = weight
			material.clearcoat_roughness = coat.get("clearcoatRoughnessFactor",0.0)
	for child in node.get_children(): _apply_coats(child,coats)

extends RefCounted
## Existing installations keep their settings and saved designs after the rename.

static func environment(suffix: String) -> String:
	var current := "EXA_" + suffix
	return OS.get_environment(current) if OS.has_environment(current) else OS.get_environment("CDDA_" + suffix)

static func user_file(filename: String, directory := "") -> String:
	var root_dir: String = OS.get_user_data_dir() if directory.is_empty() else directory
	var current := root_dir.path_join(filename)
	var legacy := root_dir.get_base_dir().path_join("Dungeon 3D").path_join(filename)
	if not FileAccess.file_exists(current) and not DirAccess.dir_exists_absolute(current) and FileAccess.file_exists(legacy):
		return legacy
	return current

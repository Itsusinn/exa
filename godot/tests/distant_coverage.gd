extends SceneTree
const Cut = preload("res://src/distant_coverage.gd")
var failed := false

func _initialize() -> void: call_deferred("run")

func check(value: bool, message: String) -> void:
	if not value:
		failed = true
		push_error(message)

func run() -> void:
	var parent := Vector3i(1024,16,1024)
	var children := Cut.children(parent)
	var cut = Cut.new()
	var ready := {parent:true}
	var split := {parent:true}
	for child in children:
		cut.configure(ready,{},split,Vector2i.ZERO)
		cut.visit(parent)
		check(cut.active.size()==1 and cut.active.has(parent),"parent holds until all four children, including empty results, are ready")
		ready[child] = true
	cut.configure(ready,{},split,Vector2i.ZERO)
	cut.visit(parent)
	check(cut.active.size()==4 and not cut.active.has(parent),"four children replace parent in one selection")
	ready.erase(parent)
	ready.erase(children[0])
	cut.configure(ready,{},split,Vector2i.ZERO)
	cut.visit(parent)
	check(cut.active.size()==3 and not cut.covered(parent),"missing parent exposes only completed child coverage")
	var fine := Vector3i(-16,1,-16)
	var near_parent := Vector3i(-32,2,-32)
	cut.configure({near_parent:true,fine:true},{},{},Vector2i(-1,-1))
	cut.visit(near_parent)
	check(not cut.active.has(near_parent) and cut.active.has(fine),"coarse volume cannot cut through the fine guard")
	cut.configure({fine:true},{Vector2i(-1,-1):8},{},Vector2i(-1,-1))
	cut.visit(fine)
	check(cut.active.is_empty() and cut.covered(fine),"eight near uploads replace their one metre backup")
	check(cut.present.has(Vector3i(-512,32,-512)),"negative coordinates use floor-aligned ancestors")
	# Deterministic arrival/removal permutations must never expose overlapping levels.
	var random := RandomNumberGenerator.new()
	random.seed = 726411
	for iteration in range(256):
		ready.clear()
		if random.randi_range(0,1)==1: ready[parent] = true
		for child in children:
			if random.randi_range(0,1)==1: ready[child] = true
		cut.configure(ready,{},split,Vector2i.ZERO)
		cut.visit(parent)
		check(not cut.active.has(parent) or cut.active.size()==1,"a visible parent never overlaps a visible descendant")
		var all_children := true
		for child in children: all_children = all_children and ready.has(child)
		check(cut.covered(parent)==(ready.has(parent) or all_children),"coverage follows ready geometry, never requests")
	cut.configure({},{},{},Vector2i.ZERO)
	cut.visit(parent)
	check(cut.active.is_empty() and cut.present.is_empty(),"selection clears between sessions")
	print("GODOT_DISTANT_COVERAGE_RESULT=","FAIL" if failed else "PASS")
	quit(1 if failed else 0)

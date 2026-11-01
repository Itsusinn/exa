extends SceneTree
var failed := false
var a: GameBridge
var b: GameBridge

func check(condition: bool, message: String) -> void:
	if condition: print("PASS: ",message)
	else:
		failed = true
		push_error(message)

func _initialize() -> void: call_deferred("run")

func poll_ready(bridge: GameBridge) -> bool:
	for i in range(300):
		if bridge.advance(0.0,{}).ready: return true
		await create_timer(0.02).timeout
	return false

func step_pair(input: Dictionary = {}) -> void:
	a.advance(1.0/30.0,input)
	b.advance(1.0/30.0,{})
	await create_timer(1.0/30.0).timeout

func run() -> void:
	a = GameBridge.new()
	b = GameBridge.new()
	root.add_child(a)
	root.add_child(b)
	var address := OS.get_environment("CDDA_TEST_ADDRESS")
	var certificate := OS.get_environment("CDDA_CERT_PATH")
	a.set_identity_path(certificate + ".alice.uuid")
	b.set_identity_path(certificate + ".bob.uuid")
	a.connect_game(address,"GodotAlice",certificate)
	var ready_a := await poll_ready(a)
	# Establish the pickup actor first; simultaneous logins can swap spawn slots.
	b.connect_game(address,"GodotBob",certificate)
	var ready_b := await poll_ready(b)
	check(ready_a and ready_b,"two Godot native clients receive authoritative world via TLS QUIC")
	if not ready_a or not ready_b:
		print(a.snapshot().status, " / ",b.snapshot().status)
		quit(1)
		return
	# Verify pickup before jumping away from the two-metre interaction radius.
	for i in range(30):
		await step_pair({"pickup":true})
		if not a.snapshot().inventory.is_empty(): break
	check(not a.snapshot().inventory.is_empty(),"authoritative pickup reaches Godot inventory")
	var start: Vector3 = a.snapshot().body_position
	var peak := start.y
	for i in range(12):
		await step_pair({"forward":1.0,"strafe":1.0,"jump":i == 0,"pickup":true})
		peak = maxf(peak,a.snapshot().body_position.y)
	check(a.snapshot().body_position.z < start.z and peak > start.y + 0.1,"native prediction and authoritative jumping")
	var observed := false
	for actor in b.snapshot().actors:
		if actor.id == a.snapshot().player_id and actor.position.z < start.z: observed = true
	check(observed,"second client observes first client movement")
	for i in range(28): await step_pair()
	a.send_chat("Godot gdext integration verified")
	for i in range(8): await step_pair({"pickup":true})
	check("Godot gdext integration verified" in " ".join(PackedStringArray(b.snapshot().journal)),"chat delivered to second Godot client")
	# Approach the nearest enemy using the same player-input API, then attack.
	var hit := false
	for i in range(240):
		var s := a.snapshot()
		var target: Dictionary = {}
		var nearest := INF
		for actor in s.actors:
			if actor.kind == 1:
				var d: float = actor.position.distance_to(s.body_position)
				if d < nearest:
					nearest = d
					target = actor
		if target.is_empty(): break
		var offset: Vector3 = target.position - s.body_position
		var aim := atan2(offset.x,-offset.z)
		await step_pair({"forward":1.0 if nearest > 2.2 else 0.0,"yaw":aim,"attack":true})
		if "hit a zombie" in " ".join(PackedStringArray(a.snapshot().journal)):
			hit = true
			break
	check(hit,"melee input reaches server and returns combat log")
	var old_id: int = b.snapshot().player_id
	b.disconnect_game()
	for i in range(12):
		a.advance(0.0,{})
		await create_timer(0.03).timeout
	var departed := true
	for actor in a.snapshot().actors:
		if actor.id == old_id: departed = false
	check(departed,"disconnect removes peer from authoritative state")
	b.connect_game(address,"GodotBobAgain",certificate)
	check(await poll_ready(b),"Godot client reconnects")
	check(b.snapshot().player_id != old_id,"reconnection creates fresh session identity")
	a.disconnect_game()
	b.disconnect_game()
	for i in range(200):
		var sa := a.advance(0.0,{})
		var sb := b.advance(0.0,{})
		if not sa.shutdown_pending and not sb.shutdown_pending: break
		await create_timer(0.02).timeout
	check(not a.snapshot().shutdown_pending and not b.snapshot().shutdown_pending,"network workers finish before extension unload")
	a.queue_free()
	b.queue_free()
	await process_frame
	print("GODOT_MULTIPLAYER_RESULT=", "FAIL" if failed else "PASS")
	quit(1 if failed else 0)

var/list/memory_retained = list()

/proc/memory_allocate()
	for(var/i in 1 to 1000)
		var/list/item = list("index" = i, "payload" = "owned-memory-probe-[i]")
		memory_retained += list(item)
	return memory_retained.len

/proc/memory_release()
	memory_retained.Cut()
	return memory_retained.len

/proc/memory_native_hold()
	return text2num(call_ext("allocation_fixture.dll", "hold")())

/proc/memory_native_release()
	return text2num(call_ext("allocation_fixture.dll", "release")())

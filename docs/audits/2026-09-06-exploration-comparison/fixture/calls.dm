/proc/exercise(datum/cell/power, datum/decoy/other)
	power.reset()
	other.reset()
	var/message = "power.reset() is documentation"
	// power.reset() is not a second call.
	return message

/proc/inherited_call(datum/alias/power)
	return power.reset()

/proc/dynamic_call(datum/receiver, method_name)
	return call(receiver, method_name)()

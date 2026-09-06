/datum/cell
	var/charge = 10

/datum/cell/proc/reset()
	charge = 0
	return charge

/datum/cell/special
	charge = 40

/datum/alias
	parent_type = /datum/cell/special

/datum/cell/override/reset()
	charge = 5
	return charge

/datum/decoy/proc/reset()
	return "unrelated"

/datum/configured
#if ENABLE_FAST
	var/mode = "fast"
#else
	var/mode = "slow"
#endif

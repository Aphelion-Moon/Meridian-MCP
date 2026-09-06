/datum/query_base
	var/charge = 7

/datum/query_base/proc/work()
	charge = charge + 1
	return charge

/datum/query_base/local
	charge = 8

/datum/query_base/local/work()
	return ..()

/datum/query_alias
	parent_type = /datum/query_base
	charge = 9

/datum/query_alias/work()
	return ..()

/datum/query_unrelated
	var/charge = 20

/datum/query_unrelated/proc/work()
	return charge

/datum/query_base/redirected
	parent_type = /datum/query_unrelated

/datum/query_base/redirected/work()
	return ..()

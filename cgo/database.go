package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/database.h"
import "C"

type RrdHost struct {
	c_host C.RRDHOSTP
}

type RrdSet struct {
	c_set C.RRDSETP
}

func NewLocalHost() RrdHost {
	return RrdHost{c_host: C.localhost}
}

func (rh *RrdHost) HostName() string {
	return C.GoString(C.rrdhostp_hostname(rh.c_host))
}

func (rh *RrdHost) ReadLock() {
	C.rrdhostp_rdlock(rh.c_host)
}

func (rh *RrdHost) UnLock() {
	C.rrdhostp_unlock(rh.c_host)
}

func (rh *RrdHost) RootSet() RrdSet {
	return RrdSet{c_set: C.rrdhostp_root_set(rh.c_host)}
}

func (rs *RrdSet) NextSet() RrdSet {
	return RrdSet{c_set: C.rrdsetp_next_set(rs.c_set)}
}

func (rs *RrdSet) Name() string {
	return C.GoString(C.rrdsetp_name(rs.c_set))
}

func (rh *RrdHost) Sets() []RrdSet {
	rh.ReadLock()
	defer rh.UnLock()

	sets := []RrdSet{}
	rs := rh.RootSet()

	for rs.c_set != nil {
		sets = append(sets, rs)
		rs = rs.NextSet()
	}

	return sets
}

func main() {}

package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/database.h"
import "C"

import (
	"unsafe"
)

type RrdHost struct {
	c_host C.RRDHOSTP
}

type RrdSet struct {
	c_set C.RRDSETP
}

type RrdResult struct {
	c_res C.RRDRP
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

func (rs *RrdSet) UpdateEvery() int {
	return int(C.rrdsetp_update_every(rs.c_set))
}

func (rs *RrdSet) NumDims() int {
	return int(C.rrdsetp_num_dims(rs.c_set))
}

func (rs *RrdSet) ReadLock() {
	C.rrdsetp_rdlock(rs.c_set)
}

func (rs *RrdSet) UnLock() {
	C.rrdsetp_unlock(rs.c_set)
}

func (rs *RrdSet) GetResult(NumSamples int) *RrdResult {
	c_res := C.rrdrp_get(rs.c_set, C.int(NumSamples))
	return &RrdResult{c_res: c_res}
}

func (res *RrdResult) NumRows() int {
	return int(C.rrdrp_num_rows(res.c_res))
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

func ConfigGetNum(section string, name string, value int) int {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	return int(C.cfg_get_number(csection, cname, C.longlong(value)))
}

func main() {}

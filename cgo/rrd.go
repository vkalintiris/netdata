package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/cgo-rrd.h"
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

type RrdDim struct {
	c_dim C.RRDDIMP
}

type RrdResult struct {
	c_res C.RRDRP
}

type KMeans struct {
	c_kmref C.KMREF
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

func KMeansNew(NumCenters int) KMeans {
	return KMeans{c_kmref: C.kmref_new(C.int(NumCenters))}
}

func (km *KMeans) Train(Res RrdResult, DiffN int, SmoothN int, LagN int) {
	C.kmref_train(km.c_kmref, Res.c_res, C.int(DiffN), C.int(SmoothN), C.int(LagN))
}

func (km *KMeans) Predict(Res RrdResult, DiffN int, SmoothN int, LagN int) float64 {
	return float64(C.kmref_predict(km.c_kmref, Res.c_res, C.int(DiffN), C.int(SmoothN), C.int(LagN)))
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

func (rs *RrdSet) GetResult(NumSamples int) RrdResult {
	c_res := C.rrdrp_get(rs.c_set, C.int(NumSamples))
	return RrdResult{c_res: c_res}
}

func (res *RrdResult) NumRows() int {
	return int(C.rrdrp_num_rows(res.c_res))
}

func (res *RrdResult) Free() {
	C.rrdrp_free(res.c_res)
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

func (rh *RrdHost) CreateRrdSet(
	ty, id, name, family, context, title, units, plugin, module string,
	priority, update_every int) RrdSet {
	c_ty := C.CString(ty)
	c_id := C.CString(id)
	c_name := C.CString(name)
	c_family := C.CString(family)
	c_context := C.CString(context)
	c_title := C.CString(title)
	c_units := C.CString(units)
	c_plugin := C.CString(plugin)
	c_module := C.CString(module)

	c_set := C.rrdsetp_create(
		c_ty, c_id, c_name, c_family, c_context,
		c_title, c_units, c_plugin, c_module,
		C.long(priority), C.int(update_every),
	)

	return RrdSet{c_set: c_set}
}

func (rs *RrdSet) AddDim(id string, name string) RrdDim {
	return RrdDim{
		c_dim: C.rrdsetp_add_dim(rs.c_set, C.CString(id), C.CString(name)),
	}
}

func ConfigGetNum(section string, name string, value int) int {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	return int(C.cfg_get_number(csection, cname, C.longlong(value)))
}

func main() {}

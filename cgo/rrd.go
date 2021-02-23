package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/cgo-rrd.h"
import "C"
import "unsafe"

type RrdHost struct{ c_host C.RRDHOSTP }
type RrdSet struct{ c_set C.RRDSETP }
type RrdDim struct{ c_dim C.RRDDIMP }
type RrdResult struct{ c_res C.RRDRP }

/*
 * RRD Host
 */

func LocalHostRef() RrdHost {
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

func (rh *RrdHost) Sets() []RrdSet {
	rh.ReadLock()
	defer rh.UnLock()

	sets := []RrdSet{}
	rs := RrdSet{c_set: C.rrdhostp_root_set(rh.c_host)}

	for rs.c_set != nil {
		sets = append(sets, rs)
		rs = RrdSet{c_set: C.rrdsetp_next_set(rs.c_set)}
	}

	return sets
}

func (rh *RrdHost) CreateRrdSet(
	ty, id, name, family, context, title, units, plugin, module string,
	priority, update_every int) RrdSet {
	c_ty := C.CString(ty)
	defer C.free(unsafe.Pointer(c_ty))

	c_id := C.CString(id)
	defer C.free(unsafe.Pointer(c_id))

	c_name := C.CString(name)
	defer C.free(unsafe.Pointer(c_name))

	c_family := C.CString(family)
	defer C.free(unsafe.Pointer(c_family))

	c_context := C.CString(context)
	defer C.free(unsafe.Pointer(c_context))

	c_title := C.CString(title)
	defer C.free(unsafe.Pointer(c_title))

	c_units := C.CString(units)
	defer C.free(unsafe.Pointer(c_units))

	c_plugin := C.CString(plugin)
	defer C.free(unsafe.Pointer(c_plugin))

	c_module := C.CString(module)
	defer C.free(unsafe.Pointer(c_module))

	c_set := C.rrdsetp_create(
		c_ty, c_id, c_name, c_family, c_context,
		c_title, c_units, c_plugin, c_module,
		C.long(priority), C.int(update_every),
	)

	return RrdSet{c_set: c_set}
}

/*
 * RRD Set
 */

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

func (rs *RrdSet) AddDim(id string, name string) RrdDim {
	return RrdDim{
		c_dim: C.rrdsetp_add_dim(rs.c_set, C.CString(id), C.CString(name)),
	}
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

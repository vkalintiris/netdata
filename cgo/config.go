package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/cgo-config.h"
// #include <stdlib.h>
import "C"

import (
	"unsafe"
)

func ConfigGetNum(section string, name string, value int) int {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	return int(C.cfg_get_number(csection, cname, C.longlong(value)))
}

func main() {}

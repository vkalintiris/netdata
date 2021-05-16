package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

import (
	"time"
	"unsafe"
)

type Config struct {
	cptr *C.struct_config
}

func (c *Config) GetString(section, name, defaultValue string) string {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	cdefaultValue := C.CString(defaultValue)
	defer C.free(unsafe.Pointer(cdefaultValue))

	cres := C.appconfig_get(c.cptr, csection, cname, cdefaultValue)
	return C.GoString(cres)
}

func (c *Config) GetInt(section, name string, defaultValue int) int {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	cdefaultValue := C.longlong(defaultValue)

	cres := C.appconfig_get_number(c.cptr, csection, cname, cdefaultValue)
	return int(cres)
}

func (c *Config) GetFloat(section, name string, defaultValue float64) float64 {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	cdefaultValue := C.double(defaultValue)

	cres := C.appconfig_get_float(c.cptr, csection, cname, cdefaultValue)
	return float64(cres)
}

func (c *Config) GetDuration(section, name, defaultValue string) time.Duration {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	cdefaultValue := C.CString(defaultValue)
	defer C.free(unsafe.Pointer(cdefaultValue))

	cres := C.appconfig_get_duration(c.cptr, csection, cname, cdefaultValue)
	return time.Duration(int(cres)) * time.Second
}

func (c *Config) GetBool(section, name string, defaultValue bool) bool {
	csection := C.CString(section)
	defer C.free(unsafe.Pointer(csection))

	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))

	var cdefaultValue C.int
	if defaultValue {
		cdefaultValue = C.int(1)
	} else {
		cdefaultValue = C.int(0)
	}

	cres := C.appconfig_get_boolean(c.cptr, csection, cname, cdefaultValue)
	if int(cres) == 0 {
		return false
	}
	return true
}

func GetNetdataConfig() *Config {
	return &Config{cptr: &C.netdata_config}
}

package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

type Calc struct {
	cptr *C.struct_rrdcalc
}

const (
	CalcFlagDbError             = 0x00000001
	CalcFlagDbNan               = 0x00000002
	CalcFlagCalcError           = 0x00000008
	CalcFlagWarnError           = 0x00000010
	CalcFlagCritError           = 0x00000020
	CalcFlagRunnable            = 0x00000040
	CalcFlagDisabled            = 0x00000080
	CalcFlagSilenced            = 0x00000100
	CalcFlagRunOnce             = 0x00000200
	CalcFlagNoClearNotification = 0x80000000
)

func (c *Calc) IsSilenced() bool {
	return (c.cptr.rrdcalc_flags & CalcFlagDisabled) != 0
}

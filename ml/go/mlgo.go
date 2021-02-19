package main

// #cgo LDFLAGS: ml-cgo.o
// #include "ml-cgo.h"
import "C"

import (
	"fmt"
	"os"
)

//export GoHelloWorld
func GoHelloWorld() {
	fp, err := os.OpenFile("/tmp/text.log", os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		panic(err)
	}
	defer fp.Close()

	name := C.GoString(C.rrdset_name(C.curr_set))
	num_dims := C.rrdset_num_dims(C.curr_set)
	update_every := C.rrdset_update_every(C.curr_set)

	fmt.Fprintf(fp, "chart %s\n", name)
	fmt.Fprintf(fp, "\tnum dims: %d\n", num_dims)
	fmt.Fprintf(fp, "\tupdate every: %d\n", update_every)
}

func main() {}

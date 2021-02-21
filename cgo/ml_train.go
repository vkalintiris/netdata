package main

import "C"

import (
	"fmt"
	"os"
)

//export GoMLTrain
func GoMLTrain() {
	flags := os.O_APPEND | os.O_CREATE | os.O_WRONLY
	fp, err := os.OpenFile("/tmp/go.log", flags, 0644)
	if err != nil {
		panic(err)
	}
	defer fp.Close()

	fmt.Fprintf(fp, "Hello from GO\n")
}

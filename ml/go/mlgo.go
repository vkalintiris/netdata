package main

// #include "ml-cgo.h"
import "C"

import (
	"io/ioutil"
)

//export GoHelloWorld
func GoHelloWorld() {
	d1 := []byte("hello\ngo\n")
	err := ioutil.WriteFile("/tmp/ml-go.txt", d1, 0644)
	if err != nil {
		panic("Tsimpa")
	}
}

func main() {}

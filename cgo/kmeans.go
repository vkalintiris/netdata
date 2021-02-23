package main

// #include "bindings/cgo-kmeans.h"
import "C"

type KMeans struct {
	c_kmref C.KMREF
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

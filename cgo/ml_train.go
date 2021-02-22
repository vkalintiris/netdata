package main

import "C"

import (
	"log"
	"os"
	"time"
)

func redirectLog(path string) *os.File {
	flags := os.O_APPEND | os.O_CREATE | os.O_WRONLY

	fp, err := os.OpenFile(path, flags, 0664)
	if err != nil {
		log.Fatal(err)
	}

	log.SetOutput(fp)
	return fp
}

type MlConfig struct {
	NumSamples int
	TrainEvery time.Duration

	DiffN   int
	SmoothN int
	LagN    int
}

func NewMlConfig() *MlConfig {
	var mlc MlConfig

	mlc.NumSamples = ConfigGetNum("ml", "num samples to train", 600)
	mlc.TrainEvery = time.Duration(ConfigGetNum("ml", "train every secs", 60)) * time.Second

	mlc.DiffN = ConfigGetNum("ml", "num samples to diff", 1)
	mlc.SmoothN = ConfigGetNum("ml", "num samples to smooth", 3)
	mlc.LagN = ConfigGetNum("ml", "num samples to lag", 5)

	return &mlc
}

type MlChart struct {
	Config        *MlConfig
	Set           RrdSet
	Name          string
	KM            KMeans
	LastTrainedAt time.Time
}

func NewMlChart(mlc *MlConfig, set RrdSet, name string) *MlChart {
	return &MlChart{
		Config:        mlc,
		Set:           set,
		Name:          name,
		KM:            KMeansNew(2),
		LastTrainedAt: time.Now(),
	}
}

func (chart *MlChart) Train() bool {
	set := chart.Set
	cfg := chart.Config

	if set.NumDims() == 0 {
		log.Printf("Skipping %s because it has 0 dims\n", chart.Name)
		return false
	}

	if set.UpdateEvery() != 1 {
		log.Printf("Skipping %s because it has update every %d\n", chart.Name, set.UpdateEvery())
		return false
	}

	res := set.GetResult(cfg.NumSamples)
	if res == nil {
		log.Printf("Skipping %s because it has empty result", chart.Name)
		return false
	}

	if cfg.NumSamples-res.NumRows() > 2 {
		log.Printf("Skipping %s because it has %d/%d rows\n", chart.Name, res.NumRows(), cfg.NumSamples)
		res.Free()
		return false
	}

	log.Printf("Training %s with %d rows", chart.Name, res.NumRows())

	chart.KM.Train(res, cfg.DiffN, cfg.SmoothN, cfg.LagN)
	chart.LastTrainedAt = time.Now()

	return true
}

//export GoMLTrain
func GoMLTrain() {
	fp := redirectLog("/tmp/go.log")
	defer fp.Close()

	log.Printf("Heartbeat\n")

	cfg := NewMlConfig()
	charts := map[string]*MlChart{}

	for _ = range time.Tick(10 * time.Second) {
		log.Printf("Loop start\n")

		localhost := NewLocalHost()
		for _, set := range localhost.Sets() {
			name := set.Name()

			if _, ok := charts[name]; !ok {
				log.Printf("Adding new chart %s\n", name)
				charts[name] = NewMlChart(cfg, set, name)
			}
		}

		log.Printf("Have %d charts\n", len(charts))

		for _, chart := range charts {
			elapsed := time.Now().Sub(chart.LastTrainedAt)
			if elapsed < chart.Config.TrainEvery {
				continue
			}

			chart.Train()
		}
		log.Printf("Loop end\n")
	}
}

package main

import "C"

import (
	"log"
	"os"
	_ "sync"
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

	Localhost           RrdHost
	AnomalyDetectionSet RrdSet
	NvmeDim             RrdDim
}

func NewMlConfig() *MlConfig {
	var mlc MlConfig

	mlc.NumSamples = ConfigGetNum("ml", "num samples to train", 120)
	mlc.TrainEvery = time.Duration(ConfigGetNum("ml", "train every secs", 30)) * time.Second

	mlc.DiffN = ConfigGetNum("ml", "num samples to diff", 1)
	mlc.SmoothN = ConfigGetNum("ml", "num samples to smooth", 3)
	mlc.LagN = ConfigGetNum("ml", "num samples to lag", 5)

	localhost := LocalHostRef()
	mlc.AnomalyDetectionSet = localhost.CreateRrdSet(
		"ml", "st_id", "anomaly_detection", "st_family",
		"st_context", "st_title", "st_units", "st_plugin", "st_module",
		1, 1,
	)

	return &mlc
}

type MlChart struct {
	Config        *MlConfig
	Set           RrdSet
	Name          string
	KM            KMeans
	LastTrainedAt time.Time
	AnomalyScore  float64
}

func NewMlChart(mlc *MlConfig, set RrdSet, name string) *MlChart {
	return &MlChart{
		Config:        mlc,
		Set:           set,
		Name:          name,
		KM:            KMeansNew(2),
		LastTrainedAt: time.Now(),
		AnomalyScore:  -1.0,
	}
}

func (chart *MlChart) InBlockList() bool {
	blocklistedNames := []string{
		"apps.cpu", "apps.cpu_system", "apps.cpu_user", "apps.files",
		"apps.lreads", "apps.lwrites", "apps.major_faults", "apps.mem",
		"apps.minor_faults", "apps.pipes", "apps.preads", "apps.processes",
		"apps.pwrites", "apps.sockets", "apps.swap", "apps.threads",
		"apps.uptime_avg", "apps.uptime", "apps.uptime_max", "apps.uptime_min",
		"apps.vmem", "services.cpu", "services.mem_usage",
		"services.throttle_io_ops_read", "services.throttle_io_ops_write",
		"services.throttle_io_read", "services.throttle_io_write",
	}

	for _, name := range blocklistedNames {
		if chart.Name == name {
			return true
		}
	}
	return false
}

func (chart *MlChart) Train() bool {
	set := chart.Set
	cfg := chart.Config

	if chart.InBlockList() {
		return false
	}

	res := set.GetResult(cfg.NumSamples)
	if res.c_res == nil {
		log.Printf("Not training %s because it has empty result", chart.Name)
		return false
	}

	if cfg.NumSamples-res.NumRows() > 2 {
		log.Printf("Not training %s because it has %d/%d rows\n", chart.Name, res.NumRows(), cfg.NumSamples)
		res.Free()
		return false
	}

	log.Printf("Training %s with %d rows", chart.Name, res.NumRows())
	chart.KM.Train(res, cfg.DiffN, cfg.SmoothN, cfg.LagN)
	chart.LastTrainedAt = time.Now()

	return true
}

func (chart *MlChart) Predict() bool {
	set := chart.Set
	cfg := chart.Config
	numSamples := cfg.DiffN + cfg.SmoothN + cfg.LagN

	if chart.InBlockList() {
		return false
	}

	res := set.GetResult(numSamples)
	if res.c_res == nil {
		log.Printf("Not predicting %s because it has empty result", chart.Name)
		return false
	}

	if numSamples-res.NumRows() > 1 {
		log.Printf("Not predicting %s because it has %d/%d rows\n", chart.Name, res.NumRows(), numSamples)
		res.Free()
		return false
	}

	log.Printf("Predicting %s with %d rows", chart.Name, res.NumRows())
	chart.AnomalyScore = chart.KM.Predict(res, cfg.DiffN, cfg.SmoothN, cfg.LagN)

	return true
}

func GoMLTrain(cfg *MlConfig, charts map[string]*MlChart) {
	for _ = range time.Tick(15 * time.Second) {
		// Collect new charts
		localhost := LocalHostRef()
		for _, set := range localhost.Sets() {
			name := set.Name()

			if _, ok := charts[name]; !ok {
				charts[name] = NewMlChart(cfg, set, name)
			}
		}

		// Filter charts to train
		chartsToTrain := []*MlChart{}
		for _, chart := range charts {
			elapsed := time.Now().Sub(chart.LastTrainedAt)
			if elapsed > chart.Config.TrainEvery {
				chartsToTrain = append(chartsToTrain, chart)
			}
		}

		// Train charts
		for _, chart := range chartsToTrain {
			chart.Train()
		}
	}
}

func GoMLPredict(cfg *MlConfig, charts map[string]*MlChart) {
	for _ = range time.Tick(1 * time.Second) {
		chart, ok := charts["system.cpu"]
		if !ok {
			continue
		}

		chart.Predict()

		log.Printf("%s has anomaly score %f\n", chart.Name, chart.AnomalyScore)
	}
}

//export GoMLMain
func GoMLMain() {
	fp := redirectLog("/tmp/go.log")
	defer fp.Close()

	cfg := NewMlConfig()
	charts := map[string]*MlChart{}

	GoMLTrain(cfg, charts)

	/*
		var wg sync.WaitGroup
		wg.Add(2)

		cfg := NewMlConfig()
		charts := map[string]*MlChart{}

		go func() { GoMLTrain(cfg, charts); wg.Done() }()
		go func() { GoMLPredict(cfg, charts); wg.Done() }()

		wg.Wait()
	*/
}

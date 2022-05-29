package main

import (
	"bufio"
	"bytes"
	"container/list"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/textproto"
	"net/url"
	"strings"
	"syscall"
	"time"

	"github.com/golang/protobuf/proto"
	"github.com/google/uuid"
	"github.com/ian-kent/linkio"
	"github.com/vkalintiris/ndstream/github.com/vkalintiris/replication"
)

var SpaceUUID uuid.UUID
var version4 []byte = []byte("Hit me baby, push them over with the version=4")

func init() {
	var err error
	SpaceUUID, err = uuid.Parse("377eb935-9b5f-3591-b4d6-367c64361ce1")
	if err != nil {
		log.Fatal(err)
	}
}

type Agent struct {
	uid     int
	sysInfo url.Values

	queue      *list.List
	dstConn    net.Conn
	startAfter time.Time
}

func NewAgent(uid int, sysInfo url.Values) *Agent {
	u := url.URL{
		RawQuery: sysInfo.Encode(),
	}
	sysInfoCp := u.Query()

	hostname := fmt.Sprintf("gond%d", uid)
	registryHostname := fmt.Sprintf("gond%d", uid)

	guid := uuid.NewSHA1(SpaceUUID, []byte(hostname))
	machine_guid := guid.String()

	sysInfoCp["hostname"][0] = hostname
	sysInfoCp["registry_hostname"][0] = registryHostname
	sysInfoCp["machine_guid"][0] = machine_guid
	sysInfoCp["hops"][0] = "0"

	startAfter := time.Now().Add(time.Duration(int64(10*uid) * int64(time.Second)))

	return &Agent{
		uid:        uid,
		sysInfo:    sysInfoCp,
		queue:      list.New(),
		startAfter: startAfter,
	}
}

func (a *Agent) Connect(address string) error {
	dstConn, err := net.Dial("tcp", address)
	if err != nil {
		log.Fatal(err)
	}

	a.dstConn = dstConn

	textConn := textproto.NewConn(dstConn)
	textConn.PrintfLine("STREAM %s HTTP/1.1", a.sysInfo.Encode())
	textConn.PrintfLine("User-Agent: netdata/v1.33.1-9-deadbeef")
	textConn.PrintfLine("Accept: */*")
	textConn.PrintfLine("")

	// Response is not line oriented
	versionBytes := make([]byte, 8192)
	n, err := dstConn.Read(versionBytes)
	if err != nil {
		log.Fatal(err)
	}
	if n == 0 {
		log.Fatal("Received empty response")
	}

	if bytes.Compare(versionBytes[:len(version4)], version4[:]) != 0 {
		log.Fatalf("Got wrong version from remote:\n%s\n", versionBytes)
	}

	return nil
}

func (a *Agent) AddSlice(buf []byte) {
	log.Printf("[%d] Pushing slice of size %d", a.uid, len(buf))
	a.queue.PushBack(buf)
}

func (a *Agent) Send(Now time.Time) error {
	/*
	   if Now.Before(a.startAfter) {
	       return nil
	   }
	*/

	if a.dstConn == nil {
		a.Connect(":19999")
	}

	e := a.queue.Back()
	buf := e.Value.([]byte)
	a.queue.Remove(e)

	log.Printf("[%d] Pop'd slice of %d bytes\n", a.uid, len(buf))

	n, err := a.dstConn.Write(buf)
	if err != nil {
		if err == io.EOF {
			log.Fatalf("EOF for agent %d", a.uid)
		}

		if errors.Is(err, syscall.EPIPE) {
			log.Printf("Broken pipe for agent %d: %s", a.uid, err)
		}
	}
	if n < len(buf) {
		log.Fatalf("Short write for %d", a.uid)
	}

	return nil
}

func getSystemInfo(conn net.Conn) (url.Values, error) {
	textConn := textproto.NewConn(conn)

	var streamLine string
	for i := 0; i != 3; i += 1 {
		line, err := textConn.ReadLine()
		if err != nil {
			return nil, err
		}

		if i == 0 {
			streamLine = line
		}
	}

	scanner := bufio.NewScanner(strings.NewReader(streamLine))
	scanner.Split(bufio.ScanWords)

	words := []string{}
	for scanner.Scan() {
		words = append(words, scanner.Text())
	}

	if err := scanner.Err(); err != nil {
		return nil, err
	}

	for i, word := range words {
		fmt.Printf("[%d]: %s\n", i, word)
	}

	values, err := url.ParseQuery(words[1])
	if err != nil {
		return nil, err
	}

	// write version string before we return so that we can start
	// reading data straight away
	_, err = conn.Write(version4)
	if err != nil {
		return nil, err
	}

	log.Printf("Will read gaps!\n")

	var gapsBytes [5]byte
	_, err = conn.Read(gapsBytes[:])
	if err != nil {
		return nil, err
	}

	gapsExpectedBytes := []byte{71, 65, 80, 83, 0}

	if bytes.Compare(gapsBytes[:], gapsExpectedBytes) != 0 {
		log.Fatalf("Got wrong GAPS response")
	}

	now := time.Now()

	before := now.Unix()
	after := before - 10
	tr1 := replication.TimeRange{After: after, Before: before}

    /*
	before = before - 20
	after = before - 20
	tr2 := replication.TimeRange{After: after, Before: before}
    */

	trs := replication.TimeRanges{}
	trs.TRs = append(trs.TRs, &tr1)
	// trs.TRs = append(trs.TRs, &tr2)

	data, err := proto.Marshal(&trs)
	if err != nil {
		panic(err)
	}

	resp := [8192]byte{}
	for idx := range data {
		resp[idx] = data[idx]
	}

	_, err = conn.Write(resp[:])
	if err != nil {
		return nil, err
	}

	for {
		line, err := textConn.ReadLine()
		if err != nil {
			return nil, err
		}
        fmt.Printf("%s\n", line)
    }
        /*
	link := linkio.NewLink(64 * linkio.KilobytePerSecond)
	linkReader := link.NewLinkReader(io.Reader(conn))


	// bufConn := bufio.NewReader(linkReader)
	for {
		buf := make([]byte, 512*1024)
		n, err := bufConn.Read(buf)
		if err != nil {
			log.Fatal(err)
		}
		if err == io.EOF {
			break
		}

		fmt.Printf("Read %d bytes from %s\n", n, conn.RemoteAddr())
        fmt.Printf("%+v\n", buf[:n])
	}
    */

	log.Fatalf("ALL GOOD (%+v)\n", gapsBytes)

	return values, nil
}

func handleConn(conn net.Conn) {
	sysInfo, err := getSystemInfo(conn)
	if err != nil {
		if err == io.EOF {
			log.Fatal("EOF after SystemInfo")
		}
	}

	return

	agents := []*Agent{}
	for i := 0; i != 1; i++ {
		agents = append(agents, NewAgent(i, sysInfo))
	}

	// Send buffers
	link := linkio.NewLink(64 * linkio.KilobitPerSecond)
	linkReader := link.NewLinkReader(io.Reader(conn))

	bufConn := bufio.NewReader(linkReader)
	for {
		buf := make([]byte, 512*1024)
		n, err := bufConn.Read(buf)
		if err != nil {
			log.Fatal(err)
		}
		if err == io.EOF {
			break
		}
		if n == 0 {
			log.Printf("Read 0 bytes from %s", conn.RemoteAddr())
			continue
		}
		buf = buf[:n]

		now := time.Now()
		for _, agent := range agents {
			agent.AddSlice(buf)
			agent.Send(now)
		}
	}
}

func main() {
	l, err := net.Listen("tcp", ":19999")
	if err != nil {
		log.Fatal(err)
	}
	defer l.Close()

	for {
		conn, err := l.Accept()
		if err != nil {
			log.Fatal(err)
		}
		handleConn(conn)
	}

	log.Fatal("Lost connection")
}

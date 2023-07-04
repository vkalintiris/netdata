package main

import (
	"bytes"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"strconv"
	"strings"
	// "time"
	"github.com/google/uuid"
)

const (
	HOST                      = "localhost"
	PORT                      = "20000"
	TYPE                      = "tcp"
	START_STREAMING_PROMPT_VN = "Hit me baby, push them over with the version="
)

type HttpHeader struct {
	buffer   []byte
	hostname string
	guid     string
}

func newHttpHeader(buffer []byte) *HttpHeader {
	return &HttpHeader{
		buffer: buffer,
	}
}

func (hdr *HttpHeader) version() int {
	tokens := strings.Split(string(hdr.buffer), "&")
	for _, t := range tokens {
		if strings.HasPrefix(t, "ver=") {
			ver, err := strconv.Atoi(t[len("ver="):])

			if err != nil {
				log.Fatal("Could not decode version")
				return 0
			}

			// clear the replication bit
			return ver & ^(1 << 12)
		}
	}

	log.Fatal("Could not decode version")
	return 0
}

func (hdr *HttpHeader) formatWith(hostname string, guid string) []byte {
	tokens := strings.Split(string(hdr.buffer), "&")

	tokens[1] = fmt.Sprintf("hostname=%s", hostname)
	tokens[2] = fmt.Sprintf("registry_hostname=%s", hostname)
	tokens[3] = fmt.Sprintf("machine_guid=%s", guid)

	s := strings.Join(tokens, "&")
	return bytes.Trim([]byte(s), "\x00")
}

func handleHttpHeader(conn net.Conn) *HttpHeader {
	buffer := make([]byte, 8192)
	_, err := conn.Read(buffer)
	if err != nil {
		log.Fatal(err)
	}

	hdr := newHttpHeader(buffer)

	initial_response := fmt.Sprintf("%s%d", START_STREAMING_PROMPT_VN, hdr.version())

	var buf bytes.Buffer
	buf.WriteString(initial_response)
	conn.Write([]byte(buf.String()))

	return hdr
}

type Client struct {
	conn net.Conn
	hdr  *HttpHeader
}

func (hdr *HttpHeader) dial(address string, hostname string, guid string) net.Conn {
	clientConn, err := net.Dial("tcp", address)
	if err != nil {
		log.Fatalf("Dial failed: %s", err.Error())
	}

	newHdr := hdr.formatWith(hostname, guid)
	clientConn.Write(newHdr)

	parentResp := make([]byte, 60, 4096)
	n, err := clientConn.Read(parentResp)
	if err != nil {
		log.Fatalf("client err: %s", err)
	}
	log.Printf("Received %d bytes from client connection: >>>%s<<<", n, parentResp)

	return clientConn
}

func getConnections(hdr *HttpHeader, n int) []net.Conn {
	connections := make([]net.Conn, n)

	for i := 0; i != n; i += 1 {
		hostname := fmt.Sprintf("child-%04d", i+1)
		guid := uuid.New()
		connections[i] = hdr.dial("localhost:19999", hostname, guid.String())
	}

	return connections
}

func handleIncomingRequest(conn net.Conn) {
	log.Println("Handling incoming connection...")

	hdr := handleHttpHeader(conn)

	n := 10
	connections := getConnections(hdr, n)

	writers := make([]io.Writer, n)
	for i := 0; i != n; i += 1 {
		writers[i] = connections[i]
	}
	multiWriter := io.MultiWriter(writers...)

	for {
		bytes, err := io.Copy(multiWriter, conn)
		if err != nil {
			log.Fatal(err)
		}

		log.Printf("Copied %d bytes\n", bytes)
	}

	// close conn
	// clientConn.Close()
	// conn.Close()
}

func main() {
	listen, err := net.Listen(TYPE, HOST+":"+PORT)
	if err != nil {
		log.Fatal(err)
		os.Exit(1)
	}
	defer listen.Close()

	connected := false
	for {
		conn, err := listen.Accept()
		if err != nil {
			log.Fatal(err)
			os.Exit(1)
		}

		if connected {
			log.Printf("Real-agent already streaming to us.")
			conn.Close()
			continue
		}

		connected = true
		go handleIncomingRequest(conn)
	}
}

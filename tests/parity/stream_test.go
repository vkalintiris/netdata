// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// streamRequest is a STREAM request line as children send it.
func streamRequest(query string) []byte {
	return []byte("STREAM " + query + " HTTP/1.1\r\nUser-Agent: parity-child/1.0\r\nAccept: */*\r\n\r\n")
}

// TestStreamHandshake compares the receiver's answers to STREAM requests: accepted prompts per protocol version,
// every refusal decided before the host exists, the parent's own GUID, and a second connection for a GUID that is
// still connected, then the stream.conf rows. Only `v5` and the `compression` row negotiate a compression (C answers
// LZ4 for version 5, zstd for the capabilities); no data follows the prompt.
func TestStreamHandshake(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, StreamExtra: hsConfSections}, parentIdentity)
	key := parentIdentity.StreamKey
	guid := func(n int) string { return fmt.Sprintf("5a1e0000-0000-4000-8000-%012d", 100+n) }
	query := func(n int, extra string) string {
		return fmt.Sprintf("key=%s&hostname=hs-%d&machine_guid=%s&update_every=1%s", key, n, guid(n), extra)
	}
	steps := []struct {
		name    string
		request []byte
	}{
		{"vcaps", streamRequest(query(1, "&ver=17088&NETDATA_PROTOCOL_VERSION=1.1"))},
		{"v1", streamRequest(query(2, "&ver=1"))},
		{"v2", streamRequest(query(3, "&ver=2"))},
		{"vn4", streamRequest(query(4, "&ver=4"))},
		{"protocol-version-before-ver", streamRequest(query(5, "&NETDATA_PROTOCOL_VERSION=1.1&ver=17088"))},
		{"replication-caps", streamRequest(query(6, "&ver=21184"))},
		{"no-key", streamRequest(fmt.Sprintf("hostname=x&machine_guid=%s&ver=17088", guid(7)))},
		{"no-hostname", streamRequest(fmt.Sprintf("key=%s&machine_guid=%s&ver=17088", key, guid(8)))},
		{"no-guid", streamRequest(fmt.Sprintf("key=%s&hostname=x&ver=17088", key))},
		{"key-not-enabled", streamRequest(fmt.Sprintf("key=%s&hostname=x&machine_guid=%s&ver=17088", guid(9), guid(10)))},
		{"invalid-hops", streamRequest(query(11, "&ver=17088&hops=0"))},
		{"own-guid", streamRequest(fmt.Sprintf("key=%s&hostname=x&machine_guid=%s&ver=17088", key, parentIdentity.MachineGUID))},
		{"no-ver", streamRequest(query(16, ""))},
		{"v3", streamRequest(query(17, "&ver=3"))},
		{"v5", streamRequest(query(18, "&ver=5"))},
		{"vcaps-low-bits", streamRequest(query(19, "&ver=17095"))},
		{"ver-bit-31", streamRequest(query(20, "&ver=2147500736"))},
		{"ver-bit-30", streamRequest(query(60, "&ver=1073758912&ver=4"))},
	}
	oracleAnswers := map[string]string{
		"no-ver":         "Hit me baby, push them over...<timeout>",
		"v3":             "Hit me baby, push them over with the version=3<timeout>",
		"v5":             "Hit me baby, push them over with the version=5<timeout>",
		"vcaps-low-bits": "Hit me baby, push them over with the version=17088<timeout>",
		"ver-bit-31":     "Hit me baby, push them over...<timeout>",
		"ver-bit-30":     "Hit me baby, push them over with the version=17088<timeout>",
	}
	for _, step := range steps {
		t.Run(step.name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, step.request, time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = b
			}
			if want, ok := oracleAnswers[step.name]; ok && string(got[0]) != want {
				t.Errorf("oracle answered %q, want %q", got[0], want)
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
			}
		})
	}

	// A second connection while the first one is still attached (and quiet for less than 30 s) is refused.
	t.Run("already-streaming", func(t *testing.T) {
		var got [2][]byte
		for i, side := range p.Each() {
			held, err := net.Dial("tcp", side.Daemon.Addr)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			defer held.Close()
			if _, err := held.Write(streamRequest(query(12, "&ver=17088"))); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			prompt := make([]byte, 128)
			_ = held.SetReadDeadline(time.Now().Add(2 * time.Second))
			n, _ := held.Read(prompt)
			second, err := rawExchange(side.Daemon.Addr, streamRequest(query(12, "&ver=17088")), time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			got[i] = append(append(prompt[:n:n], '|'), second...)
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
		}
	})
	// the request's GUID is kept whole until the host is created with its first 37 characters (D126.7): a longer one
	// is accepted once, then meets the host it made in the index (busy); a 37-character one behaves as any other; the
	// parent's own GUID with a suffix is a child
	t.Run("suffixed-guid", func(t *testing.T) {
		var got [2][]byte
		for i, side := range p.Each() {
			var parts [][]byte
			for _, g := range []string{guid(13) + "xyz", guid(13) + "xyz"} {
				b, err := rawExchange(side.Daemon.Addr, streamRequest(fmt.Sprintf(
					"key=%s&hostname=hs-13&machine_guid=%s&update_every=1&ver=17088", key, g)), time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				parts = append(parts, b)
			}
			held, err := net.Dial("tcp", side.Daemon.Addr)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			defer held.Close()
			q37 := fmt.Sprintf("key=%s&hostname=hs-14&machine_guid=%sx&update_every=1&ver=17088", key, guid(14))
			if _, err := held.Write(streamRequest(q37)); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			prompt := make([]byte, 128)
			_ = held.SetReadDeadline(time.Now().Add(2 * time.Second))
			n, _ := held.Read(prompt)
			second, err := rawExchange(side.Daemon.Addr, streamRequest(q37), time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			own, err := rawExchange(side.Daemon.Addr, streamRequest(fmt.Sprintf(
				"key=%s&hostname=hs-15&machine_guid=%sxyz&update_every=1&ver=17088", key, parentIdentity.MachineGUID)),
				time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			parts = append(parts, prompt[:n:n], second, own)
			got[i] = bytes.Join(parts, []byte("|"))
		}
		t.Logf("oracle: %q", got[0])
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
		}
	})
	// the receiver's stream.conf rules and the host it creates, with their records (M7 10f, D122.11)
	t.Run("conf", func(t *testing.T) { hsRun(t, p, hsConfRows) })
	t.Run("host", func(t *testing.T) { hsRun(t, p, hsHostRows) })
	t.Run("host-api", func(t *testing.T) { hsHostAPI(t, p) })
}

// hsGUID is the n-th GUID of the handshake rows, as TestStreamHandshake's own.
func hsGUID(n int) string { return fmt.Sprintf("5a1e0000-0000-4000-8000-%012d", 100+n) }

// The handshake rows' keys: one typed machine and not enabled (the type is checked first), one with no type, one
// allowed only from 127.0.0.2, one whose `update every` is ignored (only a GUID's section is read), and one that is
// no UUID.
const (
	hsKeyMachine = "5a1e0000-0000-4000-8000-00000000a0e1"
	hsKeyNoType  = "5a1e0000-0000-4000-8000-00000000a0e2"
	hsKeyAllow   = "5a1e0000-0000-4000-8000-00000000a0e3"
	hsKeyUE      = "5a1e0000-0000-4000-8000-00000000a0e4"
	hsBadKey     = "parity-bad-key"
	hsBadGUID    = "parity-not-a-guid"
	hsProxyKey   = "5a1e0000-0000-4000-8000-00000000a0e5"
	hsPrompt     = "Hit me baby, push them over with the version="
)

// hsConfSections are the parent's stream.conf sections the rows need: the keys above, a GUID typed api, a GUID
// allowed from anywhere but `localhost` (C names a 127.0.0.1 or ::1 client so, socket.c:667-669), the `update every`
// GUIDs (a duration, minutes, a negative one, 0, an invalid one), and the host-members GUIDs (alloc with a retention
// under the minimum; a proxy that is enabled but never starts: no data comes).
var hsConfSections = fmt.Sprintf(`
[%[1]s]
    type = machine
[%[2]s]
    enabled = yes
    db = ram
[%[3]s]
    enabled = yes
    type = api
    db = ram
    allow from = 127.0.0.2
[%[4]s]
    enabled = yes
    type = api
    db = ram
    update every = 9
[%[5]s]
    enabled = yes
    type = api
    db = ram
[%[6]s]
    type = api
[%[7]s]
    allow from = !localhost *
[%[8]s]
    update every = 5
[%[9]s]
    update every = 1m
[%[10]s]
    update every = -7
[%[11]s]
    update every = 0
[%[12]s]
    update every = abc
[%[13]s]
    db = alloc
    retention = 3
[%[14]s]
    proxy enabled = yes
    proxy destination = 127.0.0.1:9
    proxy api key = %[15]s
[%[16]s]
    allow from = 127.0.0 0.0.2
`, hsKeyMachine, hsKeyNoType, hsKeyAllow, hsKeyUE, hsBadKey, hsGUID(25), hsGUID(28), hsGUID(40), hsGUID(41),
	hsGUID(42), hsGUID(43), hsGUID(44), hsGUID(50), hsGUID(51), hsProxyKey, hsGUID(35))

// hsFlexiGUID is hs-32's GUID as uuid_parse_flexi() also reads it: upper case, no dashes.
var hsFlexiGUID = strings.ToUpper(strings.ReplaceAll(hsGUID(32), "-", ""))

// hsRow is one STREAM request of the conf and host rows.
type hsRow struct {
	name   string
	from   string // the client's address; empty is 127.0.0.1 (C's `localhost`)
	query  string
	agent  string // the User-Agent; empty is parity-child/1.0
	host   string // the row's hostnames and GUID, whose records are compared
	host2  string
	guid   string
	accept bool   // the oracle must answer with its prompt (else the denial)
	want   string // a text the oracle's records of the row must hold
	wantNo string // a text they must not hold
	caps   uint32 // when set, the oracle's answer must carry exactly these compression bits
	pause  bool   // wait for the previous connection of the same GUID to be gone first
}

func hsQuery(key, host, guid, extra string) string {
	return fmt.Sprintf("key=%s&hostname=%s&machine_guid=%s&update_every=1&ver=%d%s", key, host, guid, stream.CapsLive, extra)
}

var hsConfRows = []hsRow{
	// strm.conf.rcv.type
	{name: "key-type-machine", query: hsQuery(hsKeyMachine, "hs-21", hsGUID(21), ""), host: "hs-21",
		want: "API key provided is a machine UUID (did you mix them up?)"},
	{name: "key-no-type", query: hsQuery(hsKeyNoType, "hs-22", hsGUID(22), ""), host: "hs-22", accept: true,
		want: "Host 'hs-22' (at registry as 'hs-22')"},
	// a lookup creates the option it reads (inicfg_get()): a name first seen as a key is typed api from then on
	{name: "key-then-guid-1", query: hsQuery(hsGUID(23), "hs-23", hsGUID(24), ""), host: "hs-23",
		want: "API key is not enabled in stream.conf"},
	{name: "key-then-guid-2", query: hsQuery(parentIdentity.StreamKey, "hs-24", hsGUID(23), ""), host: "hs-24",
		want: "machine UUID is an API key (did you mix them up?)"},
	{name: "guid-type-api", query: hsQuery(parentIdentity.StreamKey, "hs-25", hsGUID(25), ""), host: "hs-25",
		want: "machine UUID is an API key (did you mix them up?)"},
	// strm.conf.rcv.allow_from
	{name: "key-allow-localhost", query: hsQuery(hsKeyAllow, "hs-26", hsGUID(26), ""), host: "hs-26",
		want: "API key is not allowed from this IP"},
	{name: "key-allow-127.0.0.2", from: "127.0.0.2", query: hsQuery(hsKeyAllow, "hs-27", hsGUID(27), ""), host: "hs-27",
		accept: true, want: "Host 'hs-27'"},
	{name: "guid-allow-localhost", query: hsQuery(parentIdentity.StreamKey, "hs-28", hsGUID(28), ""), host: "hs-28",
		want: "machine UUID is not allowed from this IP"},
	{name: "guid-allow-127.0.0.2", from: "127.0.0.2", query: hsQuery(parentIdentity.StreamKey, "hs-28b", hsGUID(28), ""),
		host: "hs-28b", accept: true, want: "Host 'hs-28b'"},
	// exact words: `127.0.0` and `0.0.2` would admit 127.0.0.2 as a prefix or a suffix
	{name: "guid-allow-exact", from: "127.0.0.2", query: hsQuery(parentIdentity.StreamKey, "hs-35", hsGUID(35), ""),
		host: "hs-35", want: "machine UUID is not allowed from this IP"},
	// strm.hs.val.uuid: no UUID check refuses anything
	{name: "bad-key", query: hsQuery("parity-other-key", "hs-29", hsGUID(29), ""), host: "hs-29",
		want: "API key is not enabled in stream.conf"},
	{name: "bad-key-section", query: hsQuery(hsBadKey, "hs-30", hsGUID(30), ""), host: "hs-30", accept: true,
		want: "Host 'hs-30'"},
	{name: "bad-guid", query: hsQuery(parentIdentity.StreamKey, "hs-31", hsBadGUID, ""), host: "hs-31", guid: hsBadGUID,
		accept: true, want: "Host machine GUID " + hsBadGUID + " is not valid"},
	{name: "flexi-guid", query: hsQuery(parentIdentity.StreamKey, "hs-32", hsFlexiGUID, ""), host: "hs-32",
		guid: hsFlexiGUID, accept: true, want: "Host 'hs-32'", wantNo: "is not valid"},
	// strm.hs.req.unknown_params: the hostname first, so the records name it
	{name: "unknown-params", query: fmt.Sprintf("hostname=hs-33&key=%s&machine_guid=%s&update_every=1&ver=%d"+
		"&foo=bar&tags=t1&program_name=pn&NETDATA_SYSTEM_CPU_VENDOR=v&NETDATA_SYSTEM_CPU_DETECTION=d"+
		"&NETDATA_SYSTEM_RAM_DETECTION=r&NETDATA_SYSTEM_DISK_DETECTION=s&NETDATA_CONTAINER_IS_OFFICIAL_IMAGE=1"+
		"&NETDATA_PROTOCOL_VERSION=1.1&key=%s&ver=1&hostname=hs-33-again", parentIdentity.StreamKey, hsGUID(33),
		stream.CapsLive, hsKeyNoType), host: "hs-33", accept: true,
		want: "request has parameter 'foo' = 'bar', which is not used.", wantNo: "NETDATA_SYSTEM_CPU_VENDOR"},
	// strm.conf.stream.enable_compression: the key has none, [stream] has none: yes, and the parent picks zstd
	{name: "compression", query: fmt.Sprintf("key=%s&hostname=hs-34&machine_guid=%s&update_every=1&ver=%d",
		parentIdentity.StreamKey, hsGUID(34), stream.CapsLive|stream.CapsCompression), host: "hs-34", accept: true,
		caps: stream.CapZSTD, want: "Host 'hs-34'"},
}

var hsHostRows = []hsRow{
	// strm.conf.rcv.update_every: only a GUID's section, a duration, its absolute value, 0 for 1, an invalid one
	{name: "ue-5", query: hsQuery(parentIdentity.StreamKey, "hs-40", hsGUID(40), ""), host: "hs-40", accept: true,
		want: "update every 5,"},
	{name: "ue-1m", query: hsQuery(parentIdentity.StreamKey, "hs-41", hsGUID(41), ""), host: "hs-41", accept: true,
		want: "update every 60,"},
	{name: "ue-negative", query: hsQuery(parentIdentity.StreamKey, "hs-42", hsGUID(42), ""), host: "hs-42",
		accept: true, want: "update every 7,"},
	{name: "ue-0", query: hsQuery(parentIdentity.StreamKey, "hs-43", hsGUID(43), ""), host: "hs-43", accept: true,
		want: "update every 1,"},
	{name: "ue-0-again", query: hsQuery(parentIdentity.StreamKey, "hs-43", hsGUID(43), ""), host: "hs-43", accept: true,
		pause: true, want: "has an update frequency of 1 seconds, but the wanted one is 0 seconds"},
	{name: "ue-invalid", query: hsQuery(parentIdentity.StreamKey, "hs-44", hsGUID(44), ""), host: "hs-44",
		guid: hsGUID(44), accept: true, want: "is configured with an invalid duration"},
	{name: "ue-key-ignored", query: hsQuery(hsKeyUE, "hs-45", hsGUID(45), ""), host: "hs-45", accept: true,
		want: "update every 1,"},
	// strm.rcv.host_create: every member the request carries, then a reconnect under another name and program
	{name: "members", agent: "hm-agent/9.8.7", query: hsMembersQuery("hs-50", "6.1.0"), host: "hs-50", accept: true,
		want: "Host 'hs-50' (at registry as 'hs-50-reg')"},
	// a reconnect replaces the system info with the request's (another kernel version here)
	{name: "members-again", agent: "other-agent/1.0", pause: true, query: hsMembersQuery("hs-50b", "6.2.0"), host: "hs-50",
		host2: "hs-50b", accept: true, want: "Host 'hs-50b' switched program version from '9.8.7' to '1.0'"},
	{name: "proxy", query: hsQuery(parentIdentity.StreamKey, "hs-51", hsGUID(51), ""), host: "hs-51", accept: true,
		want: "streaming enabled (to '127.0.0.1:P'"},
}

// hsMembersQuery is the host-members request under `host`: every member a handshake feeds and system info of each
// kind (one legacy NETDATA_SYSTEM_OS_* name).
func hsMembersQuery(host, kernel string) string {
	return fmt.Sprintf("key=%s&hostname=%s&registry_hostname=hs-50-reg&machine_guid=%s&update_every=2&os=parityos"+
		"&timezone=Europe/Athens&abbrev_timezone=EEST&utc_offset=10800&hops=1&ml_capable=1&ml_enabled=0&mc_version=4"+
		"&ver=%d&NETDATA_HOST_OS_NAME=ParityOS&NETDATA_HOST_OS_ID=parity&NETDATA_HOST_OS_ID_LIKE=debian"+
		"&NETDATA_HOST_OS_VERSION=1.2&NETDATA_HOST_OS_VERSION_ID=1&NETDATA_HOST_OS_DETECTION=t"+
		"&NETDATA_SYSTEM_OS_NAME=Legacy&NETDATA_SYSTEM_KERNEL_NAME=Linux&NETDATA_SYSTEM_KERNEL_VERSION="+kernel+
		"&NETDATA_SYSTEM_ARCHITECTURE=x86_64&NETDATA_SYSTEM_VIRTUALIZATION=kvm&NETDATA_SYSTEM_VIRT_DETECTION=t"+
		"&NETDATA_SYSTEM_CONTAINER=none&NETDATA_SYSTEM_CONTAINER_DETECTION=t&NETDATA_SYSTEM_CPU_LOGICAL_CPU_COUNT=4"+
		"&NETDATA_SYSTEM_CPU_FREQ=2400000000&NETDATA_SYSTEM_CPU_MODEL=ParityCPU&NETDATA_SYSTEM_TOTAL_RAM=8589934592"+
		"&NETDATA_SYSTEM_TOTAL_DISK_SIZE=1073741824&NETDATA_INSTANCE_CLOUD_TYPE=none&NETDATA_HOST_IS_K8S_NODE=false"+
		"&NETDATA_CONTAINER_OS_NAME=none&NETDATA_SYSTEM_DEFAULT_INTERFACE_NAME=eth9",
		parentIdentity.StreamKey, host, hsGUID(50), stream.CapsLive)
}

// hsMsgRe is a record's message.
var hsMsgRe = regexp.MustCompile(`msg="((?:[^"\\]|\\.)*)"`)

var (
	// the stream API key's echoes, which the Rust agent masks (D31, D34)
	hsRepeatedKeyRe = regexp.MustCompile(`'key' = '[^']*'`)
	hsHostKeyRe     = regexp.MustCompile(`with api key '[^']*'`)
	hsVersionRe     = regexp.MustCompile(`version=(\d+)`)
)

// hsRecords are a parent's records of one row: its handshake's (`STREAM RCV '<host>'`), its host's (`Host '<host>'`)
// and those naming its GUID's section or an invalid GUID, normalized as parentRecords does, the API key's echoes
// masked; the web thread's errno of rrdhost_create() and of the invalid duration is left out (D36).
func hsRecords(t *testing.T, d *daemon.Daemon, r hsRow) []string {
	t.Helper()
	var out []string
	for _, l := range parentRecords(t, d, "msg=", nil) {
		named := false
		for _, h := range []string{r.host, r.host2} {
			if h != "" && (strings.Contains(l, "STREAM RCV '"+h+"'") || strings.Contains(l, "Host '"+h+"'")) {
				named = true
			}
		}
		if r.guid != "" && (strings.Contains(l, "GUID "+r.guid+" is not valid") || strings.Contains(l, "["+r.guid+"].")) {
			named = true
		}
		if !named {
			continue
		}
		l = hsRepeatedKeyRe.ReplaceAllString(l, "'key' = 'K'")
		l = hsHostKeyRe.ReplaceAllString(l, "with api key 'K'")
		if strings.Contains(l, " is not valid") || strings.Contains(l, "invalid duration") {
			l = errnoRe.ReplaceAllString(l, "")
		}
		out = append(out, l)
	}
	return out
}

// hsRun sends each row to both parents, compares the answers and then each row's records; the oracle must answer and
// log as the row says, so no row passes on two empty sides.
func hsRun(t *testing.T, p *Pair, rows []hsRow) {
	for _, r := range rows {
		if r.pause {
			// the previous connection of the GUID ended with rawExchange's silence: let both parents drop it
			time.Sleep(3 * time.Second)
		}
		agent := r.agent
		if agent == "" {
			agent = "parity-child/1.0"
		}
		req := []byte("STREAM " + r.query + " HTTP/1.1\r\nUser-Agent: " + agent + "\r\nAccept: */*\r\n\r\n")
		var got [2][]byte
		for i, side := range p.Each() {
			b, err := rawExchangeFrom(r.from, side.Daemon.Addr, req, time.Second)
			if err != nil {
				t.Fatalf("%s: %s: %v", r.name, side.Role, err)
			}
			got[i] = b
		}
		if accepted := bytes.HasPrefix(got[0], []byte(hsPrompt)); accepted != r.accept {
			t.Errorf("%s: the oracle answered %q", r.name, got[0])
		}
		if r.caps != 0 {
			m := hsVersionRe.FindSubmatch(got[0])
			if m == nil {
				t.Errorf("%s: the oracle's answer %q has no version", r.name, got[0])
				continue
			}
			v, _ := strconv.ParseUint(string(m[1]), 10, 32)
			if uint32(v)&stream.CapsCompression != r.caps {
				t.Errorf("%s: the oracle's answer %q has compression bits %d", r.name, got[0], uint32(v)&stream.CapsCompression)
			}
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("%s: responses differ\noracle:    %q\ncandidate: %q", r.name, got[0], got[1])
		}
	}
	time.Sleep(2 * time.Second)
	for _, r := range rows {
		var recs [2][]string
		for i, side := range p.Each() {
			recs[i] = hsRecords(t, side.Daemon, r)
		}
		all := strings.Join(recs[0], "\n")
		var msgs []string
		for _, l := range recs[0] {
			if m := hsMsgRe.FindStringSubmatch(l); m != nil {
				msgs = append(msgs, m[1])
			}
		}
		if !strings.Contains(all, r.want) {
			t.Errorf("%s: the oracle's records lack %q:\n%s", r.name, r.want, all)
		}
		if r.wantNo != "" && strings.Contains(strings.Join(msgs, "\n"), r.wantNo) {
			t.Errorf("%s: the oracle's records hold %q:\n%s", r.name, r.wantNo, all)
		}
		diffLines(t, r.name+" records", recs[0], recs[1])
		t.Logf("%s:\n%s", r.name, all)
	}
}

// hsHostAPI compares what each parent serves about the host-members row's host (renamed by its reconnect): the host
// members of /host/<h>/api/v1/charts and the system info of /host/<h>/api/v1/info.
func hsHostAPI(t *testing.T, p *Pair) {
	rules := Rules{Masks: []Mask{
		{Pattern: "rrd_memory_bytes", Reason: "each implementation's own structures (D24)"},
		{Pattern: "hosts", Reason: "every host of the parent, in its list order"},
	}}
	for _, h := range []string{"hs-50b", "hs-40", "hs-41", "hs-42", "hs-43"} {
		diffs, err := p.CompareJSON("/host/"+h+"/api/v1/charts", nil, rules)
		if err != nil {
			t.Fatal(err)
		}
		for _, d := range diffs {
			t.Errorf("/host/%s/api/v1/charts: %s", h, d)
		}
	}
	if b, err := rawExchange(p.Oracle.Addr, []byte("GET /host/hs-40/api/v1/charts HTTP/1.1\r\n\r\n"), 5*time.Second); err != nil ||
		!bytes.Contains(b, []byte(`"update_every":5,`)) {
		t.Errorf("the oracle's /host/hs-40/api/v1/charts lacks update_every 5: %v %.300s", err, b)
	}
	var text [2]string
	for i, side := range p.Each() {
		text[i], _ = infoIdentity(t, side.Daemon.Addr, "/host/hs-50b/api/v1/info")
	}
	if !strings.Contains(text[0], `"kernel_version":"6.2.0"`) {
		t.Errorf("the oracle's /host/hs-50b/api/v1/info keeps the first connection's kernel version:\n%s", text[0])
	}
	if !strings.Contains(text[0], `"os_name":"Legacy",`) || !strings.Contains(text[0], `"os_id":"parity",`) {
		t.Errorf("the oracle's /host/hs-50b/api/v1/info lacks the request's OS name (the legacy name, the later):\n%s", text[0])
	}
	if text[0] != text[1] {
		t.Errorf("/host/hs-50b/api/v1/info differs\n%s", firstDifference([]byte(text[0]), []byte(text[1])))
	}
}

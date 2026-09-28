// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// accessNames are `http_access_name[]`, in bit order.
var accessNames = []string{"signed-in", "same-space", "commercial", "anonymous-data", "sensitive-data",
	"view-config", "edit-config", "view-notifications-config", "edit-notifications-config", "view-alerts-silencing",
	"edit-alerts-silencing"}

// bearerToken is a token file as `bearer_token_save_to_file()` writes one.
type bearerToken struct {
	token, account string
	name           string
	access         []string
	role           string
	created        int64
	expires        int64
}

func uuidBytes(t *testing.T, s string) [16]byte {
	t.Helper()
	var u [16]byte
	b, err := hex.DecodeString(strings.ReplaceAll(s, "-", ""))
	if err != nil || len(b) != 16 {
		t.Fatalf("uuid %q: %v", s, err)
	}
	copy(u[:], b)
	return u
}

// signature is `bearer_token_signature()`: XXH3-64 over the 136-byte struct, with its access bits or, as the
// production build hashes it, zeros in their place (D96.4).
func (b bearerToken) signature(t *testing.T, host string, withAccess bool) uint64 {
	t.Helper()
	var buf [136]byte
	h, tok, acc := uuidBytes(t, host), uuidBytes(t, b.token), uuidBytes(t, b.account)
	copy(buf[0:], h[:])
	copy(buf[16:], tok[:])
	copy(buf[32:], acc[:])
	copy(buf[48:111], b.name)
	if withAccess {
		var bits uint16
		for _, n := range b.access {
			bits |= 1 << slices.Index(accessNames, n)
		}
		binary.LittleEndian.PutUint16(buf[112:], bits)
	}
	roles := []string{"none", "admin", "manager", "troubleshooter", "observer", "member", "billing", "any"}
	buf[114] = byte(slices.Index(roles, b.role))
	binary.LittleEndian.PutUint64(buf[120:], uint64(b.created))
	binary.LittleEndian.PutUint64(buf[128:], uint64(b.expires))
	return xxh3_64(buf[:])
}

// file is the token's JSON, signed as given.
func (b bearerToken) file(host string, signature uint64) []byte {
	names := make([]string, len(b.access))
	for i, n := range b.access {
		names[i] = strconv.Quote(n)
	}
	return fmt.Appendf(nil, `{"version":1,"host_uuid":"%s","token":"%s","cloud_account_id":"%s","client_name":%s,`+
		`"access":[%s],"user_role":"%s","created_s":%d,"expires_s":%d,"signature":%d}`, host, b.token, b.account,
		strconv.Quote(b.name), strings.Join(names, ","), b.role, b.created, b.expires, signature)
}

// withBearer is a GET of path carrying the token in `Authorization`.
func withBearer(path, token string) []byte {
	return []byte("GET " + path + " HTTP/1.1\r\nAuthorization: Bearer " + token + "\r\n\r\n")
}

// bearerRun starts a pair with `files` (name → content) in each side's `lib/bearer_tokens/` and `webExtra` in [web].
func bearerRun(t *testing.T, roles [2]Role, webExtra string, files map[string][]byte) *Pair {
	t.Helper()
	prepare := func(t *testing.T, runDir string) {
		dir := filepath.Join(runDir, "lib", "bearer_tokens")
		if err := os.MkdirAll(dir, 0o750); err != nil {
			t.Fatal(err)
		}
		for name, content := range files {
			if err := os.WriteFile(filepath.Join(dir, name), content, 0o640); err != nil {
				t.Fatal(err)
			}
		}
	}
	opts := daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1, WebExtra: webExtra,
		LogsExtra: "    level = debug\n"}
	return startPairWith(t, opts, parentIdentity, binaries(t), [2]string{}, roles, prepare)
}

// compareBearerRequests sends each raw request to both sides and compares the masked answers.
func compareBearerRequests(t *testing.T, p *Pair, requests map[string][]byte) {
	t.Helper()
	for name, req := range requests {
		var got [2][]byte
		for i, side := range p.Each() {
			r, err := rawExchange(side.Daemon.Addr, req, 10*time.Second)
			if err != nil {
				t.Fatalf("%s: %s: %v", side.Role, name, err)
			}
			if bytes.HasPrefix(req, []byte("GET /api/v1/info")) && bytes.Contains(r, []byte(" 200 OK")) {
				// served: the body is `api.v1-info`'s
				r, _, _ = bytes.Cut(r, []byte("\r\n"))
			}
			got[i] = maskTimings(maskRaw(r))
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("%s: responses differ\n%s", name, firstDifference(got[0], got[1]))
		}
	}
}

// bearerFileRecordRe finds the records of loading a token file, which follow `readdir` order: a directory's own.
var bearerFileRecordRe = regexp.MustCompile(`level=\w+ (tid=N  )msg="(?:Failed to parse bearer token file|` +
	`Cannot parse bearer token file|HTTP user role)[^"]*"`)

// bearerLogMasks collapse the file records for the in-order comparison (compareBearerRecords compares them as a set)
// and hide the response sizes of the bodies other checks own.
var bearerLogMasks = []logMask{{responseBytesRe, "${1}N"}, {bearerFileRecordRe, `level=L ${1}msg="token file record"`}}

// compareBearerRecords compares both sides' token file records, sorted.
func compareBearerRecords(t *testing.T, p *Pair) {
	t.Helper()
	var records [2][]string
	for i, side := range p.Each() {
		for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
			if n := normalizeLog(l, side.Daemon.Opts.RunDir, ""); bearerFileRecordRe.MatchString(n) {
				records[i] = append(records[i], n)
			}
		}
		slices.Sort(records[i])
	}
	if !slices.Equal(records[0], records[1]) {
		t.Errorf("token file records:\noracle:\n%s\ncandidate:\n%s", strings.Join(records[0], "\n"),
			strings.Join(records[1], "\n"))
	}
}

// compareBearerFiles compares the token files left on both sides.
func compareBearerFiles(t *testing.T, p *Pair) {
	t.Helper()
	var left [2][]string
	for i, side := range p.Each() {
		entries, _ := os.ReadDir(filepath.Join(side.Daemon.Opts.RunDir, "lib", "bearer_tokens"))
		for _, e := range entries {
			left[i] = append(left[i], e.Name())
		}
	}
	if !slices.Equal(left[0], left[1]) {
		t.Errorf("token files left: oracle %v, candidate %v", left[0], left[1])
	}
	t.Logf("token files left: %v", left[0])
}

// TestWebBearer (check `web.bearer`, milestone 6 commit 3, D96): the production build's signature.
//   - f1-probe (D96.4): three admin tokens with every access, signed with their access bits (A), with zeros in their
//     place (B), and with no access at all (C, where both forms agree). Compared: the files left, `/api/v3/me` with
//     each token, and the records about token files.
func TestWebBearer(t *testing.T) {
	host := parentIdentity.MachineGUID
	t.Run("f1-probe", func(t *testing.T) {
		base := bearerToken{account: "b6b6b6b6-4444-4444-8444-00000000acc1", name: "f1-probe", access: accessNames,
			role: "admin", created: 1700000000, expires: 4102444800}
		a, b, c := base, base, base
		a.token, b.token, c.token = "b6b6b6b6-4444-4444-8444-0000000000a1", "b6b6b6b6-4444-4444-8444-0000000000b1",
			"b6b6b6b6-4444-4444-8444-0000000000c1"
		c.access = nil
		files := map[string][]byte{
			a.token: a.file(host, a.signature(t, host, true)),
			b.token: b.file(host, b.signature(t, host, false)),
			c.token: c.file(host, c.signature(t, host, true)),
		}
		p := bearerRun(t, [2]Role{"probe-oracle", "probe-candidate"}, "", files)
		compareBearerFiles(t, p)
		for _, token := range []string{a.token, b.token, c.token} {
			var got [2][]byte
			for i, side := range p.Each() {
				r, err := rawExchange(side.Daemon.Addr, withBearer("/api/v3/me", token), 10*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = maskTimings(maskRaw(r))
			}
			_, body, _ := bytes.Cut(got[0], []byte("\r\n\r\n"))
			t.Logf("me with %s: %s", token, body)
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("me with %s: responses differ\n%s", token, firstDifference(got[0], got[1]))
			}
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		var records [2][]string
		for i, side := range p.Each() {
			for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
				if strings.Contains(l, "bearer token") {
					records[i] = append(records[i], normalizeLog(l, side.Daemon.Opts.RunDir, ""))
				}
			}
			slices.Sort(records[i])
		}
		t.Logf("oracle records:\n%s", strings.Join(records[0], "\n"))
		if !slices.Equal(records[0], records[1]) {
			t.Errorf("token records:\noracle:\n%s\ncandidate:\n%s", strings.Join(records[0], "\n"),
				strings.Join(records[1], "\n"))
		}
	})
}

// TestWebBearerMatrix (check `web.bearer`): bearer token files an agent finds at start, and the tokens requests
// present.
//   - files: a valid token among files C deletes (expired, another host, a bad signature, a token not the file's
//     name, bad dates, no object) or keeps (no JSON, `null`), one with an unknown role, one with an unknown access
//     name, a compact and a non-UUID name; then header forms (`X-Netdata-Auth`, a lowercase `bearer`, `null`, a
//     non-UUID, an unknown token, an invalid token before a valid one and after it) and a file written after the
//     start (loaded on demand). Compared: the files left, the answers, both whole logs.
//   - protected: `[web] bearer token protection` on: `/api/v1/info` without a token (412), with an admin's (200) and
//     with a signed-in token lacking anonymous data (403); `/api/v3/me` with each.
func TestWebBearerMatrix(t *testing.T) {
	host := parentIdentity.MachineGUID
	valid := bearerToken{token: "b6b6b6b6-5555-4555-8555-000000000001", account: "b6b6b6b6-5555-4555-8555-00000000acc1",
		name: "matrix", access: accessNames, role: "admin", created: 1700000000, expires: 4102444800}
	signed := func(b bearerToken) []byte { return b.file(host, b.signature(t, host, false)) }
	variant := func(token string, edit func(*bearerToken)) bearerToken {
		b := valid
		b.token = token
		edit(&b)
		return b
	}
	expired := variant("b6b6b6b6-5555-4555-8555-000000000002", func(b *bearerToken) { b.created, b.expires = 1000, 2000 })
	role := variant("b6b6b6b6-5555-4555-8555-000000000003", func(b *bearerToken) { b.role = "chief" })
	unknownAccess := variant("b6b6b6b6-5555-4555-8555-000000000004", func(b *bearerToken) {
		b.access = []string{"signed-in", "bogus", "anonymous-data"}
	})
	signedIn := variant("b6b6b6b6-5555-4555-8555-000000000005", func(b *bearerToken) { b.access = []string{"signed-in"} })
	later := variant("b6b6b6b6-5555-4555-8555-000000000006", func(b *bearerToken) { b.role = "observer" })
	files := map[string][]byte{
		valid.token:         signed(valid),
		expired.token:       signed(expired),
		signedIn.token:      signed(signedIn),
		unknownAccess.token: unknownAccess.file(host, unknownAccess.signature(t, host, false)),
		"b6b6b6b6-5555-4555-8555-000000000011": variant("b6b6b6b6-5555-4555-8555-000000000011", func(*bearerToken) {}).
			file("b6b6b6b6-5555-4555-8555-0000000000ff", 0),
		"b6b6b6b6-5555-4555-8555-000000000012": variant("b6b6b6b6-5555-4555-8555-000000000012", func(*bearerToken) {}).
			file(host, 12345),
		"b6b6b6b6-5555-4555-8555-000000000013": signed(valid),
		"b6b6b6b6-5555-4555-8555-000000000014": signed(variant("b6b6b6b6-5555-4555-8555-000000000014",
			func(b *bearerToken) { b.created = b.expires })),
		"b6b6b6b6-5555-4555-8555-000000000015": []byte("123"),
		"b6b6b6b6-5555-4555-8555-000000000016": []byte("not json"),
		"b6b6b6b6-5555-4555-8555-000000000017": []byte("null"),
		"b6b6b6b655554555855500000000001a": signed(variant("b6b6b6b6-5555-4555-8555-00000000001a",
			func(*bearerToken) {})),
		"not-a-token": []byte("{}"),
	}
	// the role's file is signed with the role the agent reads (none) after its warning
	r := role
	r.role = "none"
	files[role.token] = role.file(host, r.signature(t, host, false))
	get := func(path string, headers ...string) []byte {
		return []byte("GET " + path + " HTTP/1.1\r\n" + strings.Join(headers, "\r\n") + "\r\n\r\n")
	}
	bearer := func(token string) string { return "Authorization: Bearer " + token }
	t.Run("files", func(t *testing.T) {
		p := bearerRun(t, [2]Role{"files-oracle", "files-candidate"}, "", files)
		compareBearerFiles(t, p)
		compareBearerRequests(t, p, map[string][]byte{
			"valid":          get("/api/v3/me", bearer(valid.token)),
			"x-netdata-auth": get("/api/v3/me", "X-Netdata-Auth: Bearer "+valid.token),
			"lowercase":      get("/api/v3/me", "authorization: bearer   "+valid.token),
			"compact":        get("/api/v3/me", bearer(strings.ReplaceAll(valid.token, "-", ""))),
			"expired":        get("/api/v3/me", bearer(expired.token)),
			"role":           get("/api/v3/me", bearer(role.token)),
			"unknown-access": get("/api/v3/me", bearer(unknownAccess.token)),
			"null":           get("/api/v3/me", bearer("null")),
			"undefined":      get("/api/v3/me", bearer("undefined")),
			"not-a-uuid":     get("/api/v3/me", bearer("zzz")),
			"unknown":        get("/api/v3/me", bearer("b6b6b6b6-5555-4555-8555-0000000000ee")),
			"invalid-then":   get("/api/v3/me", bearer("zzz"), "X-Netdata-Auth: Bearer "+valid.token),
			"then-invalid":   get("/api/v3/me", bearer(valid.token), "X-Netdata-Auth: Bearer zzz"),
		})
		for _, side := range p.Each() {
			dir := filepath.Join(side.Daemon.Opts.RunDir, "lib", "bearer_tokens")
			if err := os.WriteFile(filepath.Join(dir, later.token), signed(later), 0o640); err != nil {
				t.Fatal(err)
			}
		}
		compareBearerRequests(t, p, map[string][]byte{"on-demand": get("/api/v3/me", bearer(later.token))})
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareBearerRecords(t, p)
		compareLogFilesWith(t, p, bearerLogMasks)
	})
	t.Run("protected", func(t *testing.T) {
		p := bearerRun(t, [2]Role{"protected-oracle", "protected-candidate"}, "    bearer token protection = yes\n",
			files)
		compareBearerRequests(t, p, map[string][]byte{
			"info-anonymous": get("/api/v1/info"),
			"info-admin":     get("/api/v1/info", bearer(valid.token)),
			"info-signed-in": get("/api/v1/info", bearer(signedIn.token)),
			"me-anonymous":   get("/api/v3/me"),
			"me-signed-in":   get("/api/v3/me", bearer(signedIn.token)),
		})
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareBearerRecords(t, p)
		compareLogFilesWith(t, p, bearerLogMasks)
	})
}

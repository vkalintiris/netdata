// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
)

// testDashNormWeb pins the static answers' mask (maskAnswer, as walkFile uses it) on the head C sent for `/index.html`
// in helper s6's probe pass (2026-10-06, the request in second 1791318731): the random transaction id is masked; the
// expiry is masked only when it is now + 86400 for a second the request was in flight; the Date (the file's
// modification time) is kept, and so is the body.
func testDashNormWeb(t *testing.T) {
	const (
		expires = "Expires: Wed, 07 Oct 2026 20:32:11 GMT"
		tx      = "X-Transaction-ID: 51a353ef61e747d7ad489d303992457e"
		head    = "HTTP/1.1 200 OK\r\nConnection: close\r\n" +
			"Server: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\n" +
			"Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\n" +
			"Date: Wed, 23 Sep 2026 09:19:11 GMT\r\nContent-Type: text/html; charset=utf-8\r\n" +
			"Cache-Control: public\r\n" + expires + "\r\nContent-Length: 102375\r\n" + tx + "\r\n\r\n<!doctype html>"
		sent = 1791318731
	)
	noTx := strings.Replace(head, tx, "X-Transaction-ID: <masked>", 1)
	without := func(s string) string { return strings.Replace(s, expires+"\r\n", "", 1) }
	other := func(s string) string { return strings.Replace(s, expires, "Expires: x", 1) }
	for name, c := range map[string]struct {
		in       string
		from, to int64
		want     string
	}{
		"in-flight":   {head, sent, sent, strings.Replace(noTx, expires, "Expires: now+86400", 1)},
		"in-window":   {head, sent - 1, sent + 1, strings.Replace(noTx, expires, "Expires: now+86400", 1)},
		"before-it":   {head, sent + 1, sent + 2, noTx},
		"after-it":    {head, sent - 2, sent - 1, noTx},
		"no-expiry":   {without(head), sent, sent, without(noTx)},
		"not-a-clock": {other(head), sent, sent, other(noTx)},
	} {
		t.Run(name, func(t *testing.T) {
			if got := string(maskAnswer([]byte(c.in), [2]int64{c.from, c.to})); got != c.want {
				t.Errorf("maskAnswer:\n got %q\nwant %q", got, c.want)
			}
		})
	}
}

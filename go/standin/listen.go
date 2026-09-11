// The one place in the stand-in that opens anything.
//
// Everything that can reach outside this process is in this file and nowhere
// else, which is the same shape Catalyst's own adapter boundary takes: a
// reader who wants to know what this program can touch reads one short file
// rather than grepping the package. It is also the only file that needs the
// net package at all -- the service itself is written against net/http and
// never names a socket.
//
// The rule it enforces is the rule of section 8, and it is the whole reason
// this file is worth separating: a stand-in reachable from outside the
// machine is a service nobody meant to run, so anything that is not a
// Unix-domain socket or a loopback address is refused before a port is bound.
package main

import (
	"fmt"
	"net"
	"os"
	"strings"
)

// listen opens the address, refusing anything that is not a Unix-domain
// socket or loopback.
func listen(address string) (net.Listener, error) {
	if path, ok := strings.CutPrefix(address, "unix:"); ok {
		if path == "" {
			return nil, fmt.Errorf("unix: needs a socket path")
		}
		// A socket left behind by an earlier run would make this one fail to
		// start; it belongs to no other program.
		_ = os.Remove(path)
		return net.Listen("unix", path)
	}
	host := address
	if index := strings.LastIndex(address, ":"); index >= 0 {
		host = address[:index]
	}
	host = strings.Trim(host, "[]")
	if host != "" && host != "localhost" && host != "::1" && !strings.HasPrefix(host, "127.") {
		return nil, fmt.Errorf("refusing to listen on %q: a stand-in listens on a unix socket or on loopback only", host)
	}
	return net.Listen("tcp", address)
}

// Command catalyst_standin is a stand-in service for a failure rehearsal.
//
// It is deliberately not a real order service. It accepts orders, remembers
// them in memory, and holds a small fixed number of slots so that too much
// traffic at once produces overload rather than an unbounded queue -- which is
// the failure a rehearsal is there to reproduce.
//
// Every response body says "stand_in": true. A measurement taken against this
// program is a measurement of this program.
//
// It listens on a Unix-domain socket or on loopback, and refuses anywhere
// else: a stand-in reachable from outside the machine is a service nobody
// meant to run.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

type order struct {
	ID   string `json:"id"`
	Item string `json:"item"`
	Qty  int    `json:"qty"`
	Paid bool   `json:"paid"`
}

type standIn struct {
	mu       sync.Mutex
	orders   map[string]order
	next     int
	inFlight int
	queue    int
	work     time.Duration
}

// acquire takes one of the queue's slots. It reports false when more than
// queue requests are already in flight, which is the overload the rehearsal
// measures.
func (s *standIn) acquire() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.inFlight >= s.queue {
		return false
	}
	s.inFlight++
	return true
}

func (s *standIn) release() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.inFlight--
}

func write(w http.ResponseWriter, status int, body map[string]interface{}) {
	body["stand_in"] = true
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	encoded, err := json.Marshal(body)
	if err != nil {
		return
	}
	_, _ = w.Write(encoded)
	_, _ = w.Write([]byte("\n"))
}

func overloaded(w http.ResponseWriter) {
	write(w, http.StatusServiceUnavailable, map[string]interface{}{
		"error": "overloaded",
	})
}

func (s *standIn) health(w http.ResponseWriter, r *http.Request) {
	if !s.acquire() {
		overloaded(w)
		return
	}
	defer s.release()
	time.Sleep(s.work)
	write(w, http.StatusOK, map[string]interface{}{
		"service": "catalyst-standin",
	})
}

func (s *standIn) create(w http.ResponseWriter, r *http.Request) {
	var request struct {
		Item string `json:"item"`
		Qty  int    `json:"qty"`
	}
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		write(w, http.StatusBadRequest, map[string]interface{}{
			"error": "malformed request body",
		})
		return
	}
	if !s.acquire() {
		overloaded(w)
		return
	}
	defer s.release()
	time.Sleep(s.work)
	s.mu.Lock()
	s.next++
	id := fmt.Sprintf("o%d", s.next)
	s.orders[id] = order{ID: id, Item: request.Item, Qty: request.Qty}
	s.mu.Unlock()
	write(w, http.StatusCreated, map[string]interface{}{
		"id":  id,
		"qty": request.Qty,
	})
}

func (s *standIn) show(w http.ResponseWriter, id string) {
	if !s.acquire() {
		overloaded(w)
		return
	}
	defer s.release()
	time.Sleep(s.work)
	s.mu.Lock()
	found, ok := s.orders[id]
	s.mu.Unlock()
	if !ok {
		write(w, http.StatusNotFound, map[string]interface{}{
			"error": "no such order",
		})
		return
	}
	write(w, http.StatusOK, map[string]interface{}{
		"id":   found.ID,
		"qty":  found.Qty,
		"paid": found.Paid,
	})
}

func (s *standIn) pay(w http.ResponseWriter, id string) {
	if !s.acquire() {
		overloaded(w)
		return
	}
	defer s.release()
	time.Sleep(s.work)
	s.mu.Lock()
	found, ok := s.orders[id]
	if ok {
		found.Paid = true
		s.orders[id] = found
	}
	s.mu.Unlock()
	if !ok {
		write(w, http.StatusNotFound, map[string]interface{}{
			"error": "no such order",
		})
		return
	}
	write(w, http.StatusOK, map[string]interface{}{
		"id":   found.ID,
		"paid": true,
	})
}

// routeOrders routes /orders, /orders/<id> and /orders/<id>/pay by hand: the
// standard multiplexer does not pattern-match path segments in Go 1.21.
func (s *standIn) routeOrders(w http.ResponseWriter, r *http.Request) {
	rest := strings.TrimPrefix(r.URL.Path, "/orders")
	rest = strings.Trim(rest, "/")
	if rest == "" {
		if r.Method != http.MethodPost {
			write(w, http.StatusMethodNotAllowed, map[string]interface{}{
				"error": "use POST /orders",
			})
			return
		}
		s.create(w, r)
		return
	}
	parts := strings.Split(rest, "/")
	id := parts[0]
	if len(parts) == 1 && r.Method == http.MethodGet {
		s.show(w, id)
		return
	}
	if len(parts) == 2 && parts[1] == "pay" && r.Method == http.MethodPost {
		s.pay(w, id)
		return
	}
	write(w, http.StatusNotFound, map[string]interface{}{
		"error": "no such route",
	})
}

func main() {
	address := flag.String("listen", "127.0.0.1:8080", "unix:PATH or 127.0.0.1:PORT")
	queue := flag.Int("queue", 4, "how many requests may be in flight at once")
	workMS := flag.Int("work-ms", 20, "how long each request holds its slot, in milliseconds")
	flag.Parse()

	if *queue < 1 {
		fmt.Fprintln(os.Stderr, "-queue must be at least 1")
		os.Exit(2)
	}
	if *workMS < 0 {
		fmt.Fprintln(os.Stderr, "-work-ms must not be negative")
		os.Exit(2)
	}

	listener, err := listen(*address)
	if err != nil {
		fmt.Fprintf(os.Stderr, "stand-in: %v\n", err)
		os.Exit(2)
	}
	defer func() { _ = listener.Close() }()

	service := &standIn{
		orders: map[string]order{},
		queue:  *queue,
		work:   time.Duration(*workMS) * time.Millisecond,
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/health", service.health)
	mux.HandleFunc("/orders", service.routeOrders)
	mux.HandleFunc("/orders/", service.routeOrders)

	fmt.Fprintf(os.Stderr,
		"catalyst stand-in service (a stand-in, not a real service) listening on %s, queue %d, work %d ms\n",
		*address, *queue, *workMS)

	server := &http.Server{
		Handler:           mux,
		ReadHeaderTimeout: 10 * time.Second,
	}
	if err := server.Serve(listener); err != nil {
		fmt.Fprintf(os.Stderr, "stand-in: %v\n", err)
		os.Exit(1)
	}
}

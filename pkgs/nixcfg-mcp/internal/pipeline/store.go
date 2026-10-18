package pipeline

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"nixcfg-mcp/internal/config"
)

// Store persists waits under $XDG_STATE_HOME/nixcfg/waits:
//
//	<id>.json          spec, timeline and bookkeeping (rewritten atomically)
//	<id>.events.ndjson append-only event log
//	<id>.lock          flock held by whichever process is running the wait
//
// Whoever holds the lock polls; anyone else (a CLI attaching to a wait the MCP
// server started, or a restarted server) follows the files, and takes over if
// the lock is free.
type Store struct {
	Dir string

	mu    sync.Mutex
	waits map[string]*Wait
	locks map[string]*os.File
	cfg   *config.Config
	obs   *Observer
}

// NewStore opens the store.
func NewStore(cfg *config.Config, obs *Observer) (*Store, error) {
	dir := config.StateDir("waits")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	return &Store{Dir: dir, waits: map[string]*Wait{}, locks: map[string]*os.File{}, cfg: cfg, obs: obs}, nil
}

type record struct {
	Result Result `json:"result"`
	Book   book   `json:"book"`
}

func (s *Store) path(id, ext string) string { return filepath.Join(s.Dir, id+ext) }

func (s *Store) save(r *Result, b book) {
	raw, err := json.MarshalIndent(record{Result: *r, Book: b}, "", "  ")
	if err != nil {
		return
	}
	tmp := s.path(r.ID, ".json.tmp")
	if err := os.WriteFile(tmp, raw, 0o600); err == nil {
		_ = os.Rename(tmp, s.path(r.ID, ".json"))
	}
}

func (s *Store) appendEvent(id string, e Event) {
	f, err := os.OpenFile(s.path(id, ".events.ndjson"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return
	}
	defer f.Close()
	raw, _ := json.Marshal(e)
	_, _ = f.Write(append(raw, '\n'))
}

func (s *Store) load(id string) (*record, []Event, error) {
	if strings.ContainsAny(id, "/\\") || !strings.HasPrefix(id, "w-") {
		return nil, nil, fmt.Errorf("invalid wait id %q", id)
	}
	raw, err := os.ReadFile(s.path(id, ".json"))
	if err != nil {
		return nil, nil, fmt.Errorf("unknown wait %q", id)
	}
	var rec record
	if err := json.Unmarshal(raw, &rec); err != nil {
		return nil, nil, err
	}
	var evs []Event
	if f, err := os.Open(s.path(id, ".events.ndjson")); err == nil {
		defer f.Close()
		sc := bufio.NewScanner(f)
		sc.Buffer(make([]byte, 64*1024), 1<<20)
		for sc.Scan() {
			var e Event
			if json.Unmarshal(sc.Bytes(), &e) == nil {
				evs = append(evs, e)
			}
		}
	}
	return &rec, evs, nil
}

// tryLock takes the wait's lock without blocking.
func (s *Store) tryLock(id string) bool {
	f, err := os.OpenFile(s.path(id, ".lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return false
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		f.Close()
		return false
	}
	s.locks[id] = f
	return true
}

func (s *Store) unlock(id string) {
	if f, ok := s.locks[id]; ok {
		_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
		f.Close()
		delete(s.locks, id)
	}
}

// Start creates a wait and runs it in the background of this process.
func (s *Store) Start(spec Spec) (*Wait, error) {
	w, err := NewWait(s.cfg, s.obs, s, spec)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	if !s.tryLock(w.ID()) {
		s.mu.Unlock()
		return nil, fmt.Errorf("could not lock new wait %s", w.ID())
	}
	s.waits[w.ID()] = w
	s.mu.Unlock()
	w.persist()
	s.run(w)
	return w, nil
}

func (s *Store) run(w *Wait) {
	go func() {
		w.Run(context.Background())
		s.mu.Lock()
		s.unlock(w.ID())
		s.mu.Unlock()
	}()
}

// Handle is a wait this process can report on: either running here, or
// followed from the files another process is writing.
type Handle struct {
	ID    string
	local *Wait
	store *Store
}

// Get finds a wait: running here, resumable from disk (and resumed), or
// followed read-only while another process runs it.
func (s *Store) Get(id string) (*Handle, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if w, ok := s.waits[id]; ok {
		return &Handle{ID: id, local: w, store: s}, nil
	}
	rec, evs, err := s.load(id)
	if err != nil {
		return nil, err
	}
	if !rec.Result.State.Terminal() && s.tryLock(id) {
		// Nobody is running it: resume here.
		w := &Wait{
			res: rec.Result, book: rec.Book, events: evs, store: s, obs: s.obs, cfg: s.cfg,
			notify: make(chan struct{}), done: make(chan struct{}), lastDetail: map[string]string{},
		}
		w.addEvent(Event{At: time.Now(), Stage: w.res.CurrentStage, Kind: "info", Text: "resumed by pid " + fmt.Sprint(os.Getpid())})
		s.waits[id] = w
		s.run(w)
		return &Handle{ID: id, local: w, store: s}, nil
	}
	return &Handle{ID: id, store: s}, nil
}

// Snapshot returns the current result and events from cursor.
func (h *Handle) Snapshot(cursor int) (Result, []Event, error) {
	if h.local != nil {
		r, e := h.local.Snapshot(cursor)
		return r, e, nil
	}
	rec, evs, err := h.store.load(h.ID)
	if err != nil {
		return Result{}, nil, err
	}
	if cursor < 0 || cursor > len(evs) {
		cursor = 0
	}
	return rec.Result, evs[cursor:], nil
}

// Await blocks until the wait is terminal, ctx ends or block elapses, calling
// onEvent for each new event. It returns the result and the next cursor.
func (h *Handle) Await(ctx context.Context, cursor int, block time.Duration, onEvent func(Event)) (Result, int, error) {
	timer := time.NewTimer(block)
	defer timer.Stop()
	for {
		var changed <-chan struct{}
		if h.local != nil {
			changed = h.local.Changed()
		}
		r, evs, err := h.Snapshot(cursor)
		if err != nil {
			return r, cursor, err
		}
		for _, e := range evs {
			if onEvent != nil {
				onEvent(e)
			}
		}
		cursor += len(evs)
		if r.State.Terminal() {
			return r, cursor, nil
		}
		if changed == nil {
			c := make(chan struct{})
			go func() { time.Sleep(2 * time.Second); close(c) }()
			changed = c
		}
		select {
		case <-ctx.Done():
			return r, cursor, nil
		case <-timer.C:
			return r, cursor, nil
		case <-changed:
		}
		// A follower may find the runner gone: take over.
		if h.local == nil {
			if nh, err := h.store.Get(h.ID); err == nil && nh.local != nil {
				h.local = nh.local
			}
		}
	}
}

// Cancel stops a wait running in this process.
func (h *Handle) Cancel() error {
	if h.local == nil {
		return fmt.Errorf("wait %s is being run by another process", h.ID)
	}
	h.local.Cancel()
	return nil
}

// List returns recent waits, newest first.
func (s *Store) List(limit int) []Result {
	entries, _ := os.ReadDir(s.Dir)
	var out []Result
	for _, e := range entries {
		name := e.Name()
		if !strings.HasSuffix(name, ".json") || strings.HasSuffix(name, ".events.ndjson") {
			continue
		}
		rec, _, err := s.load(strings.TrimSuffix(name, ".json"))
		if err == nil {
			out = append(out, rec.Result)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Created.After(out[j].Created) })
	if limit > 0 && len(out) > limit {
		out = out[:limit]
	}
	return out
}

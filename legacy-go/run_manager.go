package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

type RunManager struct {
	cfg    Config
	logger *log.Logger
	pools  []*tokenPool
	next   atomic.Uint64

	stopCh chan struct{}
	wg     sync.WaitGroup
}

type tokenPool struct {
	name   string
	token  string
	cfg    Config
	client *UpstreamClient
	logger *log.Logger

	mu               sync.Mutex
	runs             map[string]*managedRun // agentID -> current run
	rootRun          *managedRun            // 唯一根 run，所有 run 的直接祖先
	draining         []*managedRun
	session          *cachedSession
	sessionRefreshCh chan struct{}
	lastError        string
	cooldownUntil    time.Time
}

type managedRun struct {
	id           string
	agentID      string
	startedAt    time.Time
	inflight     int
	requestCount int
	finishing    bool
}

type runLease struct {
	pool *tokenPool
	run  *managedRun
}

type tokenSnapshot struct {
	Name              string        `json:"name"`
	Runs              []runSnapshot `json:"runs"`
	DrainingRuns      int           `json:"draining_runs"`
	SessionStatus     string        `json:"session_status,omitempty"`
	SessionInstanceID string        `json:"session_instance_id,omitempty"`
	SessionExpiresAt  time.Time     `json:"session_expires_at,omitempty"`
	SessionPosition   int           `json:"session_position,omitempty"`
	SessionQueueDepth int           `json:"session_queue_depth,omitempty"`
	SessionPollAt     time.Time     `json:"session_poll_at,omitempty"`
	CooldownUntil     time.Time     `json:"cooldown_until,omitempty"`
	LastError         string        `json:"last_error,omitempty"`
}

type runSnapshot struct {
	AgentID      string    `json:"agent_id"`
	RunID        string    `json:"run_id"`
	StartedAt    time.Time `json:"started_at"`
	Inflight     int       `json:"inflight"`
	RequestCount int       `json:"request_count"`
}

type waitingRoomError struct {
	Token      string
	Position   int
	QueueDepth int
	RetryAfter time.Duration
}

func (e *waitingRoomError) Error() string {
	if e == nil {
		return "freebuff waiting room queued"
	}

	message := "freebuff waiting room queued"
	if e.Token != "" {
		message += " for " + e.Token
	}
	if e.Position > 0 {
		if e.QueueDepth >= e.Position {
			message += fmt.Sprintf(" (position %d/%d)", e.Position, e.QueueDepth)
		} else {
			message += fmt.Sprintf(" (position %d)", e.Position)
		}
	}
	if e.RetryAfter > 0 {
		message += fmt.Sprintf(", retry in about %s", e.RetryAfter.Round(time.Second))
	}
	return message
}

func NewRunManager(cfg Config, client *UpstreamClient, logger *log.Logger) *RunManager {
	pools := make([]*tokenPool, 0, len(cfg.AuthTokens))
	for index, token := range cfg.AuthTokens {
		pools = append(pools, &tokenPool{
			name:   fmt.Sprintf("token-%d", index+1),
			token:  token,
			cfg:    cfg,
			client: client,
			runs:   make(map[string]*managedRun),
			logger: logger,
		})
	}

	return &RunManager{
		cfg:    cfg,
		logger: logger,
		pools:  pools,
		stopCh: make(chan struct{}),
	}
}

func (m *RunManager) Start(ctx context.Context, agentIDs []string) {
	// 后台只预热根 run；子 run 随请求惰性创建。
	go m.prewarm()

	m.wg.Add(1)
	go func() {
		defer m.wg.Done()
		ticker := time.NewTicker(time.Minute)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				maintainCtx, cancel := context.WithTimeout(context.Background(), m.cfg.RequestTimeout)
				for _, pool := range m.pools {
					if err := pool.maintain(maintainCtx); err != nil {
						m.logger.Printf("%s: maintenance failed: %v", pool.name, err)
					}
				}
				cancel()
			case <-m.stopCh:
				return
			}
		}
	}()
}

func (m *RunManager) prewarm() {
	ctx, cancel := context.WithTimeout(context.Background(), m.cfg.RequestTimeout)
	defer cancel()

	for _, pool := range m.pools {
		if _, err := pool.ensureSession(ctx); err != nil {
			m.logger.Printf("%s: free session prewarm failed: %v", pool.name, err)
		}
		// 只预热根 run；子 run 按请求惰性创建，祖先指向根。
		if err := pool.rotateAgent(ctx, rootAgentID); err != nil {
			m.logger.Printf("%s: prewarm root %s failed: %v", pool.name, rootAgentID, err)
		} else {
			m.logger.Printf("%s: prewarmed root %s", pool.name, rootAgentID)
		}
	}
}

func (m *RunManager) Close(ctx context.Context) {
	close(m.stopCh)
	m.wg.Wait()
	for _, pool := range m.pools {
		if err := pool.shutdown(ctx); err != nil {
			m.logger.Printf("%s: shutdown failed: %v", pool.name, err)
		}
	}
}

func (m *RunManager) Acquire(ctx context.Context, agentID string) (*runLease, error) {
	if len(m.pools) == 0 {
		return nil, errors.New("no auth tokens configured")
	}

	startIndex := int(m.next.Add(1)-1) % len(m.pools)
	var errs []string
	var waiting []*waitingRoomError
	for offset := 0; offset < len(m.pools); offset++ {
		pool := m.pools[(startIndex+offset)%len(m.pools)]
		lease, err := pool.acquire(ctx, agentID)
		if err == nil {
			return lease, nil
		}
		var waitingErr *waitingRoomError
		if errors.As(err, &waitingErr) {
			waiting = append(waiting, waitingErr)
		}
		errs = append(errs, fmt.Sprintf("%s: %v", pool.name, err))
	}

	if len(waiting) == len(m.pools) && len(waiting) > 0 {
		best := waiting[0]
		for _, candidate := range waiting[1:] {
			if candidate != nil && (best == nil || (candidate.Position > 0 && candidate.Position < best.Position)) {
				best = candidate
			}
		}
		if best != nil {
			return nil, best
		}
	}

	return nil, fmt.Errorf("unable to acquire run from any token (%s)", strings.Join(errs, "; "))
}

func (m *RunManager) Release(lease *runLease) {
	if lease == nil || lease.pool == nil || lease.run == nil {
		return
	}
	lease.pool.release(lease.run)
}

func (m *RunManager) Invalidate(lease *runLease, reason string) {
	if lease == nil || lease.pool == nil || lease.run == nil {
		return
	}
	lease.pool.invalidate(lease.run, reason)
}

func (m *RunManager) Cooldown(lease *runLease, duration time.Duration, reason string) {
	if lease == nil || lease.pool == nil {
		return
	}
	lease.pool.markCooldown(duration, reason)
}

func (m *RunManager) Snapshots() []tokenSnapshot {
	snapshots := make([]tokenSnapshot, 0, len(m.pools))
	for _, pool := range m.pools {
		snapshots = append(snapshots, pool.snapshot())
	}
	return snapshots
}

func (p *tokenPool) acquire(ctx context.Context, agentID string) (*runLease, error) {
	if _, err := p.ensureSession(ctx); err != nil {
		return nil, err
	}

	p.mu.Lock()
	if now := time.Now(); now.Before(p.cooldownUntil) {
		cooldownUntil := p.cooldownUntil
		p.mu.Unlock()
		return nil, fmt.Errorf("token cooling down until %s", cooldownUntil.Format(time.RFC3339))
	}
	root := p.rootRun
	rootNeedsRotate := root == nil || time.Since(root.startedAt) >= p.cfg.RotationInterval
	run := p.runs[agentID]
	needsRotate := run == nil || time.Since(run.startedAt) >= p.cfg.RotationInterval
	p.mu.Unlock()

	// 先保活根 run（会话根），再轮换目标 agent。
	if rootNeedsRotate {
		if err := p.rotateAgent(ctx, rootAgentID); err != nil {
			return nil, fmt.Errorf("rotate root agent: %w", err)
		}
	}
	if agentID != rootAgentID && needsRotate {
		if err := p.rotateAgent(ctx, agentID); err != nil {
			return nil, err
		}
	}

	p.mu.Lock()
	defer p.mu.Unlock()
	if agentID == rootAgentID {
		run = p.rootRun
	} else {
		run = p.runs[agentID]
	}
	if run == nil {
		return nil, errors.New("run missing after rotation")
	}
	run.inflight++
	run.requestCount++
	return &runLease{pool: p, run: run}, nil
}

func (p *tokenPool) maintain(ctx context.Context) error {
	if _, err := p.ensureSession(ctx); err != nil {
		p.logger.Printf("%s: refresh free session failed: %v", p.name, err)
	}

	p.mu.Lock()
	var toRotate []string
	for agentID, run := range p.runs {
		if time.Since(run.startedAt) >= p.cfg.RotationInterval {
			toRotate = append(toRotate, agentID)
		}
	}
	rootExpired := p.rootRun == nil || time.Since(p.rootRun.startedAt) >= p.cfg.RotationInterval
	draining := append([]*managedRun(nil), p.draining...)
	p.mu.Unlock()

	// 根 run 过期时优先轮换根，子 run 的祖先才能指向新根。
	if rootExpired {
		if err := p.rotateAgent(ctx, rootAgentID); err != nil {
			p.logger.Printf("%s: rotate root agent failed: %v", p.name, err)
		}
	}
	for _, agentID := range toRotate {
		if err := p.rotateAgent(ctx, agentID); err != nil {
			p.logger.Printf("%s: rotate agent %s failed: %v", p.name, agentID, err)
		}
	}

	for _, run := range draining {
		if err := p.finishIfReady(run); err != nil {
			p.logger.Printf("%s: finish draining run %s failed: %v", p.name, run.id, err)
		}
	}
	return nil
}

func (p *tokenPool) shutdown(ctx context.Context) error {
	p.mu.Lock()
	var allRuns []*managedRun
	for _, run := range p.runs {
		allRuns = append(allRuns, run)
	}
	if p.rootRun != nil {
		allRuns = append(allRuns, p.rootRun)
		p.rootRun = nil
	}
	allRuns = append(allRuns, p.draining...)
	p.runs = make(map[string]*managedRun)
	p.draining = nil
	p.mu.Unlock()

	var errs []string
	for _, run := range allRuns {
		if err := p.client.FinishRun(ctx, p.token, run.id, run.requestCount); err != nil {
			errs = append(errs, err.Error())
		}
	}
	if err := p.endSession(ctx); err != nil {
		errs = append(errs, err.Error())
	}
	if len(errs) > 0 {
		return errors.New(strings.Join(errs, "; "))
	}
	return nil
}

func (p *tokenPool) rotateAgent(ctx context.Context, agentID string) error {
	p.mu.Lock()
	if now := time.Now(); now.Before(p.cooldownUntil) {
		cooldownUntil := p.cooldownUntil
		p.mu.Unlock()
		return fmt.Errorf("token cooling down until %s", cooldownUntil.Format(time.RFC3339))
	}
	// 非根 run 的祖先只能是根 run；根 run 传空数组（上游不接受 null）。
	ancestors := []string{}
	if agentID != rootAgentID && p.rootRun != nil && p.rootRun.id != "" {
		ancestors = []string{p.rootRun.id}
	}
	p.mu.Unlock()

	runID, err := p.client.StartRun(ctx, p.token, agentID, ancestors...)
	if err != nil {
		p.mu.Lock()
		p.lastError = err.Error()
		p.mu.Unlock()
		return err
	}

	p.mu.Lock()
	newRun := &managedRun{
		id:        runID,
		agentID:   agentID,
		startedAt: time.Now(),
	}
	var oldRun *managedRun
	if agentID == rootAgentID {
		oldRun = p.rootRun
		p.rootRun = newRun
	} else {
		oldRun = p.runs[agentID]
		p.runs[agentID] = newRun
	}
	p.lastError = ""
	if oldRun != nil {
		p.draining = append(p.draining, oldRun)
	}
	p.mu.Unlock()

	if oldRun != nil {
		go func(run *managedRun) {
			if err := p.finishIfReady(run); err != nil {
				p.logger.Printf("%s: finish rotated run %s (agent %s) failed: %v", p.name, run.id, run.agentID, err)
			}
		}(oldRun)
	}
	return nil
}

func (p *tokenPool) release(run *managedRun) {
	if run == nil {
		return
	}

	p.mu.Lock()
	if run.inflight > 0 {
		run.inflight--
	}
	p.mu.Unlock()

	if err := p.finishIfReady(run); err != nil {
		p.logger.Printf("%s: finish released run %s failed: %v", p.name, run.id, err)
	}
}

func (p *tokenPool) finishIfReady(run *managedRun) error {
	p.mu.Lock()
	if run == nil || run.inflight > 0 || run.finishing {
		p.mu.Unlock()
		return nil
	}
	// 当前仍在服役的 run（子 run 在 runs 表、根 run 在 rootRun）不能提前结束。
	if run.agentID == rootAgentID {
		if p.rootRun == run {
			p.mu.Unlock()
			return nil
		}
	} else if current, ok := p.runs[run.agentID]; ok && current == run {
		p.mu.Unlock()
		return nil
	}
	run.finishing = true
	p.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), p.cfg.RequestTimeout)
	defer cancel()

	if err := p.client.FinishRun(ctx, p.token, run.id, run.requestCount); err != nil {
		p.mu.Lock()
		run.finishing = false
		p.lastError = err.Error()
		p.mu.Unlock()
		return err
	}

	p.mu.Lock()
	filtered := p.draining[:0]
	for _, drainingRun := range p.draining {
		if drainingRun != run {
			filtered = append(filtered, drainingRun)
		}
	}
	p.draining = filtered
	p.mu.Unlock()
	return nil
}

func (p *tokenPool) invalidate(run *managedRun, reason string) {
	p.mu.Lock()
	defer p.mu.Unlock()

	// Remove from current runs if it matches
	if run.agentID == rootAgentID {
		if p.rootRun == run {
			p.rootRun = nil
		}
	} else if current, ok := p.runs[run.agentID]; ok && current == run {
		delete(p.runs, run.agentID)
	}

	filtered := p.draining[:0]
	for _, drainingRun := range p.draining {
		if drainingRun != run {
			filtered = append(filtered, drainingRun)
		}
	}
	p.draining = filtered
	if reason != "" {
		p.lastError = reason
	}
}

func (p *tokenPool) markCooldown(duration time.Duration, reason string) {
	if duration <= 0 {
		return
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	p.cooldownUntil = time.Now().Add(duration)
	if reason != "" {
		p.lastError = reason
	}
}

func (p *tokenPool) snapshot() tokenSnapshot {
	p.mu.Lock()
	defer p.mu.Unlock()

	snapshot := tokenSnapshot{
		Name:          p.name,
		DrainingRuns:  len(p.draining),
		CooldownUntil: p.cooldownUntil,
		LastError:     p.lastError,
	}
	if p.session != nil {
		snapshot.SessionStatus = string(p.session.status)
		snapshot.SessionInstanceID = p.session.instanceID
		snapshot.SessionExpiresAt = p.session.expiresAt
		snapshot.SessionPosition = p.session.position
		snapshot.SessionQueueDepth = p.session.queueDepth
		snapshot.SessionPollAt = p.session.pollAt
	}
	if p.rootRun != nil {
		snapshot.Runs = append(snapshot.Runs, runSnapshot{
			AgentID:      rootAgentID,
			RunID:        p.rootRun.id,
			StartedAt:    p.rootRun.startedAt,
			Inflight:     p.rootRun.inflight,
			RequestCount: p.rootRun.requestCount,
		})
	}
	for agentID, run := range p.runs {
		snapshot.Runs = append(snapshot.Runs, runSnapshot{
			AgentID:      agentID,
			RunID:        run.id,
			StartedAt:    run.startedAt,
			Inflight:     run.inflight,
			RequestCount: run.requestCount,
		})
	}
	return snapshot
}

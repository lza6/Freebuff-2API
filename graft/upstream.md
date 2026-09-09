# upstream.go

- UpstreamClient · struct · L16-L20 — UpstreamClient
- NewUpstreamClient · function · L22-L38 — func NewUpstreamClient(cfg Config) *UpstreamClient
- StartRun · method · L40-L76 — func (c *UpstreamClient) StartRun(ctx context.Context, authToken, agentID string, ancestorRunIds ...string) (string, error)
- FinishRun · method · L78-L106 — func (c *UpstreamClient) FinishRun(ctx context.Context, authToken, runID string, totalSteps int) error
- ChatCompletions · method · L108-L124 — func (c *UpstreamClient) ChatCompletions(ctx context.Context, authToken string, body []byte) (*http.Response, []byte, error)
- doJSON · method · L126-L146 — func (c *UpstreamClient) doJSON(ctx context.Context, authToken, path string, body []byte) (*http.Response, error)
- retryAfterDuration · function · L148-L157 — func retryAfterDuration(headerValue string) time.Duration

# models.go

- ModelRegistry · struct · L38-L50 — ModelRegistry
- NewModelRegistry · function · L52-L60 — func NewModelRegistry(client *http.Client, logger *log.Logger) *ModelRegistry
- Start · method · L62-L86 — func (r *ModelRegistry) Start(ctx context.Context)
- Stop · method · L88-L91 — func (r *ModelRegistry) Stop()
- Models · method · L94-L100 — func (r *ModelRegistry) Models() []string
- HasModel · method · L103-L108 — func (r *ModelRegistry) HasModel(model string) bool
- AgentForModel · method · L111-L116 — func (r *ModelRegistry) AgentForModel(model string) (string, bool)
- AgentIDs · method · L119-L127 — func (r *ModelRegistry) AgentIDs() []string
- refresh · method · L129-L178 — func (r *ModelRegistry) refresh(ctx context.Context) error
- loadFallback · method · L180-L190 — func (r *ModelRegistry) loadFallback()
- parseAllFreeModels · function · L193-L215 — func parseAllFreeModels(source string) map[string][]string
- buildModelMapping · function · L219-L235 — func buildModelMapping(agentModels map[string][]string) (map[string]string, []string)

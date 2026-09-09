# config.go

- Config · struct · L15-L24 — Config
- rawConfig · struct · L26-L34 — rawConfig
- loadConfig · function · L36-L85 — func loadConfig(configPath string) (Config, error)
- normalizeUpstreamBaseURL · function · L87-L103 — func normalizeUpstreamBaseURL(raw string) string
- loadRawConfig · function · L105-L128 — func loadRawConfig(configPath string) (rawConfig, error)
- overrideString · function · L130-L134 — func overrideString(target *string, envName string)
- overrideCSV · function · L136-L142 — func overrideCSV(target *[]string, envName string)
- splitList · function · L144-L149 — func splitList(value string) []string
- compactStrings · function · L151-L161 — func compactStrings(values []string) []string
- dedupeStrings · function · L163-L174 — func dedupeStrings(values []string) []string
- containsString · function · L176-L183 — func containsString(values []string, needle string) bool
- generateUserAgent · function · L185-L187 — func generateUserAgent() string
- generateClientSessionId · function · L192-L203 — func generateClientSessionId() string

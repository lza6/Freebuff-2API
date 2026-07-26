-- Migration 0002: 增强审计和统计功能
ALTER TABLE usage_events ADD COLUMN ip TEXT DEFAULT '';
ALTER TABLE usage_events ADD COLUMN path TEXT DEFAULT '';
ALTER TABLE usage_events ADD COLUMN user_agent TEXT DEFAULT '';
ALTER TABLE usage_events ADD COLUMN duration_ms INTEGER DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN error_message TEXT DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_events(model);
CREATE INDEX IF NOT EXISTS idx_usage_status ON usage_events(status);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_events(action);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_events(created_at);
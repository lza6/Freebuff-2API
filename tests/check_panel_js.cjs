#!/usr/bin/env node
/**
 * 面板内嵌 JS 语法检查（不启动服务）：从 src/web.rs 抽出 <script> 块，交给 node 解析。
 * 用法：node tests/check_panel_js.cjs
 */
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const src = fs.readFileSync(path.join(__dirname, '..', 'src', 'web.rs'), 'utf8');
const m = src.match(/<script>([\s\S]*?)<\/script>/);
if (!m) { console.error('❌ 未在 src/web.rs 中找到 <script> 块'); process.exit(1); }
const js = m[1];
const tmp = path.join(os.tmpdir(), `freebuff-panel-${Date.now()}.js`);
fs.writeFileSync(tmp, js);
try {
  execFileSync(process.execPath, ['--check', tmp], { stdio: 'pipe' });
  console.log(`✅ 面板 JS 语法检查通过（${js.length} 字符）`);
  process.exit(0);
} catch (e) {
  console.error('❌ 面板 JS 语法错误：');
  console.error(String(e.stderr || e.message));
  process.exit(1);
} finally {
  try { fs.unlinkSync(tmp); } catch (_) { /* 忽略 */ }
}

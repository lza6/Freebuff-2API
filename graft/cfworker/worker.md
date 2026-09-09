# cfworker/worker.js

- apiError · function · L23-L36 — function apiError(message, status)
- handleCors · function · L38-L50 — function handleCors(req)
- randomId · function · L52-L55 — function randomId(len = 32)
- agentForModel · function · L57-L68 — function agentForModel(requestedModel)
- upstreamRequest · function · L72-L97 — async function upstreamRequest(token, path, body, signal, extraHeaders = {})
- startRun · function · L99-L109 — async function startRun(token, agentId, signal, ancestorRunIds = [])
- finishRun · function · L111-L117 — async function finishRun(token, runId, signal)
- ensureSession · function · L119-L132 — async function ensureSession(token, signal)
- handleChat · function · L136-L226 — async function handleChat(req, token)
- cleanup · function · L187-L194 — cleanup = async ()
- fetch · method · L231-L275 — async fetch(request, env, ctx)
- UserStateDO · class · L278-L278 — class UserStateDO

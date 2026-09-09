function postMutation(fetchImpl, url, launchId, body) {
  return fetchImpl(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(launchId ? { 'x-freebuff-launch-id': launchId } : {}),
    },
    body: JSON.stringify(body),
  })
}

module.exports = { postMutation }

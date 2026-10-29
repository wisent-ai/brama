// brama.wisent.com serves Brama's documentation under /docs and nothing else.
// Brama's HTTP API is not published on the internet: the gateway binds
// loopback on the host Stado places it on, and a client reaches it through
// Stado's service directory (`stado service directory connect brama`), which
// answers each machine with that machine's own route. vercel.json's last
// rewrite sends every path outside /docs here, so a caller configured with
// this site as its gateway is told so by name instead of reading Vercel's 404
// page as Brama refusing its request.
import { STATUS_CODES } from 'node:http';

function statusOf(name) {
  const found = Object.entries(STATUS_CODES).find(([, text]) => text === name);
  if (!found) throw new Error(`Node names no HTTP status "${name}"`);
  const [code] = found;
  return Number(code);
}
const NOT_FOUND = statusOf('Not Found');

const RUNBOOK = '/docs/runbook/startup-and-readiness#public-address';

export default function notPublished(request, response) {
  const asked = request.query?.__brama_path ?? request.url;
  response.writeHead(NOT_FOUND, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify({
    error: {
      message: `brama.wisent.com serves Brama's documentation under /docs only; Brama's HTTP API is not published on the internet, so ${request.method} ${asked} is not served here. Reach the gateway placed for your machine through Stado's service directory: stado service directory connect brama. See ${RUNBOOK}`,
      type: 'request_error',
      code: 'api_not_published',
      retryable: false,
    },
  }));
}

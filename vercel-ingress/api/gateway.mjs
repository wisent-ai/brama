// brama.wisent.com is Brama's public address (agents' boundaries, every
// consumer's BRAMA_URL); this Vercel project serves its docs and forwards
// every other path to the gateway's published origin. The origin is the
// project's BRAMA_GATEWAY_ORIGIN, the public HTTPS origin `stado web origin
// url` prints for Brama's declared public origin; nothing here names a host.
// Without it the forwarder refuses by name instead of Vercel answering 404,
// which every caller read as Brama refusing its request.
import { request as requestHttps } from 'node:https';
import { STATUS_CODES } from 'node:http';

const ORIGIN_VARIABLE = 'BRAMA_GATEWAY_ORIGIN';

function statusOf(name) {
  const found = Object.entries(STATUS_CODES).find(([, text]) => text === name);
  if (!found) throw new Error(`Node names no HTTP status "${name}"`);
  const [code] = found;
  return Number(code);
}
const SERVICE_UNAVAILABLE = statusOf('Service Unavailable');
const BAD_GATEWAY = statusOf('Bad Gateway');

function declaredOrigin() {
  const value = process.env[ORIGIN_VARIABLE]?.trim();
  if (!value) return { refusal: `${ORIGIN_VARIABLE} is not set on the brama.wisent.com project, so no gateway origin is declared to forward to` };
  let origin;
  try { origin = new URL(value); } catch { return { refusal: `${ORIGIN_VARIABLE} is not a URL: ${JSON.stringify(value)}` }; }
  if (origin.protocol !== 'https:' || origin.username || origin.password
      || (origin.pathname !== '/' && origin.pathname !== '') || origin.search || origin.hash) {
    return { refusal: `${ORIGIN_VARIABLE} must be one credential-free HTTPS origin, not ${JSON.stringify(value)}` };
  }
  return { origin };
}

const HOP_HEADERS = new Set([
  'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization',
  'te', 'trailer', 'transfer-encoding', 'upgrade',
]);
function forwardedHeaders(headers) {
  const excluded = new Set(HOP_HEADERS);
  if (typeof headers.connection === 'string') {
    for (const name of headers.connection.split(',')) excluded.add(name.trim().toLowerCase());
  }
  return Object.fromEntries(Object.entries(headers).filter(([name, value]) => value !== undefined && !excluded.has(name.toLowerCase())));
}

function refuse(response, status, body) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify(body));
}

export default async function gateway(request, response) {
  const { origin, refusal } = declaredOrigin();
  if (refusal) {
    refuse(response, SERVICE_UNAVAILABLE, { error: { code: 'gateway_origin_undeclared', message: refusal } });
    return;
  }
  const incoming = new URL(request.url, origin);
  const routedPath = request.query?.__brama_path ?? incoming.searchParams.get('__brama_path');
  if (!routedPath) {
    refuse(response, BAD_GATEWAY, { error: { code: 'ingress_path_missing', message: 'the ingress rewrite passed no __brama_path' } });
    return;
  }
  incoming.searchParams.delete('__brama_path');
  const target = new URL(origin);
  target.pathname = String(routedPath);
  target.search = incoming.search;
  const headers = forwardedHeaders(request.headers);
  headers.host = origin.host;

  await new Promise(resolve => {
    let finished = false;
    const finish = () => { if (!finished) { finished = true; resolve(); } };
    const failed = error => {
      if (finished) return;
      if (!response.destroyed && !response.headersSent) {
        refuse(response, BAD_GATEWAY, {
          error: {
            code: 'gateway_unreachable',
            message: `Brama ingress could not reach the declared gateway ${origin.origin}: ${error.message}`,
            cause_code: error.code,
          },
        });
      } else if (!response.destroyed) response.destroy(error);
      finish();
    };
    const upstream = requestHttps(target, { method: request.method, headers }, received => {
      response.writeHead(received.statusCode, forwardedHeaders(received.headers));
      received.once('error', failed);
      received.once('end', finish);
      received.pipe(response);
    });
    upstream.once('error', failed);
    request.once('aborted', () => upstream.destroy(new Error('Brama caller disconnected')));
    response.once('close', () => {
      if (!response.writableFinished) upstream.destroy(new Error('Brama caller disconnected'));
      finish();
    });
    request.pipe(upstream);
  });
}

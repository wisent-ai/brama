// The documentation site serves /docs; vercel.json's last rewrite sends every
// other path here. Whoever deploys the site chooses whether it also publishes
// a Brama gateway: setting BRAMA_GATEWAY_ORIGIN on the deployment to one
// credential-free HTTPS origin publishes the gateway there, and this function
// forwards each call to it unchanged. Publication is off by default. A
// deployment that sets no origin publishes no API, and every such path is
// refused by name (404 api_not_published) instead of the host's own 404 page,
// which callers read as Brama refusing their request. Nothing here names a
// host: the origin is the deployment's own declaration.
import { request as requestHttps } from 'node:https';
import { STATUS_CODES } from 'node:http';

const ORIGIN_VARIABLE = 'BRAMA_GATEWAY_ORIGIN';
const RUNBOOK = '/docs/runbook/startup-and-readiness#public-address';

function statusOf(name) {
  const found = Object.entries(STATUS_CODES).find(([, text]) => text === name);
  if (!found) throw new Error(`Node names no HTTP status "${name}"`);
  const [code] = found;
  return Number(code);
}
const NOT_FOUND = statusOf('Not Found');
const INTERNAL_SERVER_ERROR = statusOf('Internal Server Error');
const BAD_GATEWAY = statusOf('Bad Gateway');

/** The deployment's declared gateway origin, or why there is none to forward to. */
function declaredOrigin() {
  const value = process.env[ORIGIN_VARIABLE]?.trim();
  if (!value) return { undeclared: true };
  let origin;
  try { origin = new URL(value); } catch (error) { return { invalid: `${ORIGIN_VARIABLE} is not a URL: ${JSON.stringify(value)}: ${error.message}` }; }
  if (origin.protocol !== 'https:' || origin.username || origin.password
      || (origin.pathname !== '/' && origin.pathname !== '') || origin.search || origin.hash) {
    return { invalid: `${ORIGIN_VARIABLE} must be one credential-free HTTPS origin, not ${JSON.stringify(value)}` };
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

function refuse(response, status, error) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify({ error }));
}

function askedPath(request) {
  return request.query?.__brama_path ?? new URL(request.url, 'http://ingress.invalid').searchParams.get('__brama_path');
}

export default async function gateway(request, response) {
  const { origin, undeclared, invalid } = declaredOrigin();
  if (undeclared) {
    const site = request.headers['x-forwarded-host'] ?? request.headers.host;
    refuse(response, NOT_FOUND, {
      message: `${site} publishes no Brama gateway: its deployment sets no ${ORIGIN_VARIABLE}, so ${request.method} ${askedPath(request) ?? request.url} is not served here. Call the gateway your deployment runs, at the address it declares (with Stado: stado service directory connect brama). A deployment publishes its gateway on this site by setting ${ORIGIN_VARIABLE} to that gateway's HTTPS origin. See ${RUNBOOK}`,
      type: 'request_error',
      code: 'api_not_published',
      retryable: false,
    });
    return;
  }
  if (invalid) {
    refuse(response, INTERNAL_SERVER_ERROR, {
      message: `${invalid}; set it to the gateway's HTTPS origin, or remove it to publish no gateway. See ${RUNBOOK}`,
      type: 'server_error',
      code: 'gateway_origin_invalid',
      retryable: false,
    });
    return;
  }
  const routedPath = askedPath(request);
  if (!routedPath) {
    refuse(response, BAD_GATEWAY, { message: 'the ingress rewrite passed no __brama_path', type: 'server_error', code: 'ingress_path_missing', retryable: false });
    return;
  }
  const incoming = new URL(request.url, origin);
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
          message: `the documentation site could not reach the declared gateway ${origin.origin}: ${error.message}`,
          type: 'server_error',
          code: 'gateway_unreachable',
          cause_code: error.code,
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

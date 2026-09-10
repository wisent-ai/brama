import { readFileSync } from 'node:fs';
import { Agent, request as requestHttps } from 'node:https';
import { isIP } from 'node:net';

const settings = JSON.parse(readFileSync(new URL('../gateway.config.json', import.meta.url), 'utf8'));
const origin = new URL(settings.origin);
if (origin.protocol !== 'https:' || origin.username || origin.password
    || origin.pathname !== '/' || origin.search || origin.hash) {
  throw new Error('Brama ingress requires one credential-free HTTPS origin');
}
const resolver = new URL(settings.dns_endpoint);
if (resolver.protocol !== 'https:') throw new Error('Brama DNS configuration requires HTTPS');

let cached;
let resolving;
async function publicAddresses() {
  if (cached && cached.expires > Date.now()) return cached.addresses;
  if (resolving) return resolving;
  resolving = (async () => {
    const query = new URL(resolver);
    query.searchParams.set('name', origin.hostname);
    query.searchParams.set('type', 'A');
    const response = await fetch(query, { headers: { accept: 'application/dns-json' } });
    if (!response.ok) throw new Error(`DNS lookup for ${origin.hostname} answered HTTP ${response.status}`);
    const answer = await response.json();
    if (answer.Status !== 0) throw new Error(`DNS lookup for ${origin.hostname} returned status ${answer.Status}`);
    const records = (answer.Answer ?? []).filter(record => record.type === 1 && isIP(record.data) === 4);
    if (!records.length) throw new Error(`DNS returned no public IPv4 address for ${origin.hostname}`);
    const addresses = records.map(record => ({ address: record.data, family: 4 }));
    const ttl = Math.min(...records.map(record => Number.isFinite(record.TTL) ? Math.max(0, record.TTL) : 0));
    cached = { addresses, expires: Date.now() + ttl * 1000 };
    return addresses;
  })();
  try { return await resolving; }
  finally { resolving = undefined; }
}

// Only DNS resolution differs from the platform transport. HTTPS still verifies
// the configured hostname, and no request is retried on another origin.
const agent = new Agent({
  keepAlive: true,
  lookup(hostname, options, callback) {
    if (hostname !== origin.hostname) return callback(new Error('Unexpected Brama upstream hostname'));
    publicAddresses().then(addresses => {
      if (options.all) callback(null, addresses);
      else callback(null, addresses[0].address, addresses[0].family);
    }, callback);
  },
});

const HOP_HEADERS = new Set([
  'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization',
  'te', 'trailer', 'transfer-encoding', 'upgrade',
]);
function forwardedHeaders(headers) {
  const excluded = new Set(HOP_HEADERS);
  for (const name of String(headers.connection ?? '').split(',')) excluded.add(name.trim().toLowerCase());
  return Object.fromEntries(Object.entries(headers).filter(([name, value]) => value !== undefined && !excluded.has(name.toLowerCase())));
}

export default async function gateway(request, response) {
  const incoming = new URL(request.url, origin);
  const routedPath = request.query?.__brama_path ?? incoming.searchParams.get('__brama_path') ?? incoming.pathname;
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
        response.writeHead(502, { 'content-type': 'application/json', 'cache-control': 'no-store' });
        response.end(JSON.stringify({
          error: 'Brama ingress could not reach the declared gateway',
          operation: 'proxy request', origin: origin.origin,
          cause: error.message, cause_code: error.code ?? null,
        }));
      } else if (!response.destroyed) response.destroy(error);
      finish();
    };
    const upstream = requestHttps(target, { method: request.method, headers, agent }, received => {
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

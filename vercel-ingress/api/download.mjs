// The site's public download: Brama's published releases, the signed objects
// Stado's release channel holds under stado://releases/brama/<version>/<platform>/,
// read through the /api/release/object route of the origin this deployment
// declares in BRAMA_RELEASE_ORIGIN and streamed back unchanged. vercel.json
// rewrites two shapes here:
//
//   /download/<platform>/<file>            the release of the version these pages document
//   /download/<version>/<platform>/<file>  one exact release
//
// The version these pages document is the one the repository's declared
// version source reads in the source this site was deployed from; the
// platforms are those whose release stages the runtime binary. Only this
// product's release objects are served: no other product, no listing, and no
// model API (every path outside /docs and /download is api/gateway.mjs).
import { readFileSync } from 'node:fs';
import { request as requestHttps } from 'node:https';
import { STATUS_CODES } from 'node:http';

const ORIGIN_VARIABLE = 'BRAMA_RELEASE_ORIGIN';
// Stado's public, bearer-free read of one release object.
const OBJECT_ROUTE = '/api/release/object';
// The objects Stado publishes for every release platform, in publication order.
const FILES = ['release.tar.gz', 'qualification.json', 'release.sig', 'release.json'];
const PAGE = '/docs/download';
// The release declaration and the version source it names, in the deployed source.
const RELEASE_MANIFEST = new URL('../../.wisent-release.json', import.meta.url);
const VERSION_SOURCE = new URL('../../Cargo.toml', import.meta.url);
const VERSION_SOURCE_PATH = 'Cargo.toml';

function statusOf(name) {
  const found = Object.entries(STATUS_CODES).find(([, text]) => text === name);
  if (!found) throw new Error(`Node names no HTTP status "${name}"`);
  const [code] = found;
  return Number(code);
}
const OK = statusOf('OK');
const PARTIAL_CONTENT = statusOf('Partial Content');
const BAD_REQUEST = statusOf('Bad Request');
const NOT_FOUND = statusOf('Not Found');
const METHOD_NOT_ALLOWED = statusOf('Method Not Allowed');
const INTERNAL_SERVER_ERROR = statusOf('Internal Server Error');
const BAD_GATEWAY = statusOf('Bad Gateway');

// Response headers of a served object that describe its bytes.
const OBJECT_HEADERS = ['content-type', 'content-length', 'content-range', 'accept-ranges', 'etag', 'last-modified'];

function refuse(response, status, error) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify({ error: { ...error, retryable: error.retryable ?? false } }));
}

/** Stado's canonical coordinate: ASCII alphanumerics, '.', '_' and '-', and not a path step. */
function coordinate(value) {
  return typeof value === 'string' && /^[A-Za-z0-9._-]+$/.test(value) && value !== '.' && value !== '..';
}

/** The product, the version these pages document and the platforms that ship the binary, or why not. */
function declaredRelease() {
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(RELEASE_MANIFEST, 'utf8'));
  } catch (error) {
    return { broken: `the deployed source's .wisent-release.json cannot be read: ${error.message}` };
  }
  const { product, version_source: source, platforms, runtime } = manifest;
  if (!coordinate(product)) return { broken: `.wisent-release.json declares no product coordinate: ${JSON.stringify(product)}` };
  if (source?.kind !== 'regex' || source.path !== VERSION_SOURCE_PATH || typeof source.pattern !== 'string') {
    return { broken: `.wisent-release.json declares its version source as ${JSON.stringify(source)}; this site reads a regex over ${VERSION_SOURCE_PATH}` };
  }
  let pattern = source.pattern.replaceAll('(?P<', '(?<');
  let flags = '';
  if (pattern.startsWith('(?m)')) {
    pattern = pattern.slice('(?m)'.length);
    flags = 'm';
  }
  let text;
  try {
    text = readFileSync(VERSION_SOURCE, 'utf8');
  } catch (error) {
    return { broken: `the deployed source's ${VERSION_SOURCE_PATH} cannot be read: ${error.message}` };
  }
  const version = text.match(new RegExp(pattern, flags))?.groups?.version;
  if (!coordinate(version)) return { broken: `${VERSION_SOURCE_PATH} declares no version the pattern ${source.pattern} reads` };
  const binary = runtime?.binary;
  const shipping = Object.entries(platforms ?? {})
    .filter(([, platform]) => Object.values(platform?.stage ?? {}).includes(binary))
    .map(([name]) => name);
  if (!shipping.length) return { broken: `.wisent-release.json declares no platform whose release stages ${binary}` };
  return { product, version, platforms: shipping };
}

/** The deployment's declared release-object origin, or why there is none. */
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

function readBody(stream) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    stream.on('data', chunk => chunks.push(chunk));
    stream.once('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    stream.once('error', reject);
  });
}

/** Whether a 404 body is the release channel's own answer that exactly this object is absent. */
function absent(body, uri) {
  try {
    const answer = JSON.parse(body);
    return answer?.state === 'absent' && answer.uri === uri;
  } catch {
    return false;
  }
}

export default async function download(request, response) {
  if (request.method !== 'GET') {
    response.setHeader('allow', 'GET');
    refuse(response, METHOD_NOT_ALLOWED, {
      message: `${request.method} is not served for a release download: the release channel answers GET only. See ${PAGE}`,
      type: 'request_error',
      code: 'method_not_allowed',
    });
    return;
  }
  const release = declaredRelease();
  if (release.broken) {
    refuse(response, INTERNAL_SERVER_ERROR, {
      message: `${release.broken}, so this site cannot say which release it offers. See ${PAGE}#refusals`,
      type: 'server_error',
      code: 'release_declaration_unreadable',
    });
    return;
  }
  const query = request.query ?? Object.fromEntries(new URL(request.url, 'http://ingress.invalid').searchParams);
  const documented = query.version === undefined;
  const version = documented ? release.version : query.version;
  const { platform, file } = query;
  if (!coordinate(version)) {
    refuse(response, BAD_REQUEST, {
      message: `${JSON.stringify(version)} is not a release version: a version is ASCII letters, digits, '.', '_' and '-', such as ${release.version}. See ${PAGE}`,
      type: 'request_error',
      code: 'version_invalid',
    });
    return;
  }
  if (!release.platforms.includes(platform)) {
    refuse(response, NOT_FOUND, {
      message: `${release.product} is released for ${release.platforms.join(' and ')}, not ${JSON.stringify(platform)}. See ${PAGE}`,
      type: 'request_error',
      code: 'platform_unknown',
    });
    return;
  }
  if (!FILES.includes(file)) {
    refuse(response, NOT_FOUND, {
      message: `a release holds ${FILES.join(', ')}, not ${JSON.stringify(file)}. See ${PAGE}`,
      type: 'request_error',
      code: 'file_unknown',
    });
    return;
  }
  const { origin, undeclared, invalid } = declaredOrigin();
  if (undeclared || invalid) {
    refuse(response, INTERNAL_SERVER_ERROR, {
      message: `${invalid ?? `this site's deployment sets no ${ORIGIN_VARIABLE}`}, so it cannot read the release channel; set it to the HTTPS origin that serves ${OBJECT_ROUTE}. See ${PAGE}#refusals`,
      type: 'server_error',
      code: undeclared ? 'release_origin_undeclared' : 'release_origin_invalid',
    });
    return;
  }
  const uri = `stado://releases/${release.product}/${version}/${platform}/${file}`;
  const target = new URL(OBJECT_ROUTE, origin);
  target.searchParams.set('uri', uri);
  const headers = request.headers.range ? { range: request.headers.range } : {};

  await new Promise(resolve => {
    let finished = false;
    const finish = () => { if (!finished) { finished = true; resolve(); } };
    const failed = error => {
      if (finished) return;
      if (!response.destroyed && !response.headersSent) {
        refuse(response, BAD_GATEWAY, {
          message: `the release channel did not answer for ${uri}: ${error.message}. See ${PAGE}#refusals`,
          type: 'server_error',
          code: 'release_origin_unreachable',
          cause_code: error.code,
          retryable: true,
        });
      } else if (!response.destroyed) response.destroy(error);
      finish();
    };
    const upstream = requestHttps(target, { method: 'GET', headers }, async received => {
      received.once('error', failed);
      if (received.statusCode === OK || received.statusCode === PARTIAL_CONTENT) {
        const served = Object.fromEntries(OBJECT_HEADERS.filter(name => received.headers[name] !== undefined).map(name => [name, received.headers[name]]));
        served['content-disposition'] = `attachment; filename="${release.product}-${version}-${platform}-${file}"`;
        served['x-release-uri'] = uri;
        response.writeHead(received.statusCode, served);
        received.once('end', finish);
        received.pipe(response);
        return;
      }
      const body = await readBody(received).catch(error => `(the body could not be read: ${error.message})`);
      if (received.statusCode === NOT_FOUND && absent(body, uri)) {
        refuse(response, NOT_FOUND, {
          message: `${uri} is not published: the release channel answered ${body}. ${documented
            ? `These pages document ${release.product} ${release.version}, and its ${platform} release is not published yet; an earlier release is read at /download/<version>/${platform}/${file}.`
            : `Read release.json of the version these pages document at /download/${platform}/release.json.`} See ${PAGE}`,
          type: 'request_error',
          code: 'release_not_published',
        });
      } else {
        refuse(response, BAD_GATEWAY, {
          message: `the release channel answered ${received.statusCode} for ${uri}: ${body}. See ${PAGE}#refusals`,
          type: 'server_error',
          code: 'release_origin_refused',
        });
      }
      finish();
    });
    upstream.once('error', failed);
    response.once('close', () => {
      if (!response.writableFinished) upstream.destroy(new Error('the downloader disconnected'));
      finish();
    });
    upstream.end();
  });
}
